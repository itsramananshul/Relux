//! Multi-turn clarification memory — let a follow-up answer resolve the clarifying
//! question Prime asked last turn, instead of being read as a fresh, context-free
//! message (`docs/prime-processing-audit.md` "Multi-turn clarify memory" — the last
//! recommended slice; `docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §10.5
//! Conversation Rules, §17.1 "Prime must understand conversational intent").
//!
//! ## Why this exists
//!
//! Prime already asks ONE good clarifying question for an ambiguous actionable request
//! ("assign this to the researcher" → "which task?"). But the user's next message
//! ("task_0001") did not carry the prior question's context, so it was classified from
//! scratch as a bare `DirectAnswer` — the original request was lost and Prime felt
//! keyword-shaped, not like Hermes/Codex. This module stores a small, bounded
//! [`relux_core::PendingClarification`] when Prime asks, and on the next turn decides
//! whether the new message *resolves* that pending question.
//!
//! ## The safety shape (binding)
//!
//! This layer NEVER executes anything and NEVER invents authority. It only decides how
//! to interpret the follow-up:
//!
//! - **Continue** — a bare answer is *combined* with the stored original message; the
//!   combined text then flows through the SAME deterministic
//!   `classify_intent` → `decide` → `prime_execute` pipeline the kernel always uses, so
//!   a risky action still becomes an approval-gated `Propose` and an unknown
//!   task/agent still fails closed. The brain authors nothing here.
//! - **FreshRequest** — the follow-up stands on its own as a new command/question
//!   ([`crate::prime::is_standalone_request`]); the pending context is dropped and the
//!   new message is handled normally, so a stale question can never hijack a fresh ask.
//! - **Cancelled** — an explicit "never mind" / "cancel" drops the pending context and
//!   Prime replies naturally.
//! - **Expired** — past its TTL, the record is ignored and dropped.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Paperclip / openclaw** `src/agents/bash-tools.exec-approval-followup-state.ts`
//!   (`registerExecApprovalFollowupRuntimeHandoff` / `consumeExecApprovalFollowupRuntimeHandoff`,
//!   `EXEC_APPROVAL_FOLLOWUP_RUNTIME_HANDOFF_TTL_MS = 5 * 60 * 1000`) — a small *pending
//!   handoff* record is stored keyed by a session/approval id with an explicit
//!   `expiresAtMs`, then *consumed* on a later turn only when it matches and has not
//!   expired, and deleted after use. We mirror that shape exactly: one bounded pending
//!   record per conversation key, with `expires_at_secs`, consumed/cleared on the next
//!   turn (`resolve_pending` here; the kernel stores/clears it).
//! - **Paperclip / openclaw** `src/agents/bash-tools.exec-approval-followup.ts`
//!   (`sendExecApprovalFollowup` → `buildExecApprovalFollowupPrompt`) — when a pending
//!   handoff is consumed, a NEW turn is run in the same session with the stored context
//!   injected into the prompt. We adapt this to "combine the stored original message
//!   with the new answer, then re-run the deterministic turn" — context-injection into a
//!   fresh, fully-validated turn rather than a privileged shortcut.
//! - **Hermes** `agent/conversation_loop.py` (`run_conversation`,
//!   `messages = list(conversation_history)` then append the new user message; lines
//!   ~330-400) — a follow-up turn appends the new user message to the SAME prior history
//!   so the model answers the earlier question in context. We carry only the single
//!   pending question's grounding (not a full transcript), which is the minimal, bounded
//!   slice of that idea that a deterministic kernel needs.

use relux_core::{PendingClarification, PrimeIntent};

/// How long (in logical-clock seconds) a pending clarification stays resolvable before
/// it is treated as stale. Mirrors Paperclip's 5-minute follow-up TTL in spirit; bounded
/// so a question can never silently steer a much later, unrelated message.
pub const CLARIFY_TTL_SECS: u64 = 900;

/// The maximum number of characters kept for the stored (and accumulated) original
/// message, so the record stays small and a long paste cannot bloat the control plane.
pub const MAX_ORIGINAL_CHARS: usize = 480;

/// The maximum number of characters kept for the stored clarifying question.
pub const MAX_QUESTION_CHARS: usize = 240;

/// Explicit cancellation phrases. A follow-up that is essentially just one of these
/// drops the pending clarification and Prime answers naturally. Matched as a substring
/// against the lowercased message, but ONLY after [`crate::prime::is_standalone_request`]
/// has already ruled the message out as a fresh command — so "never mind, create a task
/// to X" still creates the task rather than only cancelling.
const CANCEL_MARKERS: &[&str] = &[
    "never mind",
    "nevermind",
    "nvm",
    "cancel that",
    "cancel it",
    "cancel",
    "forget it",
    "forget that",
    "scratch that",
    "drop it",
    "no thanks",
    "ignore that",
    "ignore it",
];

/// What to do with a follow-up message given a stored pending clarification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClarifyResolution {
    /// The pending record is past its TTL: ignore and drop it; handle the message fresh.
    Expired,
    /// The follow-up is an explicit cancellation: drop the pending context, reply naturally.
    Cancelled,
    /// The follow-up stands on its own as a new request: drop the pending context and
    /// handle the new message normally (it supersedes the old question).
    FreshRequest,
    /// The follow-up reads as a bare answer: combine it with the stored original message
    /// and re-run the deterministic turn on the combined text.
    Continue {
        /// The original message and the answer, concatenated and length-bounded.
        combined: String,
    },
}

/// True when a follow-up is essentially just a cancellation phrase.
///
/// Operates on the trimmed, lowercased message. Callers gate this *after*
/// [`crate::prime::is_standalone_request`] so a message that also carries a fresh command
/// is never swallowed as a cancel.
pub fn is_cancellation(message: &str) -> bool {
    let m = message.trim().to_lowercase();
    if m.is_empty() {
        return false;
    }
    CANCEL_MARKERS.iter().any(|p| m.contains(p))
}

/// Whether a clarifying turn for `intent` is one this memory can later RESOLVE with a
/// follow-up answer. Only the intents whose `decide` arm produces a concrete action when
/// the missing field is supplied are eligible — assignment (needs a task id / agent) and
/// task creation (needs a description). Intents whose clarify cannot yet be resolved into
/// an action by more text (a run start has no by-id action wired; a task update has no
/// `UpdateTask` action) are deliberately NOT recorded, so the memory never sets up a
/// loop that cannot resolve and never fakes an unsupported action
/// (`docs/reference-driven-development.md`: no faked capability).
pub fn is_resolvable_clarify_intent(intent: &PrimeIntent) -> bool {
    matches!(
        intent,
        PrimeIntent::AssignTask | PrimeIntent::TaskCreation | PrimeIntent::CreateAndRunTask
    )
}

/// Combine a stored original message with the follow-up answer into one message the
/// deterministic classifier/extractors can read, length-bounded so accumulated context
/// across several follow-ups can never grow without limit.
pub fn combine(original: &str, answer: &str) -> String {
    let mut combined = String::with_capacity(original.len() + answer.len() + 1);
    combined.push_str(original.trim());
    if !original.trim().is_empty() && !answer.trim().is_empty() {
        combined.push(' ');
    }
    combined.push_str(answer.trim());
    clamp(&combined, MAX_ORIGINAL_CHARS)
}

/// Decide how a follow-up `new_message` relates to a stored pending clarification, given
/// the current logical-clock second `now_secs`.
///
/// Pure and deterministic: no clock read, no network. The order of the gates matters — a
/// fresh standalone command/question wins over a cancellation phrase (so "never mind,
/// create a task to X" creates the task), and both win over a bare-answer continuation.
pub fn resolve_pending(
    pending: &PendingClarification,
    new_message: &str,
    now_secs: u64,
) -> ClarifyResolution {
    if now_secs >= pending.expires_at_secs {
        return ClarifyResolution::Expired;
    }
    // A follow-up that is itself a complete request supersedes the pending question.
    if crate::prime::is_standalone_request(new_message) {
        return ClarifyResolution::FreshRequest;
    }
    // An explicit cancellation drops the pending context (checked after the fresh-request
    // gate so a cancel-plus-command still acts).
    if is_cancellation(new_message) {
        return ClarifyResolution::Cancelled;
    }
    // Otherwise the message reads as a bare answer: continue the original request.
    ClarifyResolution::Continue {
        combined: combine(&pending.original_message, new_message),
    }
}

/// Clamp a string to at most `max` characters, trimming the ends. Shared shape with the
/// other Prime sanitizers; here it only bounds size (the text is plain user input, never
/// a provider envelope).
pub fn clamp(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(intent: PrimeIntent, original: &str, expires: u64) -> PendingClarification {
        PendingClarification {
            original_message: original.to_string(),
            intent,
            needs: "task id".to_string(),
            question: "Which task?".to_string(),
            created_at_secs: 0,
            expires_at_secs: expires,
            source: "deterministic".to_string(),
        }
    }

    #[test]
    fn a_bare_answer_continues_the_original_request() {
        let p = pending(PrimeIntent::AssignTask, "assign this to researcher", 900);
        let r = resolve_pending(&p, "task_0001", 5);
        assert_eq!(
            r,
            ClarifyResolution::Continue {
                combined: "assign this to researcher task_0001".to_string()
            }
        );
    }

    #[test]
    fn an_expired_record_is_ignored() {
        let p = pending(PrimeIntent::AssignTask, "assign this to researcher", 100);
        // now == expiry is already stale (TTL is exclusive of the boundary).
        assert_eq!(resolve_pending(&p, "task_0001", 100), ClarifyResolution::Expired);
        assert_eq!(resolve_pending(&p, "task_0001", 250), ClarifyResolution::Expired);
    }

    #[test]
    fn an_explicit_cancellation_drops_the_context() {
        let p = pending(PrimeIntent::AssignTask, "assign this to researcher", 900);
        assert_eq!(resolve_pending(&p, "never mind", 5), ClarifyResolution::Cancelled);
        assert_eq!(resolve_pending(&p, "cancel", 5), ClarifyResolution::Cancelled);
        assert_eq!(resolve_pending(&p, "forget it", 5), ClarifyResolution::Cancelled);
    }

    #[test]
    fn a_fresh_standalone_command_supersedes_the_pending_question() {
        let p = pending(PrimeIntent::AssignTask, "assign this to researcher", 900);
        // A new create command is a fresh request, not an answer to "which task?".
        assert_eq!(
            resolve_pending(&p, "create a task to summarize the README", 5),
            ClarifyResolution::FreshRequest
        );
        // A question is also standalone.
        assert_eq!(
            resolve_pending(&p, "what is the status?", 5),
            ClarifyResolution::FreshRequest
        );
    }

    #[test]
    fn a_cancel_that_also_carries_a_command_still_acts() {
        // The fresh-request gate runs before the cancellation gate, so the command wins.
        let p = pending(PrimeIntent::AssignTask, "assign this to researcher", 900);
        assert_eq!(
            resolve_pending(&p, "never mind, create a task to ship the beta", 5),
            ClarifyResolution::FreshRequest
        );
    }

    #[test]
    fn combine_is_bounded_and_trims() {
        let long = "x".repeat(1000);
        let out = combine("assign this", &long);
        assert!(out.len() <= MAX_ORIGINAL_CHARS);
        assert!(out.starts_with("assign this x"));
    }

    #[test]
    fn only_resolvable_intents_are_recorded() {
        assert!(is_resolvable_clarify_intent(&PrimeIntent::AssignTask));
        assert!(is_resolvable_clarify_intent(&PrimeIntent::TaskCreation));
        assert!(is_resolvable_clarify_intent(&PrimeIntent::CreateAndRunTask));
        // A run start / task update clarify has no by-id action wired, so it is not
        // recorded (no faked, unresolvable continuation).
        assert!(!is_resolvable_clarify_intent(&PrimeIntent::RunStart));
        assert!(!is_resolvable_clarify_intent(&PrimeIntent::TaskUpdate));
    }
}
