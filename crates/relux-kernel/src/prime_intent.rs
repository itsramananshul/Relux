//! Brain-mediated intent classification — the structured decision stage that lets
//! a real LLM brain (OpenRouter / Claude CLI / Codex CLI) *propose* the intent of
//! a Prime turn, with the deterministic keyword classifier
//! ([`crate::prime::classify_intent`]) as the always-present fallback and a
//! fail-closed safety gate the brain can never talk its way past.
//!
//! ## Why this exists
//!
//! [`crate::prime::classify_intent`] is an ordered cascade of `contains`/`starts_with`
//! keyword checks. It is honest and predictable, but brittle: a politely phrased
//! request ("could you take care of the login bug") names no creation verb and
//! falls through to a generic chat answer instead of becoming a task. The master
//! plan asks for the opposite — Prime should *understand conversational intent*
//! and *feel like Codex with access to Relux actions*
//! (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §17.1). A real brain handles
//! that long tail; keyword rules cannot.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! The shape here is lifted from how Hermes and Paperclip (openclaw) keep a
//! model's chosen action safe — read first, then adapted:
//!
//! - **Hermes** `agent/conversation_loop.py` (~L3116-3162): the model's chosen
//!   tool is validated against a NAME ALLOWLIST *before* anything is done with it;
//!   an off-list name is rejected and fed back, never executed. We mirror that:
//!   [`parse_intent_proposal`] accepts an intent only if the label round-trips
//!   through `PrimeIntent`'s own deserializer — the model can never name an intent
//!   that does not exist.
//! - **Paperclip/openclaw** `src/agents/tool-mutation.ts` (`isMutatingToolCall`):
//!   a single FAIL-CLOSED classifier decides auto-approve vs. gate, defaulting an
//!   unknown action to *mutating*. And `src/agents/tool-policy.ts`
//!   (`applyOwnerOnlyToolPolicy`) makes work-creation one explicit, gated
//!   capability — never inferred from casual chat. We mirror both:
//!   [`reconcile_intent`] is the one gate, and it FORBIDS a brain from promoting a
//!   guarded conversational turn to any work intent.
//! - **openclaw** `src/agents/cli-output.ts` (`extractBalancedJsonFragments`):
//!   pull the JSON out of a noisy CLI reply with a balanced-brace scan rather than
//!   trusting the whole stdout. [`extract_json_object`] does the same.
//!
//! ## The contract
//!
//! The brain decides INTENT only. Every durable state change still comes from the
//! deterministic kernel path ([`crate::KernelState::prime_turn_with_intent`] →
//! `decide` → `prime_execute`); slots (task titles, agent names, goals) are still
//! derived deterministically from the message. On any brain failure — no key,
//! disabled, timeout, error envelope, off-allowlist label, low confidence — the
//! turn falls back to [`crate::prime::classify_intent`]. The brain is strictly
//! additive: it can sharpen a misread intent, but it can never mint or run work
//! from chat, and it can never auto-run a task the user did not ask to run.

use relux_core::PrimeIntent;
use serde::Deserialize;

use crate::prime::is_chat_guarded;

/// Minimum confidence before a brain's proposed intent may override the
/// deterministic classification. Below this the keyword classifier wins — a
/// hesitant brain never gets to reshape the turn.
const CONFIDENCE_FLOOR: f32 = 0.6;

/// Max characters kept from the brain's free-text rationale. It is audit /
/// provenance text only — never executed, never used as a slot — so this just
/// bounds a runaway reply.
const MAX_RATIONALE_CHARS: usize = 240;

/// Where a turn's final intent came from. Recorded in the audit log and surfaced
/// as honest provenance; it never affects execution (both paths feed the same
/// `decide`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentSource {
    /// The deterministic keyword classifier decided — no brain configured, the
    /// brain failed, confidence was too low, or the safety gate vetoed the brain.
    Deterministic,
    /// A brain proposed the intent and the safety gate accepted it.
    Brain,
}

impl IntentSource {
    /// Stable wire string (`deterministic` | `brain`).
    pub fn as_str(&self) -> &'static str {
        match self {
            IntentSource::Deterministic => "deterministic",
            IntentSource::Brain => "brain",
        }
    }
}

/// A structured intent decision a brain *proposes* for one Prime turn.
///
/// Only [`parse_intent_proposal`] builds this, and only after validating the
/// label against the `PrimeIntent` allowlist and clamping the confidence. The
/// rationale is presentation / audit text.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainIntentProposal {
    pub intent: PrimeIntent,
    pub confidence: f32,
    pub rationale: String,
}

/// The strict, self-contained prompt handed to a brain to classify ONE message.
///
/// Mirrors Hermes' prompt-steered classification (`agent/prompt_builder.py`): the
/// allowed labels are listed explicitly (the allowlist the model must pick from),
/// the conversational-safety rules are spelled out (musing/questions stay chat;
/// only an explicit instruction is work; ambiguous → ask), and the model is told
/// to answer with JSON ONLY so the reply parses cleanly and no prose leaks
/// downstream.
pub fn build_intent_prompt(message: &str) -> String {
    let labels = intent_labels().join(", ");
    format!(
        "You are the intent classifier for Prime, the operator of a local Relux control plane \
(tasks, runs, agents, plugins, permissions, approvals, an audit log). Classify the user's \
message into EXACTLY ONE of these intent labels:\n{labels}\n\n\
Rules:\n\
- Casual chat, musing, or thinking out loud (\"I was thinking we could...\", \"we should...\") \
is brainstorming, NOT work. Never pick a work or creation intent for it.\n\
- A QUESTION the user is asking or deliberating (\"how does X work?\", \"should we refactor?\") \
is brainstorming or direct_answer, NOT work.\n\
- Only an explicit instruction to DO something (\"create a task to...\", \"run it\", \
\"orchestrate...\", \"fix the login bug\") is a work intent.\n\
- A request to lay an idea out as a reviewable plan is plan_request (it only PREVIEWS; it \
creates nothing).\n\
- If the instruction is genuinely ambiguous, prefer brainstorming so Prime can ask.\n\
- greeting for hellos; status_question for \"what's running?\"; explanation_request for \
\"why did it fail?\"; tool_discovery for \"what tools can you use?\".\n\n\
Respond with JSON ONLY, no prose and no code fences, in exactly this shape:\n\
{{\"intent\":\"<one label>\",\"confidence\":<0.0-1.0>,\"rationale\":\"<short reason>\"}}\n\n\
User message:\n{message}"
    )
}

/// The wire labels offered to the brain in the prompt — the snake_case
/// `PrimeIntent` serialization. This is advisory only: [`parse_intent_proposal`]
/// validates against `PrimeIntent`'s deserializer, so a label that drifts from
/// this list simply fails validation rather than slipping through.
fn intent_labels() -> Vec<&'static str> {
    vec![
        "greeting",
        "status_question",
        "task_creation",
        "create_and_run_task",
        "task_update",
        "assign_task",
        "run_start",
        "run_retry",
        "agent_creation",
        "plugin_installation",
        "permission_change",
        "approval_response",
        "explanation_request",
        "dashboard_navigation",
        "brainstorming",
        "orchestration",
        "plan_request",
        "tool_discovery",
        "tool_invocation",
        "direct_answer",
    ]
}

/// Parse a brain's raw reply into a validated [`BrainIntentProposal`], or `Err`
/// with a short reason on anything malformed.
///
/// This is the allowlist / schema gate: the `intent` must deserialize to a real
/// `PrimeIntent` (an unknown label is rejected), the confidence is clamped to
/// `[0,1]`, and the rationale is truncated. The brain's raw text never flows
/// anywhere else — a parse failure simply drops the caller to the deterministic
/// path.
pub fn parse_intent_proposal(raw: &str) -> Result<BrainIntentProposal, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;

    #[derive(Deserialize)]
    struct Raw {
        intent: String,
        #[serde(default)]
        confidence: Option<f32>,
        #[serde(default)]
        rationale: Option<String>,
    }

    let parsed: Raw =
        serde_json::from_str(&json).map_err(|_| "reply was not the expected JSON".to_string())?;

    // Validate the label against the PrimeIntent allowlist by round-tripping it
    // through the enum's own deserializer — an unknown label fails here.
    let intent: PrimeIntent =
        serde_json::from_value(serde_json::Value::String(parsed.intent.trim().to_string()))
            .map_err(|_| format!("'{}' is not a known intent", parsed.intent))?;

    let confidence = parsed.confidence.unwrap_or(0.5).clamp(0.0, 1.0);
    let rationale: String = parsed
        .rationale
        .unwrap_or_default()
        .trim()
        .chars()
        .take(MAX_RATIONALE_CHARS)
        .collect();

    Ok(BrainIntentProposal {
        intent,
        confidence,
        rationale,
    })
}

/// Pull the first balanced top-level `{...}` object out of a possibly noisy reply
/// (leading prose, code fences). Mirrors openclaw's balanced-JSON extraction
/// (`src/agents/cli-output.ts`) so a brain that wraps its JSON in chatter still
/// parses — and a reply with no balanced object is rejected, never shown.
fn extract_json_object(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in raw.as_bytes().iter().enumerate().skip(start) {
        let c = b as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    // `start` is at `{` and `i` is at the matching `}`; both are
                    // ASCII, so this is always a valid char boundary.
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// True for an intent that creates or changes durable state, runs work, or
/// responds to an approval — the SENSITIVE set. On guarded chat the brain may
/// never promote a turn to one of these (the fail-closed rail). Conversational
/// intents (greeting, status, explanation, brainstorming, navigation, tool
/// discovery, direct answer) and the action-free `plan_request` preview are NOT
/// sensitive: a brain may steer guarded musing into a plan preview, which creates
/// nothing.
fn is_sensitive_intent(intent: &PrimeIntent) -> bool {
    matches!(
        intent,
        PrimeIntent::TaskCreation
            | PrimeIntent::CreateAndRunTask
            | PrimeIntent::TaskUpdate
            | PrimeIntent::AssignTask
            | PrimeIntent::RunStart
            | PrimeIntent::RunRetry
            | PrimeIntent::AgentCreation
            | PrimeIntent::PluginInstallation
            | PrimeIntent::PermissionChange
            | PrimeIntent::ApprovalResponse
            | PrimeIntent::Orchestration
            | PrimeIntent::ToolInvocation
    )
}

/// Whether the message itself carries explicit run language. Used so a brain may
/// create a task but never silently AUTO-RUN one the user did not ask to run.
fn mentions_run(message: &str) -> bool {
    let m = message.to_lowercase();
    [
        "run it",
        "and run it",
        "start it",
        "and start it",
        "kick off",
        "execute it",
        "and execute it",
    ]
    .iter()
    .any(|p| m.contains(p))
}

/// Reconcile the deterministic intent with a brain's proposal under a fail-closed
/// safety policy. This is the single gate; it runs at the kernel chokepoint so the
/// rules hold no matter which caller produced the proposal.
///
/// Policy — each rule fails toward the deterministic / safer choice:
/// 1. Low confidence (`< CONFIDENCE_FLOOR`) → keep the deterministic intent.
/// 2. Guarded chat (ideation / a question without an explicit command) + a
///    SENSITIVE proposed intent → veto: keep deterministic. Casual chat can never
///    mint or run work, exactly as today (§10.5, §17.1).
/// 3. A proposed `create_and_run_task` with no explicit run language in the
///    message → downgrade to `task_creation`: create, never silently auto-run.
/// 4. Otherwise → accept the brain's intent.
pub fn reconcile_intent(
    deterministic: PrimeIntent,
    proposal: &BrainIntentProposal,
    message: &str,
) -> (PrimeIntent, IntentSource) {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return (deterministic, IntentSource::Deterministic);
    }
    if is_chat_guarded(message) && is_sensitive_intent(&proposal.intent) {
        return (deterministic, IntentSource::Deterministic);
    }
    if proposal.intent == PrimeIntent::CreateAndRunTask
        && deterministic != PrimeIntent::CreateAndRunTask
        && !mentions_run(message)
    {
        return (PrimeIntent::TaskCreation, IntentSource::Brain);
    }
    (proposal.intent.clone(), IntentSource::Brain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(intent: PrimeIntent, confidence: f32) -> BrainIntentProposal {
        BrainIntentProposal {
            intent,
            confidence,
            rationale: "because".to_string(),
        }
    }

    // --- parse_intent_proposal: the allowlist / schema gate -----------------

    #[test]
    fn parses_a_clean_json_object() {
        let p = parse_intent_proposal(
            r#"{"intent":"task_creation","confidence":0.9,"rationale":"explicit ask"}"#,
        )
        .unwrap();
        assert_eq!(p.intent, PrimeIntent::TaskCreation);
        assert_eq!(p.confidence, 0.9);
        assert_eq!(p.rationale, "explicit ask");
    }

    #[test]
    fn extracts_json_from_noisy_reply_with_prose_and_fences() {
        // A CLI brain that ignores "JSON only" and wraps the object in chatter
        // still parses — the balanced-brace scan lifts just the object.
        let raw = "Sure! Here is my classification:\n```json\n{\"intent\": \"brainstorming\", \
                   \"confidence\": 0.7}\n```\nHope that helps.";
        let p = parse_intent_proposal(raw).unwrap();
        assert_eq!(p.intent, PrimeIntent::Brainstorming);
    }

    #[test]
    fn rejects_an_off_allowlist_label() {
        // A hallucinated intent the enum does not define is refused, so the caller
        // falls back to the deterministic classifier rather than acting on junk.
        let err = parse_intent_proposal(r#"{"intent":"delete_everything","confidence":1.0}"#)
            .unwrap_err();
        assert!(err.contains("not a known intent"), "got: {err}");
    }

    #[test]
    fn rejects_reply_with_no_json_object() {
        assert!(parse_intent_proposal("I think this is a task.").is_err());
    }

    #[test]
    fn clamps_confidence_and_defaults_when_absent() {
        assert_eq!(
            parse_intent_proposal(r#"{"intent":"greeting","confidence":5.0}"#)
                .unwrap()
                .confidence,
            1.0
        );
        // Absent confidence defaults to a neutral 0.5 (below the override floor),
        // so a brain that omits it cannot override the deterministic intent.
        assert_eq!(
            parse_intent_proposal(r#"{"intent":"greeting"}"#)
                .unwrap()
                .confidence,
            0.5
        );
    }

    // --- reconcile_intent: the fail-closed safety gate ----------------------

    #[test]
    fn brain_sharpens_a_politely_phrased_action_request() {
        // "could you take care of the login bug" names no creation verb, so the
        // deterministic classifier reads it as a generic answer; a confident brain
        // correctly calls it task creation, and the message is NOT guarded chat.
        let (intent, source) = reconcile_intent(
            PrimeIntent::DirectAnswer,
            &prop(PrimeIntent::TaskCreation, 0.9),
            "could you take care of the login bug",
        );
        assert_eq!(intent, PrimeIntent::TaskCreation);
        assert_eq!(source, IntentSource::Brain);
    }

    #[test]
    fn guarded_ideation_can_never_be_promoted_to_work() {
        // Musing ("we should...") is guarded chat. Even a 0.99-confidence brain
        // that wants to mint a task is vetoed — chat never creates work (§10.5).
        let (intent, source) = reconcile_intent(
            PrimeIntent::Brainstorming,
            &prop(PrimeIntent::TaskCreation, 0.99),
            "we should refactor the auth module",
        );
        assert_eq!(intent, PrimeIntent::Brainstorming);
        assert_eq!(source, IntentSource::Deterministic);
    }

    #[test]
    fn guarded_question_can_never_be_promoted_to_work() {
        let (intent, source) = reconcile_intent(
            PrimeIntent::Brainstorming,
            &prop(PrimeIntent::Orchestration, 0.95),
            "should we split this across a few agents?",
        );
        assert_eq!(intent, PrimeIntent::Brainstorming);
        assert_eq!(source, IntentSource::Deterministic);
    }

    #[test]
    fn guarded_musing_may_still_become_an_action_free_plan_preview() {
        // plan_request is not sensitive: it previews and creates nothing, so a
        // brain may steer guarded musing into a reviewable plan.
        let (intent, source) = reconcile_intent(
            PrimeIntent::Brainstorming,
            &prop(PrimeIntent::PlanRequest, 0.9),
            "we should improve the onboarding flow",
        );
        assert_eq!(intent, PrimeIntent::PlanRequest);
        assert_eq!(source, IntentSource::Brain);
    }

    #[test]
    fn low_confidence_keeps_the_deterministic_intent() {
        let (intent, source) = reconcile_intent(
            PrimeIntent::DirectAnswer,
            &prop(PrimeIntent::TaskCreation, 0.4),
            "could you take care of the login bug",
        );
        assert_eq!(intent, PrimeIntent::DirectAnswer);
        assert_eq!(source, IntentSource::Deterministic);
    }

    #[test]
    fn create_and_run_is_downgraded_to_create_without_explicit_run_language() {
        // A brain that wants to auto-run is capped at creation unless the user
        // actually said to run it — no silent auto-run.
        let (intent, source) = reconcile_intent(
            PrimeIntent::TaskCreation,
            &prop(PrimeIntent::CreateAndRunTask, 0.9),
            "make a task to summarize the readme",
        );
        assert_eq!(intent, PrimeIntent::TaskCreation);
        assert_eq!(source, IntentSource::Brain);
    }

    #[test]
    fn create_and_run_is_honored_with_explicit_run_language() {
        let (intent, source) = reconcile_intent(
            PrimeIntent::CreateAndRunTask,
            &prop(PrimeIntent::CreateAndRunTask, 0.9),
            "create a task to summarize the readme and run it",
        );
        assert_eq!(intent, PrimeIntent::CreateAndRunTask);
        assert_eq!(source, IntentSource::Brain);
    }
}
