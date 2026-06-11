//! The in-process **live run-log registry** — the seam that lets the UI poll a
//! run's stdout/stderr/system lines WHILE the adapter process is still running,
//! before the run finalizes.
//!
//! Spec ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10 (LIVE run-log streaming).
//! Reference (BINDING, `docs/reference-driven-development.md`): Paperclip
//! `references/paperclip/server/src/adapters/process/execute.ts` +
//! `packages/adapter-utils/src/server-utils.ts` `runChildProcess(runId, …, { onLog })`
//! streams `(stream, chunk)` to a per-run store as each `child.stdout/stderr.on("data")`
//! chunk is read; the API reads the bounded tail back while the run is in flight.
//! OpenClaw `reference/openclaw-main/src/process/exec.ts` confirms captured output
//! is always `maxBuffer`-bounded.
//!
//! Why a SEPARATE registry (not the kernel `State`): the Relux kernel is loaded
//! from SQLite per request under one big lock, so the finalized `RunLog` lives in
//! the durable store — but it only exists AFTER the run finalizes. To show lines
//! DURING the run, the off-lock orchestration spawn (`run_briefs_in_parallel`, the
//! one path that releases the kernel lock during the process wait) appends streamed
//! lines here, and `GET /v1/relux/runs/:id/logs` reads this registry WITHOUT taking
//! the kernel lock. Once the run finalizes and persists its canonical `RunLog`, the
//! durable log wins and the live entry is dropped ([`LiveRunLogs::finish`]).
//!
//! Bounded + redacted by construction: every line flows through
//! [`relux_core::StreamingRunLog`] (per-line `redact_secrets`, per-line + total-line
//! caps, oldest-dropped tail), and the registry itself caps the number of
//! concurrent live runs so a leak can't grow without bound.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use relux_core::{RunId, RunLog, RunLogSource, StreamingRunLog};

/// A hard backstop on how many runs may have a LIVE buffer at once. Each
/// orchestration round prepares at most the concurrency cap (<= 4) of briefs and
/// [`LiveRunLogs::finish`]es them right after finalize, so the steady state is
/// tiny; this only guards against a leaked entry (e.g. a panicked worker that
/// never finalized) accumulating unbounded. When exceeded, the oldest-inserted
/// stale entry is evicted.
const MAX_LIVE_RUNS: usize = 32;

/// A process-global, cloneable registry of in-flight run-log buffers, keyed by run
/// id. Cheap to clone (an `Arc`); held on the server `AppState` and shared with the
/// off-lock spawn workers. All access is guarded by its own mutex, INDEPENDENT of
/// the kernel store lock, so a live poll never blocks on (or is blocked by) a
/// kernel operation.
#[derive(Clone, Default)]
pub struct LiveRunLogs {
    inner: Arc<Mutex<LiveRunLogsInner>>,
}

#[derive(Default)]
struct LiveRunLogsInner {
    /// run id → its shared streaming buffer.
    runs: HashMap<String, Arc<Mutex<StreamingRunLog>>>,
    /// Insertion order of the live run ids, so the backstop can evict the oldest.
    order: Vec<String>,
}

impl LiveRunLogs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a live buffer for `run_id` and return a [`RunLogSink`] the spawn feeds
    /// as it reads the process output. Creating the entry up front means a poll that
    /// arrives the instant the process starts already finds the (initially empty)
    /// live buffer. Re-beginning an existing run id replaces its buffer (a fresh
    /// run/retry id is distinct, so this only matters defensively).
    pub fn begin(&self, run_id: &RunId) -> RunLogSink {
        let cell = Arc::new(Mutex::new(StreamingRunLog::new()));
        let key = run_id.as_str().to_string();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.runs.insert(key.clone(), cell.clone()).is_none() {
            inner.order.push(key);
        }
        // Backstop: evict the oldest stale entries beyond the cap.
        while inner.order.len() > MAX_LIVE_RUNS {
            let oldest = inner.order.remove(0);
            inner.runs.remove(&oldest);
        }
        RunLogSink { cell }
    }

    /// A bounded, redacted snapshot of the live tail for `run_id`, optionally only
    /// the lines past the `since` cursor — or `None` when no live buffer exists
    /// (the run never streamed, or already finalized + was dropped). The caller
    /// falls back to the durable kernel log in that case.
    pub fn snapshot(&self, run_id: &RunId, since: Option<u32>) -> Option<RunLog> {
        let cell = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.runs.get(run_id.as_str()).cloned()?
        };
        let guard = cell.lock().unwrap_or_else(|e| e.into_inner());
        Some(guard.snapshot(run_id.clone()).since(since))
    }

    /// Drop the live buffer for `run_id` — called once its canonical `RunLog` has
    /// been finalized + persisted, so subsequent reads serve the durable log and
    /// the buffer's memory is freed.
    pub fn finish(&self, run_id: &RunId) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.runs.remove(run_id.as_str()).is_some() {
            inner.order.retain(|k| k != run_id.as_str());
        }
    }

    /// The number of live buffers currently held (for tests / introspection).
    pub fn live_count(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).runs.len()
    }
}

/// The append handle handed to an adapter spawn. Cloneable + `Send + Sync` so each
/// of the two reader threads (stdout, stderr) can hold its own clone and append
/// concurrently — appends are serialized by the inner mutex. Holds only an `Arc`
/// to the shared [`StreamingRunLog`]; dropping it does NOT remove the registry
/// entry (a poll can still read the buffer until [`LiveRunLogs::finish`]).
#[derive(Clone)]
pub struct RunLogSink {
    cell: Arc<Mutex<StreamingRunLog>>,
}

impl RunLogSink {
    /// Append a kernel-authored `system` line (spawn / exit / timeout framing).
    pub fn system(&self, text: impl Into<String>) {
        let mut guard = self.cell.lock().unwrap_or_else(|e| e.into_inner());
        guard.push_system(text);
    }

    /// Append a raw stdout/stderr chunk (line-buffered + redacted + clamped by the
    /// inner [`StreamingRunLog`]).
    pub fn append(&self, source: RunLogSource, chunk: &str) {
        let mut guard = self.cell.lock().unwrap_or_else(|e| e.into_inner());
        guard.append_chunk(source, chunk);
    }

    /// Mark that one stream was byte-capped mid-run (the reader hit the cap and
    /// stopped feeding further chunks), so the live tail shows an honest marker.
    pub fn mark_source_truncation(&self, source: RunLogSource) {
        let mut guard = self.cell.lock().unwrap_or_else(|e| e.into_inner());
        guard.mark_source_truncation(source);
    }

    /// Flush any held partial (newline-less) lines as final lines — called once the
    /// process output is fully drained.
    pub fn flush(&self) {
        let mut guard = self.cell.lock().unwrap_or_else(|e| e.into_inner());
        guard.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(s: &str) -> RunId {
        RunId::new(s)
    }

    #[test]
    fn live_lines_are_visible_via_snapshot_before_finish() {
        let live = LiveRunLogs::new();
        let id = rid("run_live_1");
        let sink = live.begin(&id);
        sink.system("spawned adapter");
        sink.append(RunLogSource::Stdout, "first line\n");
        // A poll mid-run sees the system + complete stdout line already.
        let snap = live.snapshot(&id, None).expect("live buffer exists");
        assert_eq!(snap.lines.len(), 2);
        assert_eq!(snap.lines[0].source, RunLogSource::System);
        assert_eq!(snap.lines[1].text, "first line");

        // More output arrives; the incremental ?since=<seq> tail returns only new.
        sink.append(RunLogSource::Stderr, "an error\n");
        let tail = live.snapshot(&id, Some(2)).expect("still live");
        assert_eq!(tail.lines.len(), 1);
        assert_eq!(tail.lines[0].source, RunLogSource::Stderr);
        assert_eq!(tail.lines[0].text, "an error");
    }

    #[test]
    fn finish_drops_the_buffer_and_snapshot_returns_none() {
        let live = LiveRunLogs::new();
        let id = rid("run_live_2");
        let sink = live.begin(&id);
        sink.append(RunLogSource::Stdout, "x\n");
        assert!(live.snapshot(&id, None).is_some());
        live.finish(&id);
        assert!(live.snapshot(&id, None).is_none(), "finished run has no live buffer");
        assert_eq!(live.live_count(), 0);
    }

    #[test]
    fn snapshot_is_none_for_an_unknown_run() {
        let live = LiveRunLogs::new();
        assert!(live.snapshot(&rid("never_began"), None).is_none());
    }

    #[test]
    fn registry_is_bounded_by_the_live_run_backstop() {
        let live = LiveRunLogs::new();
        for i in 0..(MAX_LIVE_RUNS + 10) {
            let _ = live.begin(&rid(&format!("run_{i}")));
        }
        assert_eq!(live.live_count(), MAX_LIVE_RUNS, "live registry must stay bounded");
        // The oldest were evicted; the newest remain readable.
        assert!(live.snapshot(&rid("run_0"), None).is_none());
        assert!(live
            .snapshot(&rid(&format!("run_{}", MAX_LIVE_RUNS + 9)), None)
            .is_some());
    }

    #[test]
    fn two_sink_clones_append_to_the_same_buffer() {
        // Mirrors the spawn: stdout + stderr reader threads each hold a clone.
        let live = LiveRunLogs::new();
        let id = rid("run_live_3");
        let sink = live.begin(&id);
        let out = sink.clone();
        let err = sink.clone();
        out.append(RunLogSource::Stdout, "out\n");
        err.append(RunLogSource::Stderr, "err\n");
        let snap = live.snapshot(&id, None).unwrap();
        assert_eq!(snap.lines.len(), 2);
    }
}
