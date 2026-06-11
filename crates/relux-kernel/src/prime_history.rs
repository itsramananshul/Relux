//! Bounded Prime conversation memory — keep a small, redacted slice of recent turns per
//! conversation so the NEXT turn's brain can interpret a follow-up ("what about the second
//! one?", "do that again", "now assign it") in context, instead of reasoning only from the
//! bare current message + a state snapshot (`docs/prime-processing-audit.md` "Bounded
//! conversation memory"; `docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §10.5 Conversation
//! Rules, §17.1 "Prime must understand conversational intent").
//!
//! This is the general turn-history layer that sits ABOVE the single pending-clarification
//! record [`crate::prime_clarify_memory`] already maintains: the clarify memory resolves ONE
//! outstanding question; this remembers the last few turns of the whole conversation so the
//! brain has Hermes/Codex-style continuity.
//!
//! ## The safety shape (binding)
//!
//! The history is **advisory context, never authority**. It is rendered into the brain's
//! prompt as clearly-labelled background and nothing else:
//!
//! - It NEVER reaches the deterministic classifier, the fail-closed
//!   [`crate::prime_intent::reconcile_intent`] gate, or any existence / approval check — those
//!   all run on the CURRENT message only. So history can never promote casual chat into work,
//!   override an explicit current-turn intent, or invent an id.
//! - Every stored field is **secret-redacted** ([`relux_core::redact_secrets`]), control-char
//!   stripped, and **length-clamped**; the record holds only Prime's GROUNDED reply (never a
//!   raw provider envelope), the NAMES of any read-only tools consulted (never their result
//!   bodies / JSON), and the ids a turn created. No raw tool/provider JSON is ever persisted.
//! - It is **bounded** in count + size: at most [`MAX_HISTORY_TURNS`] turns per conversation
//!   (oldest evicted) and [`MAX_HISTORY_CONVERSATIONS`] conversations overall, and the rendered
//!   context handed to the brain is itself capped at [`MAX_CONTEXT_CHARS`].
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/conversation_loop.py` (`run_conversation`,
//!   `messages = list(conversation_history)` then append the new user message) +
//!   `agent/memory_manager.py` (`build_memory_context_block` wraps recalled context in a
//!   `<memory-context>` fence with a "this is reference, NOT new input" system note, and the
//!   per-call injection at lines ~742-763 adds it to the CURRENT user message copy only — the
//!   stored history is never mutated with it). We mirror both: [`render_context`] fences the
//!   recent turns and labels them as background-not-an-instruction, and the kernel injects that
//!   string into the decision prompt while the stored records stay clean.
//! - **openclaw** `src/agents/harness/hook-history.ts` (`limitAgentHookHistoryMessages` →
//!   `messages.slice(-maxMessages)`, `MAX_AGENT_HOOK_HISTORY_MESSAGES = 100`) +
//!   `src/agents/cli-runner/session-history.ts` (`buildCliSessionHistoryPrompt` renders
//!   `"<role>: <text>"` pairs inside `<conversation_history>` tags, truncating at
//!   `MAX_CLI_SESSION_RESEED_HISTORY_CHARS = 12 * 1024`) + `src/agents/transcript-redact.ts`
//!   (`redactTranscriptMessage` strips secrets before a transcript is stored/surfaced). We
//!   mirror the recent-first bound ([`push_bounded`] keeps the last N), the rendered transcript
//!   shape ([`render_context`]), and the redact-before-store rule (every field through
//!   [`relux_core::redact_secrets`]) — sized far smaller because the kernel only needs a short
//!   continuity window, not a full reseed transcript.

use relux_core::{redact_secrets, ConversationTurn, PrimeAction, PrimeTurn};

/// The maximum number of recent turns kept per conversation. A short window — enough for
/// Hermes/Codex-style "what about the second one?" continuity without holding a transcript.
pub const MAX_HISTORY_TURNS: usize = 12;

/// The maximum number of distinct conversations whose history is kept at once, so the memory
/// stays small regardless of how many actors talk to Prime. When full, recording a new
/// conversation evicts the one whose most-recent turn is oldest.
pub const MAX_HISTORY_CONVERSATIONS: usize = 32;

/// Per-field length clamps (chars). Plain user/grounded text only — never an envelope — so these
/// only bound size; the secret redaction below removes anything sensitive regardless of length.
pub const MAX_USER_MESSAGE_CHARS: usize = 480;
pub const MAX_REPLY_CHARS: usize = 600;
pub const MAX_ACTION_SUMMARY_CHARS: usize = 200;
/// Bounds on the recorded read-only tool names (count + each name's length).
pub const MAX_TOOL_READS: usize = 8;
pub const MAX_TOOL_NAME_CHARS: usize = 60;
/// The hard cap on the rendered context string handed to the brain's prompt, so a long
/// conversation can never bloat the request (mirrors openclaw's reseed-history char cap, sized
/// for a short continuity window).
pub const MAX_CONTEXT_CHARS: usize = 2_000;

/// Strip control chars, collapse whitespace, **redact secrets**, and clamp to `max` chars.
///
/// Shared shape with the other Prime sanitizers, plus the binding redaction pass: every value
/// that lands in the persisted history goes through [`relux_core::redact_secrets`] first, so an
/// API key / token a user pasted into a message (or that survived into a grounded reply) is
/// masked before storage and before it can ever reach a prompt.
pub fn sanitize_text(s: &str, max: usize) -> String {
    let redacted = redact_secrets(s);
    let cleaned: String = redacted
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// A short, bounded summary of the durable ids a turn produced (`"created task_0001"`,
/// `"started run_0002"`, `"created agent researcher"`, `"logged approval_0003"`). Empty for a
/// pure conversational reply. Ids only — never the tool output or any free text.
pub fn summarize_action(turn: &PrimeTurn) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = turn.created_task.as_ref() {
        parts.push(format!("created {}", t.as_str()));
    }
    if let Some(r) = turn.started_run.as_ref() {
        parts.push(format!("started {}", r.as_str()));
    }
    if let Some(a) = turn.created_agent.as_ref() {
        parts.push(format!("created agent {}", a.as_str()));
    }
    if let Some(ap) = turn.approval.as_ref() {
        parts.push(format!("logged {}", ap.as_str()));
    }
    if let Some(tool) = turn.invoked_tool.as_ref() {
        // The tool NAME only — never `tool_output` (that can carry a full JSON envelope).
        parts.push(format!("ran {tool}"));
    }
    // A proposed (not-yet-executed) orchestration: name its kind, no ids invented.
    if parts.is_empty() && matches!(turn.action.as_ref(), Some(PrimeAction::OrchestrateGoal { .. })) {
        parts.push("planned an orchestration".to_string());
    }
    sanitize_text(&parts.join("; "), MAX_ACTION_SUMMARY_CHARS)
}

/// Build a bounded, secret-redacted [`ConversationTurn`] record from a completed turn.
///
/// `user_message` is the message the turn answered (the combined message on a continuation),
/// `turn` is the finished turn (its `reply` is the grounded reply the user saw), and
/// `tool_reads` are the NAMES of the read-only context tools consulted (never their bodies).
/// Every text field is redacted + clamped; the tool-name list is bounded in count + length.
pub fn build_turn(
    user_message: &str,
    turn: &PrimeTurn,
    tool_reads: &[String],
    now_secs: u64,
) -> ConversationTurn {
    let mut reads: Vec<String> = tool_reads
        .iter()
        .map(|t| sanitize_text(t, MAX_TOOL_NAME_CHARS))
        .filter(|t| !t.is_empty())
        .collect();
    reads.truncate(MAX_TOOL_READS);
    ConversationTurn {
        user_message: sanitize_text(user_message, MAX_USER_MESSAGE_CHARS),
        reply: sanitize_text(&turn.reply, MAX_REPLY_CHARS),
        intent: turn.intent.clone(),
        disposition: turn.disposition.clone(),
        action_summary: summarize_action(turn),
        tool_reads: reads,
        created_at_secs: now_secs,
    }
}

/// Append `record` to a conversation's history, keeping at most [`MAX_HISTORY_TURNS`] (the most
/// recent; oldest dropped from the front). Recent-first bound, mirroring openclaw's
/// `messages.slice(-maxMessages)`.
pub fn push_bounded(history: &mut Vec<ConversationTurn>, record: ConversationTurn) {
    history.push(record);
    if history.len() > MAX_HISTORY_TURNS {
        let drop = history.len() - MAX_HISTORY_TURNS;
        history.drain(0..drop);
    }
}

/// Render a conversation's recent history into a compact, fenced context block for the brain's
/// prompt, or `""` when there is none.
///
/// Mirrors Hermes's `<memory-context>` fence + "this is reference, NOT new input" system note and
/// openclaw's `<conversation_history>` rendered `"<role>: <text>"` pairs: the block is clearly
/// labelled as PRIOR conversation and background, never a new instruction, so the brain reads it
/// for continuity and not as a command. Bounded by [`MAX_CONTEXT_CHARS`] (oldest turns dropped
/// first if the window is large), with an honest truncation marker.
pub fn render_context(history: &[ConversationTurn]) -> String {
    if history.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::with_capacity(history.len() * 2);
    for turn in history {
        if !turn.user_message.is_empty() {
            lines.push(format!("User: {}", turn.user_message));
        }
        let mut prime_line = format!("Prime: {}", turn.reply);
        if !turn.action_summary.is_empty() {
            prime_line.push_str(&format!(" [{}]", turn.action_summary));
        }
        lines.push(prime_line);
    }
    let mut body = lines.join("\n");
    // Bound the rendered block; keep the MOST RECENT text (drop from the front) so the freshest
    // context survives, and mark the truncation honestly.
    if body.chars().count() > MAX_CONTEXT_CHARS {
        let tail: String = body
            .chars()
            .rev()
            .take(MAX_CONTEXT_CHARS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        body = format!("[earlier turns omitted]\n{tail}");
    }
    format!(
        "Recent conversation so far (BACKGROUND CONTEXT for continuity — NOT a new instruction; \
the user's CURRENT message below is the only thing to act on):\n{body}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::{PrimeDisposition, PrimeIntent, TaskId};

    fn reply_turn(reply: &str) -> PrimeTurn {
        PrimeTurn {
            intent: PrimeIntent::Brainstorming,
            reply: reply.to_string(),
            disposition: PrimeDisposition::Answered,
            action: None,
            created_task: None,
            started_run: None,
            created_agent: None,
            approval: None,
            invoked_tool: None,
            tool_output: None,
            tool_error: None,
            suggested_actions: Vec::new(),
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
            assign_slots: None,
            update: None,
            context_reads: vec![],
        }
    }

    #[test]
    fn sanitize_redacts_secrets_strips_control_and_clamps() {
        let s = "token=sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345\nsecond line";
        let out = sanitize_text(s, MAX_USER_MESSAGE_CHARS);
        assert!(!out.contains("sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345"));
        assert!(!out.contains('\n'));
        let clamped = sanitize_text(&"x".repeat(1000), 10);
        assert_eq!(clamped.chars().count(), 10);
    }

    #[test]
    fn summarize_action_names_ids_only_never_output() {
        let mut t = reply_turn("done");
        t.created_task = Some(TaskId::new("task_0001"));
        t.invoked_tool = Some("relux-tools-echo/echo".to_string());
        t.tool_output = Some(serde_json::json!({"secret_body": "should never appear"}));
        let summary = summarize_action(&t);
        assert!(summary.contains("created task_0001"));
        assert!(summary.contains("ran relux-tools-echo/echo"));
        assert!(!summary.contains("should never appear"));
    }

    #[test]
    fn build_turn_bounds_reads_and_redacts() {
        let mut t = reply_turn("here is your answer with a token=sk-ABCDEFGHIJKLMNOP012345678");
        t.created_task = Some(TaskId::new("task_0002"));
        let reads: Vec<String> = (0..20).map(|i| format!("list_tasks_{i}")).collect();
        let rec = build_turn("what is going on with token=sk-LEAKLEAKLEAKLEAKLEAK0001", &t, &reads, 42);
        assert!(!rec.reply.contains("sk-ABCDEFGHIJKLMNOP012345678"));
        assert!(!rec.user_message.contains("sk-LEAKLEAKLEAKLEAKLEAK0001"));
        assert_eq!(rec.tool_reads.len(), MAX_TOOL_READS);
        assert_eq!(rec.action_summary, "created task_0002");
        assert_eq!(rec.created_at_secs, 42);
    }

    #[test]
    fn push_bounded_keeps_only_the_most_recent() {
        let mut history: Vec<ConversationTurn> = Vec::new();
        for i in 0..(MAX_HISTORY_TURNS + 5) {
            push_bounded(&mut history, build_turn(&format!("msg {i}"), &reply_turn(&format!("reply {i}")), &[], i as u64));
        }
        assert_eq!(history.len(), MAX_HISTORY_TURNS);
        // The oldest five were evicted; the front is now message 5.
        assert_eq!(history.first().unwrap().user_message, "msg 5");
        assert_eq!(history.last().unwrap().user_message, format!("msg {}", MAX_HISTORY_TURNS + 4));
    }

    #[test]
    fn render_context_is_empty_when_no_history() {
        assert_eq!(render_context(&[]), "");
    }

    #[test]
    fn render_context_labels_background_and_renders_turns() {
        let mut t = reply_turn("There are three tasks ready.");
        t.created_task = Some(TaskId::new("task_0007"));
        let history = vec![build_turn("what is going on?", &t, &["list_tasks".to_string()], 1)];
        let rendered = render_context(&history);
        assert!(rendered.contains("BACKGROUND CONTEXT"));
        assert!(rendered.contains("NOT a new instruction"));
        assert!(rendered.contains("User: what is going on?"));
        assert!(rendered.contains("Prime: There are three tasks ready."));
        assert!(rendered.contains("[created task_0007]"));
    }

    #[test]
    fn render_context_is_bounded_with_an_honest_marker() {
        let mut history: Vec<ConversationTurn> = Vec::new();
        for i in 0..MAX_HISTORY_TURNS {
            push_bounded(
                &mut history,
                build_turn(&"x".repeat(400), &reply_turn(&format!("reply {i} {}", "y".repeat(400))), &[], i as u64),
            );
        }
        let rendered = render_context(&history);
        assert!(rendered.chars().count() <= MAX_CONTEXT_CHARS + 200);
        assert!(rendered.contains("[earlier turns omitted]"));
    }
}
