//! Bounded, redacted **run log / tail** for an adapter run.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.7 (Run Event) and
//! `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8 (CLI / process / runtime adapters) +
//! §10 (UI / product ergonomics — live run-log tail). Reference mapping
//! (`docs/reference-driven-development.md`, BINDING):
//!
//! - **Paperclip** `references/paperclip/server/src/services/run-log-store.ts`:
//!   the run-log store appends per-line events `{ ts, stream, chunk }` where
//!   `stream` is one of `"stdout" | "stderr" | "system"`, and `read({ offset,
//!   limitBytes })` returns a **bounded, offset-cursored** slice `{ content,
//!   nextOffset }` (default `limitBytes: 256_000`). The three-stream
//!   classification, the per-line shape, and the bounded pollable read are the
//!   model mirrored here.
//! - **Paperclip** `references/paperclip/server/src/adapters/process/execute.ts`
//!   (`runChildProcess(runId, …, { onLog })` streams stdout/stderr/system chunks)
//!   confirms the source taxonomy.
//! - **OpenClaw** `reference/openclaw-main/src/process/exec.ts` (`maxBuffer`
//!   bound on captured output) confirms output is always bounded, never
//!   unlimited.
//!
//! Relux mapping — this is the bounded, **deterministic** metadata layer. The
//! kernel's adapter spawn ([`crate::AdapterKind`]) captures the run's final,
//! already-redacted, byte-capped stdout/stderr (it does not stream `onLog`
//! chunks during the run — that is a future seam). So Relux persists a **bounded
//! final tail**: the captured stdout/stderr split into per-line entries
//! classified `stdout`/`stderr`, plus kernel-authored `system` lines (spawn,
//! exit, timeout). Every line is re-redacted defensively and clamped; the whole
//! tail is clamped to a line cap (oldest dropped, count recorded) so a runaway
//! run can never bloat the record. The `seq` cursor is the pollable analogue of
//! Paperclip's byte `offset` — a client polls `?since=<seq>` and merges only the
//! lines past its cursor. We store ONLY bounded, redacted line text — never a raw
//! provider envelope, token, or unbounded log.

use serde::{Deserialize, Serialize};

use crate::redact::redact_secrets;
use crate::run::RunId;

/// The maximum number of log lines kept for one run. When more lines are
/// captured, the OLDEST are dropped (a tail) and the dropped count is recorded
/// honestly so the UI can show a truncation marker. Bounds the per-run record.
pub const MAX_LOG_LINES: usize = 200;

/// The maximum length (in chars) of a single log line. A longer line is clamped
/// and its `truncated` flag set, so one pathological line can never bloat the
/// record. Mirrors the per-result clamp the kernel already applies elsewhere.
pub const MAX_LOG_LINE_CHARS: usize = 2_000;

/// Which stream a captured log line came from. Mirrors Paperclip's
/// `stream: "stdout" | "stderr" | "system"` run-log taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLogSource {
    /// A line from the adapter process's standard output.
    Stdout,
    /// A line from the adapter process's standard error.
    Stderr,
    /// A kernel-authored line about the run lifecycle (spawn, exit, timeout).
    /// Never raw process output — always Relux's own bounded note.
    System,
}

impl RunLogSource {
    /// The stable wire string for this source.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunLogSource::Stdout => "stdout",
            RunLogSource::Stderr => "stderr",
            RunLogSource::System => "system",
        }
    }
}

/// One bounded, redacted line in a run's log tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLogLine {
    /// A monotonic per-run sequence number (1-based), assigned in capture order.
    /// The pollable cursor compares on this (the analogue of Paperclip's byte
    /// `offset`): `?since=<seq>` returns only lines with `seq > since`.
    pub seq: u32,
    /// Which stream this line came from.
    pub source: RunLogSource,
    /// The redacted, clamped line text (never a raw secret; never unbounded).
    pub text: String,
    /// True when this individual line was longer than [`MAX_LOG_LINE_CHARS`] and
    /// was clamped — an honest per-line truncation marker.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The bounded, redacted log tail for one run.
///
/// Empty (no lines, no truncation) for a run that captured no output — e.g. the
/// deterministic local-echo path, or a run that has not executed. The reader
/// returns this empty shape rather than erroring, so the UI never blanks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLog {
    pub run_id: RunId,
    /// The kept lines, in capture order (oldest first), already clamped to
    /// [`MAX_LOG_LINES`].
    pub lines: Vec<RunLogLine>,
    /// How many of the OLDEST lines were dropped to respect [`MAX_LOG_LINES`].
    /// Non-zero ⇒ the UI shows a "N earlier lines dropped" marker. Honest: never
    /// hidden.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dropped_lines: u32,
    /// True when the adapter's stdout was byte-capped at capture time (the
    /// upstream [`crate::AdapterKind`] spawn cap), so the captured stdout is a
    /// prefix of what the process produced.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stdout_truncated: bool,
    /// True when the adapter's stderr was byte-capped at capture time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stderr_truncated: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl RunLog {
    /// An empty log for `run_id` (no lines, no truncation). Returned by the
    /// reader for a run with no captured log so callers never have to special-
    /// case "missing".
    pub fn empty(run_id: RunId) -> Self {
        RunLog {
            run_id,
            lines: Vec::new(),
            dropped_lines: 0,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    /// True when this log carries no lines (the "No logs" UI state).
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Return only the lines strictly after the `since` sequence cursor — the
    /// incremental pollable tail (Paperclip `read({ offset })` → `nextOffset`).
    /// `None`/absent returns every line (a first load or recovery). The returned
    /// `RunLog` keeps the run-level truncation flags + dropped count so the UI
    /// can always render the markers, even on an incremental fetch.
    pub fn since(&self, since: Option<u32>) -> RunLog {
        let lines = match since {
            Some(cursor) => self
                .lines
                .iter()
                .filter(|l| l.seq > cursor)
                .cloned()
                .collect(),
            None => self.lines.clone(),
        };
        RunLog {
            run_id: self.run_id.clone(),
            lines,
            dropped_lines: self.dropped_lines,
            stdout_truncated: self.stdout_truncated,
            stderr_truncated: self.stderr_truncated,
        }
    }

    /// The highest sequence number present (the exclusive cursor for the next
    /// poll). `None` for an empty log.
    pub fn latest_seq(&self) -> Option<u32> {
        self.lines.last().map(|l| l.seq)
    }
}

/// Accumulates classified, redacted, clamped lines and produces a bounded
/// [`RunLog`]. Pure and deterministic: no clock, no IO. The kernel feeds it the
/// run's captured (already-redacted, byte-capped) stdout/stderr split by source
/// plus its own `system` lines; this builder re-redacts each line defensively,
/// clamps line length, and clamps the total to [`MAX_LOG_LINES`] (oldest
/// dropped, count recorded).
#[derive(Debug, Default)]
pub struct RunLogBuilder {
    pending: Vec<(RunLogSource, String, bool)>,
    /// How many of the OLDEST lines have already been dropped to keep `pending`
    /// bounded to [`MAX_LOG_LINES`]. Accumulated continuously (so a long, LIVE
    /// stream can never grow `pending` without bound), then carried into the
    /// built [`RunLog`] as `dropped_lines`.
    dropped: usize,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

impl RunLogBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that the adapter's stdout/stderr were byte-capped upstream so the
    /// run-level truncation markers are honest.
    pub fn mark_stream_truncation(&mut self, stdout_truncated: bool, stderr_truncated: bool) {
        self.stdout_truncated = self.stdout_truncated || stdout_truncated;
        self.stderr_truncated = self.stderr_truncated || stderr_truncated;
    }

    /// Push one kernel-authored `system` line (spawn, exit, timeout). The text is
    /// redacted + clamped like any other line.
    pub fn push_system(&mut self, text: impl Into<String>) {
        self.push_line(RunLogSource::System, &text.into());
    }

    /// Split a captured output blob into per-line entries of the given source.
    /// An empty / whitespace-only blob contributes nothing (we never fabricate a
    /// blank line). A trailing newline does not produce a spurious empty line.
    pub fn push_output(&mut self, source: RunLogSource, blob: &str) {
        if blob.trim().is_empty() {
            return;
        }
        for raw in blob.split('\n') {
            // Drop a pure carriage-return tail (CRLF) and skip lines that are
            // empty after trimming so a blank gap doesn't waste the line budget.
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if line.trim().is_empty() {
                continue;
            }
            self.push_line(source, line);
        }
    }

    fn push_line(&mut self, source: RunLogSource, raw: &str) {
        // Re-redact defensively even though the kernel already redacted the blob:
        // the run log is a distinct surface and must never carry a secret on its
        // own account.
        let redacted = redact_secrets(raw);
        let mut truncated = false;
        let text: String = if redacted.chars().count() > MAX_LOG_LINE_CHARS {
            truncated = true;
            redacted.chars().take(MAX_LOG_LINE_CHARS).collect()
        } else {
            redacted
        };
        self.pending.push((source, text, truncated));
        self.enforce_live_cap();
    }

    /// Keep `pending` bounded to [`MAX_LOG_LINES`] by dropping the OLDEST lines as
    /// they accumulate (a live tail), counting each into `dropped`. Called after
    /// every push so memory is bounded WHILE a long run streams — not only at
    /// `build` time. The total `dropped` is identical to the finalize-time
    /// `total - MAX_LOG_LINES`, so the built record is unchanged for batch callers.
    fn enforce_live_cap(&mut self) {
        while self.pending.len() > MAX_LOG_LINES {
            self.pending.remove(0);
            self.dropped += 1;
        }
    }

    /// Assemble a bounded [`RunLog`] from the kept `pending` lines (already capped
    /// to [`MAX_LOG_LINES`] by [`Self::enforce_live_cap`]). Sequence numbers are
    /// assigned 1-based over the kept lines in order, so the pollable cursor is
    /// dense and monotonic. Shared by [`Self::build`] (consuming) and
    /// [`Self::snapshot`] (non-consuming, for a live tail).
    fn assemble(&self, run_id: RunId) -> RunLog {
        let lines: Vec<RunLogLine> = self
            .pending
            .iter()
            .enumerate()
            .map(|(i, (source, text, truncated))| RunLogLine {
                seq: (i as u32) + 1,
                source: *source,
                text: text.clone(),
                truncated: *truncated,
            })
            .collect();
        RunLog {
            run_id,
            lines,
            dropped_lines: self.dropped.min(u32::MAX as usize) as u32,
            stdout_truncated: self.stdout_truncated,
            stderr_truncated: self.stderr_truncated,
        }
    }

    /// A non-consuming snapshot of the bounded log so far — the live-tail read used
    /// while a run is still streaming (the in-progress poll). Reflects only the
    /// COMPLETE lines accumulated to this point (a streamed partial line not yet
    /// terminated by a newline is held in the [`StreamingRunLog`] carry and appears
    /// once its newline arrives — honest tail behaviour).
    pub fn snapshot(&self, run_id: RunId) -> RunLog {
        self.assemble(run_id)
    }

    /// Finalize into a bounded [`RunLog`] for `run_id`. The total is clamped to
    /// [`MAX_LOG_LINES`] by dropping the OLDEST lines (a tail); the dropped count
    /// is recorded. Sequence numbers are assigned 1-based over the KEPT lines in
    /// order, so the pollable cursor is dense and monotonic.
    pub fn build(self, run_id: RunId) -> RunLog {
        self.assemble(run_id)
    }
}

/// A line-buffering, bounded accumulator for **live** run-log streaming: the
/// adapter spawn feeds raw stdout/stderr CHUNKS (which may split a line across a
/// read boundary) as they are read, and this type emits only COMPLETE lines into
/// an inner [`RunLogBuilder`] (each re-redacted + clamped), holding the trailing
/// partial line per source until its newline arrives. A non-consuming
/// [`Self::snapshot`] yields the bounded [`RunLog`] so far so the UI can poll a
/// run BEFORE it finalizes.
///
/// Spec ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10 (LIVE run-log streaming).
/// Reference: Paperclip `runChildProcess(..., { onLog })` streams `(stream, chunk)`
/// per read; Relux line-buffers the chunk into the same bounded, redacted, three-
/// stream model rather than persisting raw NDJSON chunks. Pure and deterministic
/// (no clock, no IO); the threading/registry around it lives in the kernel.
#[derive(Debug, Default)]
pub struct StreamingRunLog {
    builder: RunLogBuilder,
    /// The trailing partial (newline-less) stdout text awaiting more chunks.
    stdout_carry: String,
    /// The trailing partial (newline-less) stderr text awaiting more chunks.
    stderr_carry: String,
}

impl StreamingRunLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one kernel-authored `system` line (spawn, exit, timeout). Whole-line
    /// by construction (the kernel authors it), so it is emitted immediately.
    pub fn push_system(&mut self, text: impl Into<String>) {
        self.builder.push_system(text);
    }

    /// Record that the adapter's stdout/stderr were byte-capped upstream so the
    /// run-level truncation markers are honest on the live tail too.
    pub fn mark_stream_truncation(&mut self, stdout_truncated: bool, stderr_truncated: bool) {
        self.builder
            .mark_stream_truncation(stdout_truncated, stderr_truncated);
    }

    /// Mark the truncation flag for one stream (used when the spawn reader hits the
    /// byte cap mid-stream and stops feeding further chunks).
    pub fn mark_source_truncation(&mut self, source: RunLogSource) {
        match source {
            RunLogSource::Stdout => self.builder.mark_stream_truncation(true, false),
            RunLogSource::Stderr => self.builder.mark_stream_truncation(false, true),
            // A system line is kernel-authored and never byte-capped.
            RunLogSource::System => {}
        }
    }

    /// Append a raw output chunk for `source`. Complete lines (terminated by `\n`)
    /// are emitted into the bounded builder immediately; the trailing partial line
    /// is carried until its newline arrives. A pathologically long carry with no
    /// newline is force-emitted once it exceeds [`MAX_LOG_LINE_CHARS`] so neither
    /// the carry nor memory can grow without bound. `system` chunks are treated as
    /// whole lines (split on `\n`, no carry).
    pub fn append_chunk(&mut self, source: RunLogSource, chunk: &str) {
        if source == RunLogSource::System {
            self.builder.push_output(RunLogSource::System, chunk);
            return;
        }
        let carry = match source {
            RunLogSource::Stdout => &mut self.stdout_carry,
            RunLogSource::Stderr => &mut self.stderr_carry,
            RunLogSource::System => unreachable!(),
        };
        carry.push_str(chunk);
        // Emit every complete line (the carry may hold several after one chunk).
        while let Some(idx) = carry.find('\n') {
            let line: String = carry[..idx].to_string();
            // Drop the consumed line + its newline from the carry.
            let rest = carry[idx + 1..].to_string();
            *carry = rest;
            // `push_output` strips a trailing `\r`, skips an empty line, re-redacts
            // and clamps — exactly the finalize-path treatment, per single line.
            self.builder.push_output(source, &line);
        }
        // Bound the carry: a single line longer than the per-line cap is emitted
        // now (clamped) rather than buffered indefinitely.
        if carry.chars().count() > MAX_LOG_LINE_CHARS {
            let pending = std::mem::take(carry);
            self.builder.push_output(source, &pending);
        }
    }

    /// Flush any held partial lines as final lines (called at run end, after the
    /// last chunk). A process whose last line had no trailing newline still shows.
    pub fn flush(&mut self) {
        let stdout = std::mem::take(&mut self.stdout_carry);
        if !stdout.trim().is_empty() {
            self.builder.push_output(RunLogSource::Stdout, &stdout);
        }
        let stderr = std::mem::take(&mut self.stderr_carry);
        if !stderr.trim().is_empty() {
            self.builder.push_output(RunLogSource::Stderr, &stderr);
        }
    }

    /// A non-consuming bounded [`RunLog`] of the COMPLETE lines so far — the live
    /// poll read. Partial (carry) lines are not included until their newline
    /// arrives.
    pub fn snapshot(&self, run_id: RunId) -> RunLog {
        self.builder.snapshot(run_id)
    }

    /// Flush held partials, then consume into the final bounded [`RunLog`].
    pub fn into_log(mut self, run_id: RunId) -> RunLog {
        self.flush();
        self.builder.build(run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid() -> RunId {
        RunId::new("run_0001")
    }

    #[test]
    fn classifies_stdout_stderr_and_system_lines() {
        let mut b = RunLogBuilder::new();
        b.push_system("spawning claude adapter");
        b.push_output(RunLogSource::Stdout, "first out\nsecond out");
        b.push_output(RunLogSource::Stderr, "an error line");
        let log = b.build(rid());
        let sources: Vec<RunLogSource> = log.lines.iter().map(|l| l.source).collect();
        assert_eq!(
            sources,
            vec![
                RunLogSource::System,
                RunLogSource::Stdout,
                RunLogSource::Stdout,
                RunLogSource::Stderr,
            ]
        );
        // Sequence numbers are dense and 1-based.
        assert_eq!(log.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(log.lines[1].text, "first out");
        assert_eq!(log.lines[2].text, "second out");
        assert!(!log.is_empty());
        assert_eq!(log.latest_seq(), Some(4));
    }

    #[test]
    fn empty_and_whitespace_blobs_contribute_nothing() {
        let mut b = RunLogBuilder::new();
        b.push_output(RunLogSource::Stdout, "");
        b.push_output(RunLogSource::Stderr, "   \n  \n");
        let log = b.build(rid());
        assert!(log.is_empty());
        assert_eq!(log.latest_seq(), None);
        // The reader's empty shape matches a built-empty log.
        assert_eq!(log.lines, RunLog::empty(rid()).lines);
    }

    #[test]
    fn trailing_newline_does_not_make_a_blank_line() {
        let mut b = RunLogBuilder::new();
        b.push_output(RunLogSource::Stdout, "only line\n");
        let log = b.build(rid());
        assert_eq!(log.lines.len(), 1);
        assert_eq!(log.lines[0].text, "only line");
    }

    #[test]
    fn long_line_is_clamped_with_a_truncation_marker() {
        let mut b = RunLogBuilder::new();
        let long = "x".repeat(MAX_LOG_LINE_CHARS + 50);
        b.push_output(RunLogSource::Stdout, &long);
        let log = b.build(rid());
        assert_eq!(log.lines.len(), 1);
        assert_eq!(log.lines[0].text.chars().count(), MAX_LOG_LINE_CHARS);
        assert!(log.lines[0].truncated);
    }

    #[test]
    fn line_count_is_capped_oldest_dropped_and_counted() {
        let mut b = RunLogBuilder::new();
        for i in 0..(MAX_LOG_LINES + 25) {
            b.push_output(RunLogSource::Stdout, &format!("line {i}"));
        }
        let log = b.build(rid());
        assert_eq!(log.lines.len(), MAX_LOG_LINES);
        assert_eq!(log.dropped_lines, 25);
        // The OLDEST were dropped: the first kept line is "line 25", and seq is
        // re-densified to start at 1.
        assert_eq!(log.lines[0].text, "line 25");
        assert_eq!(log.lines[0].seq, 1);
        assert_eq!(log.lines.last().unwrap().text, format!("line {}", MAX_LOG_LINES + 24));
    }

    #[test]
    fn secrets_are_redacted_per_line_even_if_already_redacted() {
        let mut b = RunLogBuilder::new();
        b.push_output(RunLogSource::Stdout, "token is sk-ant-abcdefghijklmnopqrstuvwxyz0123456789");
        let log = b.build(rid());
        assert_eq!(log.lines.len(), 1);
        assert!(
            !log.lines[0].text.contains("abcdefghijklmnopqrstuvwxyz"),
            "secret must be redacted in the line text: {}",
            log.lines[0].text
        );
    }

    #[test]
    fn stream_truncation_markers_are_recorded() {
        let mut b = RunLogBuilder::new();
        b.mark_stream_truncation(true, false);
        b.push_output(RunLogSource::Stdout, "capped output");
        let log = b.build(rid());
        assert!(log.stdout_truncated);
        assert!(!log.stderr_truncated);
    }

    #[test]
    fn since_cursor_returns_only_the_exclusive_tail() {
        let mut b = RunLogBuilder::new();
        for i in 0..5 {
            b.push_output(RunLogSource::Stdout, &format!("line {i}"));
        }
        let log = b.build(rid());
        // No cursor ⇒ everything.
        assert_eq!(log.since(None).lines.len(), 5);
        // After seq 3 ⇒ only seq 4 and 5.
        let tail = log.since(Some(3));
        assert_eq!(tail.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![4, 5]);
        // Past the end ⇒ empty, but the run-level flags survive.
        assert!(log.since(Some(5)).lines.is_empty());
        assert!(log.since(Some(99)).lines.is_empty());
    }

    #[test]
    fn since_preserves_run_level_truncation_markers() {
        let mut b = RunLogBuilder::new();
        b.mark_stream_truncation(true, true);
        for i in 0..(MAX_LOG_LINES + 5) {
            b.push_output(RunLogSource::Stdout, &format!("line {i}"));
        }
        let log = b.build(rid());
        let tail = log.since(log.latest_seq());
        assert!(tail.lines.is_empty());
        assert!(tail.stdout_truncated);
        assert!(tail.stderr_truncated);
        assert_eq!(tail.dropped_lines, 5);
    }

    #[test]
    fn round_trips_on_the_wire_and_omits_empty_markers() {
        let mut b = RunLogBuilder::new();
        b.push_output(RunLogSource::Stdout, "hello");
        let log = b.build(rid());
        let json = serde_json::to_value(&log).unwrap();
        // A clean (non-truncated, nothing dropped) log omits the marker fields.
        assert!(json.get("dropped_lines").is_none());
        assert!(json.get("stdout_truncated").is_none());
        assert!(json.get("stderr_truncated").is_none());
        // A clean line omits its `truncated` flag.
        let line0 = &json["lines"][0];
        assert!(line0.get("truncated").is_none());
        assert_eq!(line0["source"], "stdout");
        let back: RunLog = serde_json::from_value(json).unwrap();
        assert_eq!(back, log);
    }

    // --- StreamingRunLog (the LIVE per-chunk path) -------------------------

    #[test]
    fn streaming_emits_complete_lines_and_holds_the_partial_carry() {
        let mut s = StreamingRunLog::new();
        s.push_system("spawned adapter");
        // A chunk that splits a line across a read boundary: "first\nsec" —
        // "first" is complete, "sec" is a partial carry.
        s.append_chunk(RunLogSource::Stdout, "first\nsec");
        let snap = s.snapshot(rid());
        // The system line + the one complete stdout line are visible; "sec" is NOT
        // yet (it has no newline) — honest live-tail behaviour.
        assert_eq!(snap.lines.len(), 2);
        assert_eq!(snap.lines[0].source, RunLogSource::System);
        assert_eq!(snap.lines[1].text, "first");
        // The rest of the line arrives in the next chunk.
        s.append_chunk(RunLogSource::Stdout, "ond\nthird\n");
        let snap2 = s.snapshot(rid());
        assert_eq!(
            snap2.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["spawned adapter", "first", "second", "third"]
        );
    }

    #[test]
    fn streaming_classifies_each_source_independently() {
        let mut s = StreamingRunLog::new();
        s.append_chunk(RunLogSource::Stdout, "out line\n");
        s.append_chunk(RunLogSource::Stderr, "err line\n");
        let snap = s.snapshot(rid());
        assert_eq!(snap.lines[0].source, RunLogSource::Stdout);
        assert_eq!(snap.lines[0].text, "out line");
        assert_eq!(snap.lines[1].source, RunLogSource::Stderr);
        assert_eq!(snap.lines[1].text, "err line");
    }

    #[test]
    fn streaming_flush_emits_the_trailing_partial_line() {
        let mut s = StreamingRunLog::new();
        s.append_chunk(RunLogSource::Stdout, "no trailing newline");
        // Before flush, the partial is held back.
        assert!(s.snapshot(rid()).is_empty());
        let log = s.into_log(rid());
        assert_eq!(log.lines.len(), 1);
        assert_eq!(log.lines[0].text, "no trailing newline");
    }

    #[test]
    fn streaming_redacts_each_streamed_line() {
        let mut s = StreamingRunLog::new();
        s.append_chunk(
            RunLogSource::Stdout,
            "token sk-ant-abcdefghijklmnopqrstuvwxyz0123456789\n",
        );
        let snap = s.snapshot(rid());
        assert_eq!(snap.lines.len(), 1);
        assert!(
            !snap.lines[0].text.contains("abcdefghijklmnopqrstuvwxyz"),
            "secret must be redacted in the streamed line: {}",
            snap.lines[0].text
        );
    }

    #[test]
    fn streaming_force_emits_a_carry_longer_than_the_line_cap() {
        let mut s = StreamingRunLog::new();
        // A long chunk with NO newline must not be buffered forever: it is
        // force-emitted (clamped) once it exceeds the per-line cap.
        let long = "y".repeat(MAX_LOG_LINE_CHARS + 100);
        s.append_chunk(RunLogSource::Stdout, &long);
        let snap = s.snapshot(rid());
        assert_eq!(snap.lines.len(), 1);
        assert_eq!(snap.lines[0].text.chars().count(), MAX_LOG_LINE_CHARS);
        assert!(snap.lines[0].truncated);
    }

    #[test]
    fn streaming_is_bounded_to_the_line_cap_oldest_dropped_live() {
        let mut s = StreamingRunLog::new();
        for i in 0..(MAX_LOG_LINES + 30) {
            s.append_chunk(RunLogSource::Stdout, &format!("line {i}\n"));
        }
        // Even mid-stream (before finalize) the snapshot is already bounded — the
        // builder caps `pending` continuously, so memory can't grow without bound.
        let snap = s.snapshot(rid());
        assert_eq!(snap.lines.len(), MAX_LOG_LINES);
        assert_eq!(snap.dropped_lines, 30);
        assert_eq!(snap.lines[0].text, "line 30");
        assert_eq!(snap.lines[0].seq, 1);
    }

    #[test]
    fn builder_live_cap_matches_batch_drop_semantics() {
        // The continuous live-cap must produce the SAME dropped count + tail as the
        // old build-time drop, so batch (finalize) callers are unaffected.
        let mut b = RunLogBuilder::new();
        for i in 0..(MAX_LOG_LINES + 25) {
            b.push_output(RunLogSource::Stdout, &format!("line {i}"));
        }
        let log = b.build(rid());
        assert_eq!(log.lines.len(), MAX_LOG_LINES);
        assert_eq!(log.dropped_lines, 25);
        assert_eq!(log.lines[0].text, "line 25");
        assert_eq!(log.lines[0].seq, 1);
    }

    #[test]
    fn streaming_snapshot_respects_the_since_cursor() {
        let mut s = StreamingRunLog::new();
        s.append_chunk(RunLogSource::Stdout, "a\nb\nc\n");
        let full = s.snapshot(rid());
        assert_eq!(full.lines.len(), 3);
        let tail = full.since(Some(1));
        assert_eq!(tail.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![2, 3]);
    }
}
