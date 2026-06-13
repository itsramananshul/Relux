//! Structured run-failure classification + bounded transient-retry policy.
//!
//! Spec ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §7 (Error handling / recovery)
//! and `docs/reference-driven-development.md` (the per-slice reference read for
//! this module). The audit's P1 gap was: *"Relux retry is a fresh run with no
//! error taxonomy and no backoff; Paperclip classifies (`run-liveness.ts`) and
//! retries transient upstream failures on a bounded `[2m,10m,30m,2h]` schedule."*
//!
//! This module is the pure half of closing that gap: a deterministic taxonomy of
//! why a run failed, the safe public remediation for each class, and the bounded
//! backoff schedule. The kernel ([`crate::run::Run`] carries the result; the
//! kernel stamps a [`RunRetryState`] at fail time and re-attempts only the safe
//! classes through the unchanged governed run path) is the impure half.
//!
//! ## Reference grounding
//!
//! - **Hermes** `reference/hermes-agent-main/agent/error_classifier.py`
//!   (`FailoverReason` enum + `classify_api_error`): a priority-ordered classifier
//!   that maps an API failure to a reason and to recovery-action hints
//!   (`retryable` / `should_fallback` / …). Auth/billing/format are NOT retryable;
//!   rate-limit / overloaded / server-error / timeout / unknown are. We mirror the
//!   *shape* — a closed enum + a priority-ordered, pattern-driven classifier whose
//!   result carries a `retryable` decision — but over Relux's own failure surface
//!   (adapter spawn / exit / timeout / envelope-error / governance) rather than raw
//!   HTTP status codes, and we are STRICTER about what auto-retries (see below).
//! - **Paperclip** `references/paperclip/server/src/services/run-liveness.ts`
//!   (`classifyRunActionability` / `classifyRunLiveness`): evidence/regex-based
//!   classification into `runnable` / `approval_required` / `blocked_external` /
//!   `manager_review`, surfaced with a human reason. We adopt the "classify into a
//!   small closed set + a human reason, and decide auto-continue ONLY for the safe
//!   class" pattern.
//! - **Paperclip** `references/paperclip/server/src/services/heartbeat.ts`
//!   (`BOUNDED_TRANSIENT_HEARTBEAT_RETRY_DELAYS_MS = [2m,10m,30m,2h]`,
//!   `computeBoundedTransientHeartbeatRetrySchedule`): only the
//!   `transient_upstream` error family retries, on that exact bounded schedule,
//!   capped at the schedule length. We reuse the schedule verbatim
//!   ([`RETRY_BACKOFF_SECS`]) and the "only the transient family retries, bounded
//!   by the schedule length" rule ([`RunRetryState::plan`]).
//!
//! ## What we deliberately do differently (the safety shape)
//!
//! Relux runs are not idempotent the way a stateless API call is — a coding-agent
//! run can mutate a workspace. So unlike Hermes (which auto-retries the `unknown`
//! bucket) we auto-retry ONLY the two classes that are unambiguously safe and
//! upstream-caused: [`RunFailureClass::TransientProvider`] and
//! [`RunFailureClass::Timeout`]. Every other class — auth, missing adapter,
//! permission, invalid prompt, output validation, cancelled, and the catch-all
//! unknown — is NON-retryable here: it surfaces a remediation and waits for an
//! operator (who can still trigger a manual retry through the existing
//! `prime.retry_run` path). There is no background scheduler: the retry becomes
//! *eligible* at a real wall-clock instant and is consumed manually or on the next
//! autonomy tick — an honest "retry-ready" state, never a faked timer.

use serde::{Deserialize, Serialize};

use crate::redact::redact_secrets;

/// The bounded transient-retry backoff, in seconds: `[2m, 10m, 30m, 2h]`.
///
/// Lifted verbatim from Paperclip's `BOUNDED_TRANSIENT_HEARTBEAT_RETRY_DELAYS_MS`
/// (`heartbeat.ts`). Attempt `N` (0-based) waits `RETRY_BACKOFF_SECS[N]`; once `N`
/// reaches the array length the transient budget is exhausted and no further
/// automatic retry is scheduled. We omit Paperclip's ±25% jitter on purpose: with
/// no background scheduler the delay is only a lower bound on eligibility (the
/// retry fires on the next manual/autonomy consumption at or after `not_before`),
/// so jitter would add non-determinism without the thundering-herd protection it
/// exists to provide.
pub const RETRY_BACKOFF_SECS: [u64; 4] = [
    2 * 60,      // 2 minutes
    10 * 60,     // 10 minutes
    30 * 60,     // 30 minutes
    2 * 60 * 60, // 2 hours
];

/// The maximum number of automatic transient retries (the schedule length).
pub const MAX_TRANSIENT_RETRIES: u32 = RETRY_BACKOFF_SECS.len() as u32;

/// The cap on the secret-free public failure message length, so a verbose
/// provider envelope can never balloon a run record. Mirrors Hermes'
/// `_sanitize_tool_error` 2000-char clamp and `_extract_message`'s 500-char clamp.
pub const MAX_PUBLIC_MESSAGE_CHARS: usize = 500;

/// A structured classification of why a run failed.
///
/// The names are aligned with the existing kernel error surface
/// (`KernelError::AdapterBinaryMissing` → [`Self::AdapterMissing`],
/// `KernelError::PermissionDenied` → [`Self::PermissionDenied`], an adapter
/// wall-clock timeout → [`Self::Timeout`], …) and with Hermes' `FailoverReason`
/// where they overlap (`auth` → [`Self::AuthRequired`], `rate_limit`/`overloaded`/
/// `server_error` → [`Self::TransientProvider`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureClass {
    /// An upstream/provider transient: rate limit, overload, 5xx, dropped
    /// connection. Safe to retry on the bounded backoff (the cause is not in the
    /// request). Maps to Hermes `rate_limit`/`overloaded`/`server_error`.
    TransientProvider,
    /// The adapter/provider rejected the credentials (401/403, invalid/expired
    /// key, "please sign in"). NOT auto-retried — the same key will fail again;
    /// an operator must re-authenticate. Maps to Hermes `auth`.
    AuthRequired,
    /// The adapter binary is not on PATH, has no command configured, or its
    /// runtime is disabled/unconfigured. NOT auto-retried — needs operator setup.
    AdapterMissing,
    /// Relux governance denied the action (the agent lacks a required permission).
    /// NOT auto-retried — needs an operator grant or approval.
    PermissionDenied,
    /// The request itself was rejected as malformed/invalid (a 400-class bad
    /// request, an invalid prompt). NOT auto-retried — retrying the same input
    /// fails the same way. Maps to Hermes `format_error`.
    InvalidPrompt,
    /// The adapter run exceeded its wall-clock timeout. Safe to retry on the
    /// bounded backoff (often a transient slow upstream). Maps to Hermes `timeout`.
    Timeout,
    /// The run was intentionally cancelled. Terminal and intentional — never
    /// retried, and not an operator-action signal.
    Cancelled,
    /// The adapter produced output we could not validate as a successful result
    /// (an `is_error` envelope with a non-transient cause, an unparseable
    /// result). NOT auto-retried — surfaces for operator review.
    OutputValidation,
    /// Unclassifiable. Because a Relux run can mutate a workspace, an unknown
    /// failure is NOT auto-retried (unlike Hermes, which retries its `unknown`
    /// bucket) — it surfaces for operator review and a deliberate manual retry.
    Unknown,
    /// The run watchdog recovered a run that had been `Running` with no transcript
    /// activity for longer than the configured stall window, and was not backed by
    /// any live (streaming/cancellable) process — a dangling start, an orphaned
    /// off-lock spawn, or a restart casualty. NOT auto-retried: a stall is
    /// unexpected, so it surfaces for an explicit operator choice (retry / cancel /
    /// investigate) rather than silently looping. See [`crate::RunWatchdogConfig`].
    Stale,
}

impl RunFailureClass {
    /// The stable wire/UI string (matches the serde `snake_case` rendering).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TransientProvider => "transient_provider",
            Self::AuthRequired => "auth_required",
            Self::AdapterMissing => "adapter_missing",
            Self::PermissionDenied => "permission_denied",
            Self::InvalidPrompt => "invalid_prompt",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::OutputValidation => "output_validation",
            Self::Unknown => "unknown",
            Self::Stale => "stale",
        }
    }

    /// A short human label for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::TransientProvider => "Transient provider error",
            Self::AuthRequired => "Authentication required",
            Self::AdapterMissing => "Adapter not available",
            Self::PermissionDenied => "Permission denied",
            Self::InvalidPrompt => "Invalid request",
            Self::Timeout => "Timed out",
            Self::Cancelled => "Cancelled",
            Self::OutputValidation => "Output validation failed",
            Self::Unknown => "Unknown failure",
            Self::Stale => "Stalled (no activity)",
        }
    }

    /// Whether the kernel may AUTOMATICALLY retry this class on the bounded
    /// backoff. Only the two unambiguously-safe, upstream-caused classes qualify;
    /// every other class waits for an operator (who can still retry manually).
    pub fn retryable(&self) -> bool {
        matches!(self, Self::TransientProvider | Self::Timeout)
    }

    /// Whether this failure needs an operator to act before the run can succeed
    /// (fix credentials, enable an adapter, grant a permission, fix the request,
    /// or review). Auto-retryable transients and an intentional cancel do not.
    pub fn needs_operator_action(&self) -> bool {
        matches!(
            self,
            Self::AuthRequired
                | Self::AdapterMissing
                | Self::PermissionDenied
                | Self::InvalidPrompt
                | Self::OutputValidation
                | Self::Unknown
                | Self::Stale
        )
    }

    /// A safe, secret-free remediation line for the operator. Static text only —
    /// never echoes the provider envelope.
    pub fn remediation(&self) -> &'static str {
        match self {
            Self::TransientProvider => {
                "Transient upstream error. Relux will re-attempt on a bounded backoff; \
                 no action needed unless it keeps recurring."
            }
            Self::AuthRequired => {
                "Re-authenticate the adapter's CLI (sign in again so a valid credential \
                 is on PATH), then retry."
            }
            Self::AdapterMissing => {
                "Install/enable the adapter's CLI on PATH and enable its runtime on \
                 Crew → Adapters, then retry."
            }
            Self::PermissionDenied => {
                "Grant the assigned agent the required permission (or approve the action), \
                 then retry."
            }
            Self::InvalidPrompt => {
                "The request was rejected as invalid. Adjust the task input and start a \
                 fresh run."
            }
            Self::Timeout => {
                "The run exceeded its time limit. Relux will re-attempt on a bounded \
                 backoff; raise the adapter timeout if it keeps timing out."
            }
            Self::Cancelled => "This run was cancelled. Start a fresh run if it is still wanted.",
            Self::OutputValidation => {
                "The adapter returned an error result. Review the run output, fix the \
                 cause, and start a fresh run."
            }
            Self::Unknown => {
                "The failure could not be classified. Review the run output and retry \
                 manually if it looks transient."
            }
            Self::Stale => {
                "This run stopped making progress and was automatically recovered by the \
                 run watchdog — no transcript activity for the configured stall window, \
                 and no live process behind it. Retry it, cancel it, or investigate with \
                 Prime. Raise the watchdog window on the Health page if a legitimately \
                 long, quiet run is being recovered too early."
            }
        }
    }
}

/// Classify a failure from its (kernel-authored) reason string and a
/// `timed_out` signal. Pure and priority-ordered — the deterministic rail the
/// kernel falls back to when it has no more specific signal.
///
/// The priority order is deliberate (most specific / safest-to-decide first):
/// timeout → cancelled → permission → auth → adapter-missing → transient → invalid
/// → output-validation → unknown. The reason string is matched case-insensitively
/// against high-signal substrings drawn from Hermes' pattern lists and Relux's own
/// kernel error messages.
pub fn classify_failure(reason: &str, timed_out: bool) -> RunFailureClass {
    let r = reason.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| r.contains(n));

    // 1. Timeout — the explicit signal wins, plus deadline/timeout wording.
    if timed_out || has(&["timed out", "timeout", "deadline exceeded", "deadline_exceeded"]) {
        return RunFailureClass::Timeout;
    }

    // 2. Cancelled — an intentional terminal stop.
    if has(&["cancelled", "canceled", "aborted by", "user abort"]) {
        return RunFailureClass::Cancelled;
    }

    // 3. Permission — Relux governance denial (kernel: "permission denied: agent X
    //    lacks Y").
    if has(&["permission denied", "lacks ", "not permitted", "permissiondenied"]) {
        return RunFailureClass::PermissionDenied;
    }

    // 4. Auth — provider rejected the credential. Hermes `_AUTH_PATTERNS` + 401/403.
    if has(&[
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "invalid api key",
        "invalid_api_key",
        "authentication",
        "not authenticated",
        "expired token",
        "token expired",
        "please sign in",
        "please log in",
        "login required",
        "sign in to",
        "not signed in",
    ]) {
        return RunFailureClass::AuthRequired;
    }

    // 5. Adapter missing / misconfigured — kernel adapter-runtime errors.
    if has(&[
        "not found on path",
        "was not found on path",
        "no command configured",
        "not configured",
        "runtime is configured but disabled",
        "runtime for",
        "enable it first",
        "no adapter runtime",
        "failed to spawn adapter",
    ]) {
        return RunFailureClass::AdapterMissing;
    }

    // 6. Transient provider — Hermes rate-limit / overloaded / server-error /
    //    transport patterns. Safe to auto-retry.
    if has(&[
        "rate limit",
        "rate_limit",
        "too many requests",
        "throttl",
        "overloaded",
        "temporarily unavailable",
        "service unavailable",
        "try again",
        "try later",
        "503",
        "529",
        "502",
        "500",
        "internal server error",
        "server error",
        "bad gateway",
        "connection reset",
        "connection aborted",
        "connection closed",
        "econnreset",
        "upstream",
        "transient",
    ]) {
        return RunFailureClass::TransientProvider;
    }

    // 7. Invalid request — Hermes `format_error` (400-class).
    if has(&[
        "400",
        "bad request",
        "invalid request",
        "invalid_request",
        "malformed",
        "invalid prompt",
        "unprocessable",
        "422",
    ]) {
        return RunFailureClass::InvalidPrompt;
    }

    // 8. Output validation — an adapter error result / unparseable output.
    if has(&[
        "reported an error",
        "could not parse",
        "invalid output",
        "validation failed",
        "unexpected output",
        "malformed response",
    ]) {
        return RunFailureClass::OutputValidation;
    }

    // 9. Catch-all.
    RunFailureClass::Unknown
}

/// The backoff for transient attempt `attempt` (0-based): `Some(seconds)` while
/// within the bounded schedule, `None` once the budget is exhausted.
pub fn retry_delay_secs(attempt: u32) -> Option<u64> {
    RETRY_BACKOFF_SECS.get(attempt as usize).copied()
}

/// The bounded-retry state stamped on a failed run.
///
/// This is the honest data model for "retry-ready" in the absence of a background
/// scheduler: a transient failure records WHICH attempt it is, the cap, and the
/// earliest real-wall-clock instant a retry may run (`not_before_secs`). A retry
/// is consumed manually (`prime.retry_run`) or on the next autonomy tick that
/// finds `not_before_secs <= now`. When the bounded budget is spent, `exhausted`
/// is set and `not_before_secs` is `None` (no further automatic retry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRetryState {
    /// Which transient retry attempt this run is (0 = the first attempt; 1 = the
    /// first retry, …). Derived from the `retried_from` lineage length.
    pub attempt: u32,
    /// The maximum number of automatic transient retries ([`MAX_TRANSIENT_RETRIES`]).
    pub max_attempts: u32,
    /// The earliest real unix-second instant at which a transient retry becomes
    /// eligible. `None` when the bounded budget is exhausted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before_secs: Option<u64>,
    /// True when no further automatic transient retry is allowed (budget spent).
    pub exhausted: bool,
}

impl RunRetryState {
    /// Plan the retry state for a freshly-failed run.
    ///
    /// Returns `None` for any non-[`RunFailureClass::retryable`] class — those
    /// failures carry only a `failure_class` and wait for an operator. For a
    /// retryable class, schedules the next attempt at `now_secs + backoff[attempt]`
    /// while the budget remains, or marks the state exhausted once `attempt`
    /// reaches [`MAX_TRANSIENT_RETRIES`].
    pub fn plan(class: RunFailureClass, attempt: u32, now_secs: u64) -> Option<Self> {
        if !class.retryable() {
            return None;
        }
        match retry_delay_secs(attempt) {
            Some(delay) => Some(Self {
                attempt,
                max_attempts: MAX_TRANSIENT_RETRIES,
                not_before_secs: Some(now_secs.saturating_add(delay)),
                exhausted: false,
            }),
            None => Some(Self {
                attempt,
                max_attempts: MAX_TRANSIENT_RETRIES,
                not_before_secs: None,
                exhausted: true,
            }),
        }
    }

    /// Whether a retry is eligible to run at `now_secs` (scheduled, not exhausted,
    /// and the not-before instant has passed).
    pub fn eligible_at(&self, now_secs: u64) -> bool {
        !self.exhausted
            && self
                .not_before_secs
                .map(|nb| now_secs >= nb)
                .unwrap_or(false)
    }
}

/// A safe, secret-free, length-clamped public message for a failure.
///
/// Redacts known secret shapes (via [`redact_secrets`]), collapses to a single
/// line, and clamps to [`MAX_PUBLIC_MESSAGE_CHARS`]. The raw reason may carry a
/// redacted provider envelope; this is what a non-privileged surface should show.
pub fn safe_public_message(reason: &str) -> String {
    let redacted = redact_secrets(reason);
    let single_line = redacted
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if single_line.chars().count() <= MAX_PUBLIC_MESSAGE_CHARS {
        single_line
    } else {
        let clamped: String = single_line.chars().take(MAX_PUBLIC_MESSAGE_CHARS - 1).collect();
        format!("{clamped}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_signal_and_wording_classify_as_timeout() {
        assert_eq!(
            classify_failure("adapter 'claude' exited with code 1", true),
            RunFailureClass::Timeout
        );
        assert_eq!(
            classify_failure("adapter 'claude' timed out after 120s", false),
            RunFailureClass::Timeout
        );
        assert_eq!(
            classify_failure("deadline exceeded talking to upstream", false),
            RunFailureClass::Timeout
        );
    }

    #[test]
    fn transient_provider_patterns() {
        for reason in [
            "hit a rate limit",
            "Error 503: service unavailable",
            "provider overloaded, please try again",
            "connection reset by peer",
            "HTTP 529 from upstream",
            "internal server error",
        ] {
            assert_eq!(
                classify_failure(reason, false),
                RunFailureClass::TransientProvider,
                "reason did not classify transient: {reason}"
            );
        }
    }

    #[test]
    fn auth_patterns_are_not_retryable() {
        for reason in [
            "401 Unauthorized",
            "invalid api key",
            "authentication failed",
            "please sign in to the CLI",
        ] {
            let c = classify_failure(reason, false);
            assert_eq!(c, RunFailureClass::AuthRequired, "reason: {reason}");
            assert!(!c.retryable(), "auth must never auto-retry: {reason}");
            assert!(c.needs_operator_action());
        }
    }

    #[test]
    fn adapter_missing_and_permission_and_invalid_are_not_retryable() {
        assert_eq!(
            classify_failure("adapter claude binary 'claude' was not found on PATH", false),
            RunFailureClass::AdapterMissing
        );
        assert_eq!(
            classify_failure(
                "no adapter runtime configured for relux-adapter-claude-cli; enable it first",
                false
            ),
            RunFailureClass::AdapterMissing
        );
        assert_eq!(
            classify_failure("permission denied: agent prime lacks plugin:install", false),
            RunFailureClass::PermissionDenied
        );
        assert_eq!(
            classify_failure("400 bad request: invalid prompt", false),
            RunFailureClass::InvalidPrompt
        );
        for c in [
            RunFailureClass::AdapterMissing,
            RunFailureClass::PermissionDenied,
            RunFailureClass::InvalidPrompt,
        ] {
            assert!(!c.retryable(), "{c:?} must not auto-retry");
            assert!(c.needs_operator_action(), "{c:?} needs operator action");
        }
    }

    #[test]
    fn cancelled_is_terminal_not_operator_action() {
        let c = classify_failure("run cancelled by operator", false);
        assert_eq!(c, RunFailureClass::Cancelled);
        assert!(!c.retryable());
        assert!(!c.needs_operator_action());
    }

    #[test]
    fn envelope_error_is_output_validation() {
        // The CLI envelope `is_error` path passes the model text; a non-transient
        // error result classifies as output validation (operator review).
        assert_eq!(
            classify_failure("adapter 'claude' reported an error: could not finish", false),
            RunFailureClass::OutputValidation
        );
        // …but a transient cause in the same envelope text still classifies
        // transient (the transient patterns are checked before the envelope one).
        assert_eq!(
            classify_failure("adapter 'claude' reported an error: rate limit reached", false),
            RunFailureClass::TransientProvider
        );
    }

    #[test]
    fn unknown_is_the_catch_all_and_is_not_auto_retried() {
        let c = classify_failure("adapter 'claude' exited with code 3", false);
        assert_eq!(c, RunFailureClass::Unknown);
        assert!(!c.retryable(), "unknown must NOT auto-retry (runs can mutate)");
        assert!(c.needs_operator_action());
    }

    #[test]
    fn retry_schedule_is_the_bounded_2m_10m_30m_2h() {
        assert_eq!(retry_delay_secs(0), Some(120));
        assert_eq!(retry_delay_secs(1), Some(600));
        assert_eq!(retry_delay_secs(2), Some(1800));
        assert_eq!(retry_delay_secs(3), Some(7200));
        assert_eq!(retry_delay_secs(4), None, "budget exhausted past the schedule");
        assert_eq!(MAX_TRANSIENT_RETRIES, 4);
    }

    #[test]
    fn plan_schedules_only_retryable_classes() {
        // A non-retryable class never gets a retry block.
        assert!(RunRetryState::plan(RunFailureClass::AuthRequired, 0, 1000).is_none());
        assert!(RunRetryState::plan(RunFailureClass::Unknown, 0, 1000).is_none());
        assert!(RunRetryState::plan(RunFailureClass::Cancelled, 0, 1000).is_none());

        // A transient class schedules at now + backoff[attempt].
        let s0 = RunRetryState::plan(RunFailureClass::TransientProvider, 0, 1000).unwrap();
        assert_eq!(s0.attempt, 0);
        assert_eq!(s0.not_before_secs, Some(1120));
        assert!(!s0.exhausted);

        let s1 = RunRetryState::plan(RunFailureClass::Timeout, 1, 1000).unwrap();
        assert_eq!(s1.not_before_secs, Some(1600));

        // Past the budget → exhausted, no instant.
        let s4 = RunRetryState::plan(RunFailureClass::TransientProvider, 4, 1000).unwrap();
        assert!(s4.exhausted);
        assert_eq!(s4.not_before_secs, None);
    }

    #[test]
    fn eligibility_honors_the_not_before_instant() {
        let s = RunRetryState::plan(RunFailureClass::TransientProvider, 0, 1000).unwrap();
        assert!(!s.eligible_at(1119), "before not_before → not eligible");
        assert!(s.eligible_at(1120), "at not_before → eligible");
        assert!(s.eligible_at(5000), "after not_before → eligible");

        let exhausted = RunRetryState::plan(RunFailureClass::TransientProvider, 9, 1000).unwrap();
        assert!(!exhausted.eligible_at(u64::MAX), "exhausted is never eligible");
    }

    #[test]
    fn public_message_redacts_secrets_and_clamps() {
        let secret = "failed: Authorization: Bearer sk-ant-api03-SECRETKEYVALUE1234567890 leaked";
        let msg = safe_public_message(secret);
        assert!(!msg.contains("SECRETKEYVALUE"), "secret leaked: {msg}");
        assert!(msg.contains("REDACTED"));

        let long = "x ".repeat(2000);
        let clamped = safe_public_message(&long);
        assert!(clamped.chars().count() <= MAX_PUBLIC_MESSAGE_CHARS);
    }

    #[test]
    fn class_wire_strings_round_trip() {
        for c in [
            RunFailureClass::TransientProvider,
            RunFailureClass::AuthRequired,
            RunFailureClass::AdapterMissing,
            RunFailureClass::PermissionDenied,
            RunFailureClass::InvalidPrompt,
            RunFailureClass::Timeout,
            RunFailureClass::Cancelled,
            RunFailureClass::OutputValidation,
            RunFailureClass::Unknown,
        ] {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.as_str()));
            let back: RunFailureClass = serde_json::from_str(&json).unwrap();
            assert_eq!(back, c);
        }
    }
}
