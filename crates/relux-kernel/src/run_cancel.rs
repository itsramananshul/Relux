//! The in-process **run-cancellation registry** — the seam that lets an operator
//! kill an in-flight, process-backed adapter run mid-flight, before it finalizes.
//!
//! Spec ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8 (CLI / process / runtime
//! adapters — the P2 "mid-run cancellation (`AbortSignal`-style)" slice) and §26.
//! Reference (BINDING, `docs/reference-driven-development.md`): OpenClaw
//! `reference/openclaw-main/src/process/exec.ts` threads an `AbortSignal` into the
//! child spawn and kills the process when it fires; Paperclip
//! `references/paperclip/server/src/adapters/process/execute.ts` `runChildProcess`
//! kills the child on `timeoutSec`/`graceSec`. Relux already kills a child on
//! timeout in [`crate::adapter`]; this module reuses that exact kill path but fires
//! it from an operator-requested **cancel flag** instead of a wall-clock deadline.
//!
//! ## Why a SEPARATE registry (not the kernel `State`)
//!
//! Cancellation must reach a child process that is spawned with the kernel lock
//! RELEASED — the off-lock parallel orchestration window
//! ([`crate::run_briefs_in_parallel_streaming`]). The kernel lock is held by the
//! finalize phase during that window, so a cancel that took the kernel lock could
//! not run until the round already finished. So the cancel flag lives here, in a
//! process-global registry INDEPENDENT of the kernel lock, exactly like
//! [`crate::live_run_log::LiveRunLogs`]: the spawn polls the flag between its
//! existing 40ms `try_wait` ticks and kills its own child when the flag is set; the
//! HTTP handler sets the flag WITHOUT taking the kernel lock, so a cancel is never
//! blocked by (or blocks) a kernel operation.
//!
//! ## Honesty
//!
//! Only a run that is actually streaming off-lock has a live [`CancelToken`] in
//! this registry. A request for any other run — unknown, already finished, or a
//! synchronous lock-holding run that no reader can interleave with anyway — returns
//! [`CancelOutcome::NotRunning`]; the kernel never claims to have cancelled
//! something it cannot reach. A second request for an already-cancelled run is
//! [`CancelOutcome::AlreadyRequested`] (idempotent). The registry is bounded by a
//! backstop so a leaked token can never grow without bound.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use relux_core::RunId;

/// A hard backstop on how many runs may have a live cancel token at once. Each
/// orchestration round prepares at most the concurrency cap (<= 4) of briefs and
/// [`RunCancellations::finish`]es them right after finalize, so the steady state is
/// tiny; this only guards a leaked entry (e.g. a panicked worker that never
/// finalized). When exceeded, the oldest-inserted stale entry is evicted.
const MAX_LIVE_CANCELS: usize = 32;

/// The shared cancellation state for one in-flight process run. The spawn holds an
/// `Arc` to it via a [`CancelToken`] and reads `cancelled` between its poll ticks;
/// the registry's [`RunCancellations::request`] sets `cancelled`. `pid` is recorded
/// by the spawn once the child exists, so a best-effort tree kill can target it.
#[derive(Debug)]
pub struct CancelState {
    cancelled: AtomicBool,
    /// The OS pid of the spawned child (0 until the spawn records it). Used only
    /// for a best-effort process-tree kill; the immediate child is always killed
    /// via its owned [`std::process::Child`] handle regardless.
    pid: AtomicU32,
}

impl CancelState {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            pid: AtomicU32::new(0),
        }
    }

    /// Whether a cancel has been requested for this run (the spawn polls this).
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Record the spawned child's pid (for the best-effort tree kill).
    pub fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::SeqCst);
    }

    /// The recorded child pid, or `None` if the spawn has not started yet.
    pub fn pid(&self) -> Option<u32> {
        match self.pid.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        }
    }
}

/// The append/poll handle handed to an adapter spawn. Cheap to clone (an `Arc`);
/// the spawn reads [`CancelState::is_cancelled`] each poll tick and records the
/// child pid. Dropping it does NOT remove the registry entry (a later
/// [`RunCancellations::request`] can still find the run until
/// [`RunCancellations::finish`]).
#[derive(Clone, Debug)]
pub struct CancelToken {
    state: Arc<CancelState>,
}

impl CancelToken {
    /// Whether a cancel has been requested for this run.
    pub fn is_cancelled(&self) -> bool {
        self.state.is_cancelled()
    }

    /// Record the spawned child's pid for the best-effort tree kill.
    pub fn set_pid(&self, pid: u32) {
        self.state.set_pid(pid);
    }
}

/// The honest result of a cancel request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// A live, cancellable run was found and the cancel flag was set this call.
    Requested,
    /// The run already had a cancel requested (idempotent repeat).
    AlreadyRequested,
    /// No live cancellable run with that id — it never streamed off-lock, already
    /// finished, or runs on the synchronous lock-holding path. NOT cancellable.
    NotRunning,
}

impl CancelOutcome {
    /// A stable wire/UI string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::AlreadyRequested => "already_requested",
            Self::NotRunning => "not_running",
        }
    }

    /// Whether the run is (or was already) being cancelled — true for both the
    /// fresh request and the idempotent repeat, false only for [`Self::NotRunning`].
    pub fn is_cancelling(&self) -> bool {
        matches!(self, Self::Requested | Self::AlreadyRequested)
    }
}

/// A process-global, cloneable registry of cancel tokens for in-flight process
/// runs, keyed by run id. Cheap to clone (an `Arc`); held on the server `AppState`
/// and shared with the off-lock spawn workers. All access is guarded by its own
/// mutex, INDEPENDENT of the kernel store lock, so a cancel request never blocks on
/// (or is blocked by) a kernel operation.
#[derive(Clone, Default)]
pub struct RunCancellations {
    inner: Arc<Mutex<RunCancellationsInner>>,
}

#[derive(Default)]
struct RunCancellationsInner {
    /// run id → its shared cancel state.
    runs: HashMap<String, Arc<CancelState>>,
    /// Insertion order of the run ids, so the backstop can evict the oldest.
    order: Vec<String>,
}

impl RunCancellations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a cancel token for `run_id` and return the [`CancelToken`] the spawn
    /// polls. Creating the entry up front means a cancel that arrives the instant
    /// the process starts already finds the token. Re-beginning an existing run id
    /// replaces its token (a fresh run/retry id is distinct, so this only matters
    /// defensively).
    pub fn begin(&self, run_id: &RunId) -> CancelToken {
        let state = Arc::new(CancelState::new());
        let key = run_id.as_str().to_string();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.runs.insert(key.clone(), state.clone()).is_none() {
            inner.order.push(key);
        }
        // Backstop: evict the oldest stale entries beyond the cap.
        while inner.order.len() > MAX_LIVE_CANCELS {
            let oldest = inner.order.remove(0);
            inner.runs.remove(&oldest);
        }
        CancelToken { state }
    }

    /// Request cancellation of `run_id`. Sets the cancel flag the spawn polls and
    /// reports the honest outcome: [`CancelOutcome::NotRunning`] when no live token
    /// exists (unknown/finished/synchronous run), [`CancelOutcome::AlreadyRequested`]
    /// when one was already set, else [`CancelOutcome::Requested`]. Never takes the
    /// kernel lock, so it is unblocked while the run streams.
    pub fn request(&self, run_id: &RunId) -> CancelOutcome {
        let state = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            match inner.runs.get(run_id.as_str()) {
                Some(s) => s.clone(),
                None => return CancelOutcome::NotRunning,
            }
        };
        // `swap` makes the idempotency check atomic: the first request sees `false`
        // and flips it; any repeat sees `true`.
        if state.cancelled.swap(true, Ordering::SeqCst) {
            CancelOutcome::AlreadyRequested
        } else {
            CancelOutcome::Requested
        }
    }

    /// Whether `run_id` currently has a live cancel token (i.e. it is an in-flight
    /// off-lock run that CAN be cancelled). Read-only.
    pub fn is_cancellable(&self, run_id: &RunId) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .runs
            .contains_key(run_id.as_str())
    }

    /// Drop the cancel token for `run_id` — called once its run has been finalized
    /// (cancelled or not), so subsequent requests honestly report `NotRunning` and
    /// the token's memory is freed.
    pub fn finish(&self, run_id: &RunId) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.runs.remove(run_id.as_str()).is_some() {
            inner.order.retain(|k| k != run_id.as_str());
        }
    }

    /// The number of live cancel tokens currently held (for tests / introspection).
    pub fn live_count(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).runs.len()
    }

    /// The ids of the runs that currently hold a live cancel token — i.e. in-flight
    /// off-lock runs with a real process behind them. The run watchdog excludes
    /// these so a genuinely-executing run is never recovered as stale.
    pub fn active_run_ids(&self) -> Vec<RunId> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .runs
            .keys()
            .map(|k| RunId::new(k.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(s: &str) -> RunId {
        RunId::new(s)
    }

    #[test]
    fn request_on_a_live_run_sets_the_flag() {
        let cancels = RunCancellations::new();
        let id = rid("run_cancel_1");
        let token = cancels.begin(&id);
        assert!(!token.is_cancelled(), "fresh token is not cancelled");
        assert_eq!(cancels.request(&id), CancelOutcome::Requested);
        assert!(token.is_cancelled(), "the spawn now sees the cancel flag");
    }

    #[test]
    fn request_is_idempotent() {
        let cancels = RunCancellations::new();
        let id = rid("run_cancel_2");
        let _token = cancels.begin(&id);
        assert_eq!(cancels.request(&id), CancelOutcome::Requested);
        assert_eq!(cancels.request(&id), CancelOutcome::AlreadyRequested);
        assert_eq!(cancels.request(&id), CancelOutcome::AlreadyRequested);
    }

    #[test]
    fn request_on_an_unknown_or_finished_run_is_not_running() {
        let cancels = RunCancellations::new();
        let id = rid("run_cancel_3");
        // Never began → not running.
        assert_eq!(cancels.request(&id), CancelOutcome::NotRunning);
        assert!(!cancels.is_cancellable(&id));
        // Began then finished → not running again.
        let _token = cancels.begin(&id);
        assert!(cancels.is_cancellable(&id));
        cancels.finish(&id);
        assert!(!cancels.is_cancellable(&id));
        assert_eq!(cancels.request(&id), CancelOutcome::NotRunning);
    }

    #[test]
    fn finish_drops_the_token() {
        let cancels = RunCancellations::new();
        let id = rid("run_cancel_4");
        let _token = cancels.begin(&id);
        assert_eq!(cancels.live_count(), 1);
        cancels.finish(&id);
        assert_eq!(cancels.live_count(), 0);
    }

    #[test]
    fn pid_round_trips_through_the_token() {
        let cancels = RunCancellations::new();
        let id = rid("run_cancel_pid");
        let token = cancels.begin(&id);
        assert_eq!(token.state.pid(), None);
        token.set_pid(4242);
        assert_eq!(token.state.pid(), Some(4242));
    }

    #[test]
    fn registry_is_bounded_by_the_backstop() {
        let cancels = RunCancellations::new();
        for i in 0..(MAX_LIVE_CANCELS + 10) {
            let _ = cancels.begin(&rid(&format!("run_{i}")));
        }
        assert_eq!(cancels.live_count(), MAX_LIVE_CANCELS, "registry must stay bounded");
        // The oldest were evicted; the newest remain cancellable.
        assert!(!cancels.is_cancellable(&rid("run_0")));
        assert!(cancels.is_cancellable(&rid(&format!("run_{}", MAX_LIVE_CANCELS + 9))));
    }

    #[test]
    fn outcome_wire_strings_and_is_cancelling() {
        assert_eq!(CancelOutcome::Requested.as_str(), "requested");
        assert_eq!(CancelOutcome::AlreadyRequested.as_str(), "already_requested");
        assert_eq!(CancelOutcome::NotRunning.as_str(), "not_running");
        assert!(CancelOutcome::Requested.is_cancelling());
        assert!(CancelOutcome::AlreadyRequested.is_cancelling());
        assert!(!CancelOutcome::NotRunning.is_cancelling());
    }
}
