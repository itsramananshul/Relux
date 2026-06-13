//! Run watchdog configuration — the production stall-recovery policy.
//!
//! A Relux run is created in [`crate::RunStatus::Running`] the instant `start_run`
//! commits it, and a real execution (the deterministic local path, or an off-lock
//! CLI spawn) is expected to drive it to a terminal state. Several things can break
//! that expectation without any error reaching the operator:
//!
//! - a caller that starts a run but never executes it (a dangling `start_run`);
//! - an off-lock adapter spawn whose process is killed (or the whole kernel is
//!   restarted) before `finalize_cli_run` records the outcome — an **orphaned**
//!   run that is `Running` in the durable store but backed by no live process;
//! - a background orchestration job that dies mid-flight.
//!
//! In every case the run sits in `Running` forever, the transcript stops after
//! `run_started`, and the dashboard shows the honest client-side "No activity for
//! Xs" cue — but nothing ever recovers it. The watchdog is the server-side
//! backstop that closes this gap: a periodic sweep marks a run that has shown **no
//! wall-clock transcript activity** for longer than [`RunWatchdogConfig::stale_after_secs`]
//! as a recoverable stale failure, with a transcript event and the usual recovery
//! actions (retry / cancel / investigate).
//!
//! This mirrors how the reference agents refuse to hang: Hermes caps a provider
//! read with a hard wall-clock timeout (`reference/hermes-agent-main/agent/anthropic_adapter.py`
//! `_read_timeout`, default 900s) and the openclaw/codex runtime carries a
//! `stream_idle_timeout_ms` inactivity abort (`reference/openclaw-main/extensions/acpx/src/codex-trust-config.ts`).
//! Relux maps the same "no progress ⇒ stop, never hang" idea onto its own run
//! lifecycle: an inactivity threshold measured against real wall-clock, applied
//! only to runs the kernel is *not* actively streaming.

use serde::{Deserialize, Serialize};

/// The smallest stall window the watchdog will honor, in seconds. A window
/// shorter than this risks flagging a slow-but-live local step as stale, so the
/// config is clamped up to this floor. Not a toy cap — a safety floor for a
/// user-tunable value.
pub const MIN_STALE_AFTER_SECS: u64 = 30;

/// The largest stall window the watchdog will honor, in seconds (6 hours). Beyond
/// this a "stuck" run is effectively never recovered, defeating the watchdog, so
/// the config is clamped down to this ceiling.
pub const MAX_STALE_AFTER_SECS: u64 = 21_600;

/// The default stall window, in seconds. Three minutes is long enough that no
/// genuinely-active run (those are excluded from the sweep anyway — see the kernel
/// sweep) is ever flagged, and short enough that an orphaned/dangling run is
/// surfaced quickly instead of hanging indefinitely.
pub const DEFAULT_STALE_AFTER_SECS: u64 = 180;

/// The operator-tunable run-watchdog policy. Persisted in the kernel snapshot and
/// exposed/editable over the HTTP control plane (`GET`/`PUT /v1/relux/watchdog`),
/// so the recovery behavior is always visible and never a hidden constant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunWatchdogConfig {
    /// Whether the periodic stale-run sweep runs at all. On by default — a hung
    /// run with no recovery path is exactly the failure this slice removes.
    pub enabled: bool,
    /// How long a run may stay `Running` with no new transcript activity (measured
    /// against real wall-clock) before the watchdog recovers it as stale. Clamped
    /// to `[MIN_STALE_AFTER_SECS, MAX_STALE_AFTER_SECS]`.
    pub stale_after_secs: u64,
}

impl Default for RunWatchdogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stale_after_secs: DEFAULT_STALE_AFTER_SECS,
        }
    }
}

impl RunWatchdogConfig {
    /// Return a copy with `stale_after_secs` clamped into the honored range. Always
    /// applied on load and on update so a persisted or operator-supplied value can
    /// never disable the watchdog by being absurdly large or trip on a value that
    /// is dangerously small.
    pub fn clamped(&self) -> Self {
        Self {
            enabled: self.enabled,
            stale_after_secs: self
                .stale_after_secs
                .clamp(MIN_STALE_AFTER_SECS, MAX_STALE_AFTER_SECS),
        }
    }

    /// Whether a run with `last_activity_at` (real wall-clock secs of its most
    /// recent transcript event) is stale as of `now_secs`. Pure: the kernel sweep
    /// supplies the clock and the live-run exclusion; this only decides the
    /// threshold. A run with no recorded activity timestamp is treated as stale
    /// once the window has elapsed since the epoch baseline the caller passes in.
    pub fn is_stale(&self, last_activity_at: u64, now_secs: u64) -> bool {
        now_secs.saturating_sub(last_activity_at) >= self.stale_after_secs.max(MIN_STALE_AFTER_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_with_three_minute_window() {
        let c = RunWatchdogConfig::default();
        assert!(c.enabled);
        assert_eq!(c.stale_after_secs, DEFAULT_STALE_AFTER_SECS);
    }

    #[test]
    fn clamp_pulls_an_out_of_range_window_into_the_honored_band() {
        let too_small = RunWatchdogConfig {
            enabled: true,
            stale_after_secs: 1,
        }
        .clamped();
        assert_eq!(too_small.stale_after_secs, MIN_STALE_AFTER_SECS);

        let too_big = RunWatchdogConfig {
            enabled: true,
            stale_after_secs: u64::MAX,
        }
        .clamped();
        assert_eq!(too_big.stale_after_secs, MAX_STALE_AFTER_SECS);
    }

    #[test]
    fn is_stale_only_after_the_window_elapses() {
        let c = RunWatchdogConfig {
            enabled: true,
            stale_after_secs: 100,
        };
        assert!(!c.is_stale(1_000, 1_050), "50s < 100s window: not stale");
        assert!(c.is_stale(1_000, 1_100), "100s == window: stale");
        assert!(c.is_stale(1_000, 5_000), "well past the window: stale");
    }
}
