//! Brain-assisted, VALIDATED wording for a clarifying question / brainstorm summary —
//! the reflect-and-clarify prompts moved off pure templates, the last keyword surface
//! the roadmap flagged (`docs/prime-processing-audit.md` "Next recommended slice").
//!
//! ## Why this exists
//!
//! When Prime is ambiguous it asks a clarifying question; when the user is musing it
//! brainstorms. Today that wording is built by deterministic templates
//! ([`crate::prime::brainstorm_reply`], `orchestration_clarify`, `task_update_clarify`,
//! the various `Clarify` arms). They are honest and grounded, but they read like
//! fixed templates — exactly the "keyword-shaped, not an intelligent operator" feel the
//! master plan wants gone (`docs/RELUX_MASTER_PLAN.md` §10.5 "ask clarifying questions
//! when needed", §17.1 "Prime must understand conversational intent"). A configured
//! brain can phrase ONE concrete question — or a concise, helpful brainstorm reply —
//! far more naturally than a template can.
//!
//! ## The safety shape (binding)
//!
//! The brain may only **rewrite the WORDING** of a turn that is already, deterministically,
//! a non-actionful [`relux_core::PrimePlan::Reply`] / `Clarify`. It does not pick the
//! intent, author a slot, or run anything: the action (or absence of one) was decided
//! before this stage and is never read here. So even a maximally adversarial polish can
//! only change the text the user reads on a turn that created/ran nothing — the
//! action-free wall (`docs/prime-processing-audit.md` "Conversation is action-free by
//! design") is intact. Every failure path (no brain, low confidence, malformed JSON,
//! unsupported field, a clarify that is not exactly one question, a reply that claims a
//! completed action) falls back to the deterministic wording.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/prompt_builder.py` / `agent/system_prompt.py` — the
//!   `<missing_context>` / `<act_dont_ask>` blocks steer the model to ask ONE targeted
//!   question instead of guessing or lecturing. We fold the same instruction into
//!   [`build_clarify_prompt`] (clarify → "EXACTLY ONE concrete question") and validate
//!   the result structurally ([`parse_clarify`] enforces a single `?`), rather than
//!   trusting the model to obey.
//! - **Hermes** `agent/message_sanitization.py` (`_escape_invalid_chars_in_json_strings`,
//!   the tool-error length clamp) — sanitize control chars and CLAMP length on every
//!   model string. Mirrored in [`sanitize_line`] / [`sanitize_block`].
//! - **openclaw** `src/agents/tools/sessions-spawn-tool.ts` (`UNSUPPORTED_*_PARAM_KEYS`)
//!   and `src/agents/tools/common.ts` (`readStringParam` required) — reject unsupported
//!   keys, require the mandatory string. [`parse_clarify`] accepts only the
//!   `text`/`confidence`/`rationale` allowlist and requires a non-empty `text`.
//! - **openclaw** `src/agents/cli-output.ts` / `src/shared/balanced-json.ts` — lift the
//!   reply out of a noisy CLI envelope and surface only the parsed text. The CLI path
//!   runs `parse_adapter_result` FIRST, then [`extract_json_object`]; the raw envelope
//!   never reaches the validator or the UI.

use crate::prime_intent::extract_json_object;

/// Minimum confidence before a brain's proposed wording is honored.
const CONFIDENCE_FLOOR: f32 = 0.6;
/// Max characters kept for a single clarifying question (one line, no lecture).
const MAX_CLARIFY_CHARS: usize = 240;
/// Max characters kept for a brainstorm reply (a short paragraph, not an essay).
const MAX_BRAINSTORM_CHARS: usize = 600;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;

/// The only fields a wording proposal may carry. Any other key fails the proposal
/// closed (openclaw's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may not smuggle
/// an action/slot key in as authority.
const ALLOWED_KEYS: &[&str] = &["text", "confidence", "rationale"];

/// Phrases that assert a COMPLETED durable action. A polished reply that contains one is
/// rejected wholesale (fail closed) and the deterministic wording stands, so the brain
/// can never narrate a state change that did not happen — a keyword *safety rail*, not a
/// classifier (`docs/reference-driven-development.md`: keyword rules are fallback rails).
/// Matched against the lowercased text.
const ACTION_CLAIM_MARKERS: &[&str] = &[
    "i created",
    "i've created",
    "ive created",
    "i have created",
    "i made a task",
    "i started",
    "i've started",
    "i kicked off",
    "i ran ",
    "i installed",
    "i've installed",
    "i granted",
    "i've granted",
    "i assigned",
    "i've assigned",
    "i orchestrated",
    "task created",
    "run started",
    "i added the",
];

/// Which kind of wording the brain is polishing. Drives both the prompt and the
/// structural validation (a clarify MUST be one question; a brainstorm reply need not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClarifyKind {
    /// An ambiguous turn that asks the user ONE concrete question.
    Clarify,
    /// A musing/ideation turn that engages the idea (and may ask at most one question).
    Brainstorm,
}

impl ClarifyKind {
    /// The short provenance label for the chip ("clarification" / "brainstorm").
    pub fn label(self) -> &'static str {
        match self {
            ClarifyKind::Clarify => "clarification",
            ClarifyKind::Brainstorm => "brainstorm",
        }
    }
}

/// A validated wording proposal a brain produced for one conversational turn. Only
/// [`parse_clarify`] builds this, after rejecting unknown fields, sanitizing the text,
/// clamping length, and (for a clarify) enforcing exactly one question. The rationale is
/// audit text only.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainClarify {
    pub text: String,
    pub confidence: f32,
    pub rationale: String,
}

/// Decide whether a turn is eligible for brain-assisted wording, and which kind.
///
/// Returns `None` for any actionful turn (a real state change / approval / tool result
/// keeps its grounded deterministic reply — the brain is never near an action), and for
/// any turn that is neither a clarification nor a conversational musing. A multi-step
/// `PlanRequest` is excluded too: it carries a structured proposal card with its own
/// advisory polish overlay, so its prose is not re-shaped here.
pub fn clarify_polish_kind(turn: &relux_core::PrimeTurn) -> Option<ClarifyKind> {
    if crate::ai::is_actionful(turn) {
        return None;
    }
    use relux_core::{PrimeDisposition as D, PrimeIntent as I};
    if turn.disposition == D::NeedsClarification {
        return Some(ClarifyKind::Clarify);
    }
    match turn.intent {
        I::Brainstorming => Some(ClarifyKind::Brainstorm),
        // Casual chitchat and venting are conversation too: a brain may warm up the
        // wording the same way it does a brainstorm reply (it never adds an action;
        // the turn stays a plain `Reply`). Hermes-first general-agent conversation
        // (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5).
        I::SmallTalk | I::EmotionalSupport => Some(ClarifyKind::Brainstorm),
        // A single-step plan steer is a short conversational reply (no multi-step card);
        // a multi-step plan has its own proposal-polish path, so skip it here.
        I::PlanRequest => {
            let multi_step = turn.proposal.as_ref().map(|p| p.multi_step).unwrap_or(false);
            (!multi_step).then_some(ClarifyKind::Brainstorm)
        }
        _ => None,
    }
}

/// The strict, self-contained prompt handed to a brain to re-word ONE conversational
/// turn. It pins Prime's identity, supplies the deterministic wording as the meaning to
/// preserve, forbids any action claim, and demands JSON-only output. Kept ASCII and
/// self-contained so it works as a one-shot CLI prompt with no system-message channel.
pub fn build_clarify_prompt(kind: ClarifyKind, message: &str, deterministic_text: &str) -> String {
    let common = "You are Prime, a general-purpose local AI agent — a helpful assistant and chat \
companion that can also drive a local Relux control plane when asked. You perform NO action this \
turn and create nothing: never claim you created a task, started a run, installed a plugin, \
granted a permission, assigned work, or changed any state. Do not invent runs, tasks, plugins, or \
numbers. Use plain ASCII.";
    match kind {
        ClarifyKind::Clarify => format!(
            "{common}\n\nThe request was ambiguous, so you must ask the user ONE clarifying question. \
Rewrite the question below to be a single, concrete, natural question — keep its MEANING (ask for \
the same missing piece), just phrase it well.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"text\":\"<one concrete question ending in a question mark>\",\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- text: EXACTLY ONE concrete question, ending with '?'. No lecture, no preamble, no list of \
questions. Keep it short.\n\
- Do NOT change WHAT is being asked, only the wording. Do NOT propose or perform any action.\n\
- Do NOT add any field other than text and confidence.\n\n\
The current question (keep its meaning):\n{deterministic_text}\n\n\
User message:\n{message}"
        ),
        ClarifyKind::Brainstorm => format!(
            "{common}\n\nThe user is making conversation — thinking out loud, chatting, or venting. \
Produce a concise, natural, human reply that meets them where they are — a brief sanity-check or \
acknowledgement and, if useful, AT MOST ONE clarifying question. This is a conversation; nothing \
is created or run, and you do not push the user toward work.\n\n\
Respond with JSON ONLY (no prose, no code fences) in EXACTLY this shape:\n\
{{\"text\":\"<a concise, helpful reply>\",\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- text: a SHORT, helpful reply (a few sentences at most). Stay consistent with the grounded reply \
below; you may sharpen it but must not contradict it or claim any action happened.\n\
- Do NOT add any field other than text and confidence.\n\n\
The grounded reply you may build on:\n{deterministic_text}\n\n\
User message:\n{message}"
        ),
    }
}

/// Parse a brain's raw reply into validated [`BrainClarify`], or `Err` with a short
/// reason on anything malformed/unsupported. The schema/allowlist + shape gate.
///
/// A clarify is forced to a single line and MUST contain exactly one `?` and end with it
/// (so it is one concrete question, never a multi-question lecture). A brainstorm reply
/// is a short block. Either kind is rejected if it asserts a completed action.
pub fn parse_clarify(raw: &str, kind: ClarifyKind) -> Result<BrainClarify, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    let raw_text = obj
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing text".to_string())?;

    let text = match kind {
        ClarifyKind::Clarify => sanitize_line(raw_text, MAX_CLARIFY_CHARS),
        ClarifyKind::Brainstorm => sanitize_block(raw_text, MAX_BRAINSTORM_CHARS),
    };
    if text.is_empty() {
        return Err("empty text".to_string());
    }

    // A polished reply must never narrate a state change that did not happen.
    let lowered = text.to_lowercase();
    if ACTION_CLAIM_MARKERS.iter().any(|m| lowered.contains(m)) {
        return Err("text claims a completed action".to_string());
    }

    // A clarify must be ONE concrete question — exactly one '?', ending with it.
    if kind == ClarifyKind::Clarify {
        let question_marks = text.matches('?').count();
        if question_marks != 1 || !text.trim_end().ends_with('?') {
            return Err("clarify must be exactly one question".to_string());
        }
    }

    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;

    let rationale = obj
        .get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_line(s, MAX_RATIONALE_CHARS))
        .unwrap_or_default();

    Ok(BrainClarify {
        text,
        confidence,
        rationale,
    })
}

/// Reconcile a validated wording proposal against the deterministic text, returning the
/// polished text to show, or `None` to keep the deterministic wording.
///
/// `None` on low confidence, or when the proposal merely echoes the deterministic text
/// (no point attributing a brain for a no-op). The structural validation already ran in
/// [`parse_clarify`]; this is the confidence / echo gate.
pub fn reconcile_clarify(
    deterministic_text: &str,
    proposal: &BrainClarify,
    _kind: ClarifyKind,
) -> Option<String> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }
    if proposal
        .text
        .trim()
        .eq_ignore_ascii_case(deterministic_text.trim())
    {
        return None;
    }
    Some(proposal.text.clone())
}

/// Sanitize a single-line string: control chars → space, collapse whitespace, trim,
/// clamp. Shared shape with [`crate::prime_slots`] / [`crate::prime_agent_slots`].
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect::<String>().trim().to_string()
}

/// Sanitize a multi-line block: drop control chars except `\n`, collapse intra-line
/// whitespace, drop blank lines, trim, clamp.
fn sanitize_block(s: &str, max: usize) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|line| {
            let cleaned: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect();
    let joined = lines.join("\n");
    let truncated: String = joined.chars().take(max).collect();
    truncated.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_carries_the_schema_and_safety_rules() {
        let clarify = build_clarify_prompt(ClarifyKind::Clarify, "update it", "Which task?");
        assert!(clarify.contains("\"text\""));
        assert!(clarify.contains("JSON ONLY"));
        assert!(clarify.contains("EXACTLY ONE"));
        assert!(clarify.contains("NO action") || clarify.contains("perform NO action"));
        assert!(clarify.contains("update it"));

        let brainstorm =
            build_clarify_prompt(ClarifyKind::Brainstorm, "what about a queue", "Good - let's think.");
        assert!(brainstorm.contains("conversation"));
        assert!(brainstorm.contains("AT MOST ONE"));
    }

    #[test]
    fn parses_a_clean_clarify_question() {
        let p = parse_clarify(
            r#"{"text":"Which task should I update - task_42 or task_7?","confidence":0.9}"#,
            ClarifyKind::Clarify,
        )
        .unwrap();
        assert!(p.text.ends_with('?'));
        assert_eq!(p.confidence, 0.9);
    }

    #[test]
    fn extracts_from_noisy_reply_with_prose_and_fences() {
        let raw = "Sure:\n```json\n{\"text\": \"What outcome are you after?\", \"confidence\": 0.8}\n```";
        let p = parse_clarify(raw, ClarifyKind::Clarify).unwrap();
        assert_eq!(p.text, "What outcome are you after?");
    }

    #[test]
    fn rejects_invalid_json_and_unsupported_fields() {
        assert!(parse_clarify("not json", ClarifyKind::Clarify).is_err());
        // A smuggled action key fails the whole proposal closed.
        assert!(parse_clarify(
            r#"{"text":"Which task?","run":true,"confidence":0.9}"#,
            ClarifyKind::Clarify
        )
        .is_err());
        // Missing/empty text.
        assert!(parse_clarify(r#"{"confidence":0.9}"#, ClarifyKind::Clarify).is_err());
        assert!(parse_clarify(r#"{"text":"   ","confidence":0.9}"#, ClarifyKind::Clarify).is_err());
    }

    #[test]
    fn clarify_must_be_exactly_one_question() {
        // Zero questions (a statement) is rejected.
        assert!(parse_clarify(
            r#"{"text":"I will update the task.","confidence":0.9}"#,
            ClarifyKind::Clarify
        )
        .is_err());
        // A multi-question lecture is rejected.
        assert!(parse_clarify(
            r#"{"text":"Which task? And what field? And the new value?","confidence":0.9}"#,
            ClarifyKind::Clarify
        )
        .is_err());
        // A brainstorm reply has no single-question requirement.
        let b = parse_clarify(
            r#"{"text":"A queue is a solid call. What throughput do you expect?","confidence":0.9}"#,
            ClarifyKind::Brainstorm,
        )
        .unwrap();
        assert!(b.text.contains("queue"));
    }

    #[test]
    fn rejects_a_reply_that_claims_a_completed_action() {
        // Even though this stage only runs on action-free turns, a polish that narrates
        // a state change is rejected so the user is never told something false.
        assert!(parse_clarify(
            r#"{"text":"I created the task and started the run.","confidence":0.95}"#,
            ClarifyKind::Brainstorm
        )
        .is_err());
    }

    #[test]
    fn strips_control_chars_and_clamps() {
        let p = parse_clarify(
            "{\"text\":\"Which\\ttask\\nshould I update?\",\"confidence\":0.9}",
            ClarifyKind::Clarify,
        )
        .unwrap();
        assert_eq!(p.text, "Which task should I update?");
        assert!(!p.text.contains('\n') && !p.text.contains('\t'));
    }

    #[test]
    fn reconcile_honors_a_confident_distinct_wording() {
        let p = parse_clarify(
            r#"{"text":"Which task should I update?","confidence":0.9}"#,
            ClarifyKind::Clarify,
        )
        .unwrap();
        assert_eq!(
            reconcile_clarify("Which task should I update, and what should change?", &p, ClarifyKind::Clarify)
                .as_deref(),
            Some("Which task should I update?")
        );
        // Low confidence keeps the deterministic wording.
        let low = parse_clarify(
            r#"{"text":"Which task should I update?","confidence":0.3}"#,
            ClarifyKind::Clarify,
        )
        .unwrap();
        assert!(reconcile_clarify("Anything", &low, ClarifyKind::Clarify).is_none());
        // A pure echo of the deterministic wording is a no-op (no chip).
        let echo = parse_clarify(
            r#"{"text":"Which task should I update?","confidence":0.9}"#,
            ClarifyKind::Clarify,
        )
        .unwrap();
        assert!(reconcile_clarify("which task should i update?", &echo, ClarifyKind::Clarify).is_none());
    }
}
