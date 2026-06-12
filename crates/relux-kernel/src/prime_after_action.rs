//! Safe POST-EXECUTION reply shaping for an ACTIONFUL Prime turn — the "after-action
//! narration" rung the audit deferred (`docs/prime-processing-audit.md` "the brain composes
//! its reply before the kernel executes, so an honest after-action narration needs a
//! post-execution re-shaping pass that preserves the action-free wall").
//!
//! ## Why this exists
//!
//! Every prior brain stage composes its reply BEFORE the kernel executes the turn, so the
//! action-free wall keeps an actionful turn's reply strictly deterministic
//! ([`crate::ai::is_actionful`] → `shape_reply` keeps it `DeterministicForAction`). The brain
//! could classify, sharpen slots, request a governed tool, and re-word a *conversational*
//! turn — but it could never phrase the confirmation a user reads AFTER a create / update /
//! assign / start / agent.create executes, or after a plugin.install / permission.grant is
//! proposed. That wording stayed a deterministic template.
//!
//! This module is the safe post-execution pass. Once the kernel has ALREADY executed (or
//! proposed) the action through the unchanged `decide` → [`crate::state::KernelState::prime_execute`]
//! / approval path, a configured brain may rephrase the final user-facing message — but
//! grounded ONLY in a sanitized [`ActionEnvelope`] of what truly happened, and validated
//! against it so the brain can never claim unexecuted work, invent an id, or contradict the
//! real status. The brain changes NOTHING: the action already ran; this only re-words the
//! confirmation, falling back to the grounded deterministic reply on any failure.
//!
//! ## The key difference from [`crate::prime_clarify`]
//!
//! The clarify/brainstorm path runs BEFORE execution, so it rejects EVERY completion claim
//! ([`crate::prime_clarify`] `ACTION_CLAIM_MARKERS`) — the brain there must never narrate a
//! state change. This path runs AFTER execution, so it does the INVERSE: a completion claim
//! is permitted ONLY when the envelope confirms that exact fact happened, and is rejected
//! otherwise (a claim of a kind that did not happen, a success claim on a failure, an
//! "installed"/"granted" claim on a still-pending proposal).
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/tool_executor.py` (L348-452) / `agent/conversation_loop.py`
//!   `run_conversation(...)` — after a tool executes, its result is appended back as a
//!   `{"role":"tool",...}` message carrying an `is_error` flag and a BOUNDED preview, and the
//!   model is re-called to produce the FINAL answer grounded in that real result. **Pattern:
//!   the final answer is grounded in the actual, bounded execution result (success vs.
//!   `is_error`), injected after the action ran.** Mirrored: [`build_after_action_prompt`]
//!   hands the brain the sanitized, bounded [`ActionEnvelope`] (kind = executed / proposed /
//!   failed) as the only ground truth, and the brain answers from it — but, unlike Hermes, the
//!   Relux brain executes nothing here: the action already ran deterministically.
//! - **Paperclip (openclaw)** `src/agents/bash-tools.exec-approval-followup.ts`
//!   `buildExecApprovalFollowupPrompt` (L64-82) / `buildExecDeniedFollowupPrompt` (L34-48) —
//!   the canonical "narrate the result after the approved action completed" prompt: it injects
//!   the "Exact completion details", and steers "if it succeeded, share the relevant output;
//!   if it failed, explain what went wrong", while the DENIED variant insists "the command did
//!   not run … do not claim there is new command output". **Pattern: ground the follow-up in
//!   the exact result, and distinguish succeeded / failed / did-not-run so the model never
//!   claims work that did not happen.** Mirrored exactly by the three [`ActionResultKind`]
//!   prompt variants (executed / failed / proposed).
//! - **openclaw** `src/agents/pi-embedded-helpers/sanitize-user-facing-text.ts`
//!   `sanitizeUserFacingText` — the result body shown to the user is sanitized before display.
//!   Mirrored in [`sanitize_block`] + [`redact_secrets`]: control chars stripped, length
//!   clamped, secret-shaped tokens and absolute paths masked.
//! - **openclaw** `src/agents/tools/sessions-spawn-tool.ts` (`UNSUPPORTED_*_PARAM_KEYS`) +
//!   `src/agents/tools/common.ts` (`readStringParam` required) — reject unsupported keys,
//!   require the mandatory string. [`parse_after_action`] accepts only the
//!   `text`/`confidence`/`rationale` allowlist and requires a non-empty `text`.
//!
//! ## The safety contract (binding)
//!
//! - **The action already ran; the brain changes nothing.** This stage only re-words the
//!   confirmation text. There is NO path from here to a mutation or an approval.
//! - **Grounded only in the sanitized envelope.** The brain sees the bounded, redacted result
//!   — never the raw provider envelope, secrets, or unbounded state.
//! - **No unexecuted work, no invented id, no contradiction.** A completion claim is honored
//!   ONLY when the envelope confirms that fact; a success claim on a failure is rejected; an
//!   "installed"/"granted"/"created"/"started" claim on a still-pending PROPOSAL is rejected
//!   (it must read as proposed / awaiting approval); an id-shaped token not in the envelope is
//!   rejected (an invented task/run/approval id).
//! - **Deterministic fallback always exists.** No brain, low confidence, malformed JSON, an
//!   unsupported field, a contradiction, an invented id, or a pure echo all fall back to the
//!   grounded deterministic reply with no provenance.

use relux_core::{PrimeAction, PrimeDisposition, PrimeTurn};

use crate::prime_intent::extract_json_object;

/// Minimum confidence before a brain's after-action wording is honored.
const CONFIDENCE_FLOOR: f32 = 0.6;
/// Max characters kept for the brain's after-action reply (a short confirmation, not an essay).
const MAX_REPLY_CHARS: usize = 600;
/// Max characters kept from the deterministic grounded reply folded into the envelope.
const MAX_GROUNDED_CHARS: usize = 600;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;
/// Mask substituted for a redacted secret-shaped token / absolute path.
const REDACTION: &str = "[redacted]";

/// The only fields an after-action wording proposal may carry. Any other key fails the
/// proposal closed (openclaw's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may not
/// smuggle an action/slot key in as authority.
const ALLOWED_KEYS: &[&str] = &["text", "confidence", "rationale"];

/// Generic success markers — rejected ONLY on a [`ActionResultKind::Failed`] envelope, so the
/// brain can never narrate a failed action as a success. Matched against the lowercased text.
const SUCCESS_MARKERS: &[&str] = &[
    "success",
    "succeeded",
    "completed successfully",
    "all set",
    "is done",
    "it's done",
    "its done",
    "worked",
    "finished successfully",
    "went through",
];

/// One group of completion-claim markers tied to a single durable fact. A marker in the group
/// is permitted ONLY when [`ActionFacts`] confirms that fact actually happened; otherwise the
/// whole reply is rejected (fail closed). The `installed` / `granted` groups are gated on facts
/// that are NEVER set on the execute path (a plugin install / permission grant is always an
/// approval-gated `Propose`, never executed by Prime), so they are rejected on every turn.
struct CompletionGroup {
    /// Lowercased phrases that assert this completed durable fact.
    markers: &'static [&'static str],
    /// Whether the envelope confirms this fact actually happened.
    confirmed: bool,
}

/// What actually happened this turn, derived purely from the executed [`PrimeTurn`]. Each
/// boolean gates the matching completion-claim group: a claim is honored only when its fact is
/// `true`. On a [`ActionResultKind::Proposed`] / `Failed` turn nothing executed, so every fact
/// is `false` and every completion claim is rejected (the brain must use proposal / honest
/// language).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActionFacts {
    pub task_created: bool,
    pub run_started: bool,
    pub agent_created: bool,
    pub task_updated: bool,
    pub task_assigned: bool,
}

/// Which kind of result the brain is narrating. Drives both the prompt steering and the
/// claim validation (executed may share the outcome; proposed must NOT claim completion;
/// failed must NOT claim success). Mirrors openclaw's succeeded / failed / did-not-run split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionResultKind {
    /// A safe action executed and changed durable state (disposition `Executed`).
    Executed,
    /// A risky action was proposed and is awaiting human approval (disposition
    /// `AwaitingApproval`) — it is NOT done.
    Proposed,
    /// An action was attempted but did not complete / failed. The brain must not claim
    /// success. (Live gating produces `Executed`/`Proposed`; this variant is validator-
    /// supported for an honest failure envelope.)
    Failed,
}

impl ActionResultKind {
    /// The short provenance label fragment ("executed" / "proposed" / "failed").
    pub fn label(self) -> &'static str {
        match self {
            ActionResultKind::Executed => "executed",
            ActionResultKind::Proposed => "proposed",
            ActionResultKind::Failed => "failed",
        }
    }
}

/// A sanitized, bounded projection of what a single actionful Prime turn actually did — the
/// ONLY ground truth handed to the after-action brain. It carries no raw provider envelope, no
/// secret, and no unbounded state: just the result kind, a short action label, the concrete
/// (sanitized) ids the action produced/targeted, the durable facts, and the redacted
/// deterministic reply the brain may rephrase. Built only by [`build_action_envelope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionEnvelope {
    /// Executed / proposed / failed — drives the prompt steering and the claim gating.
    pub kind: ActionResultKind,
    /// A short, human label of the action ("created a task", "proposed a plugin install").
    pub action_label: String,
    /// What durable facts are true, gating the completion-claim groups.
    pub facts: ActionFacts,
    /// The concrete, structured ids the action produced/targeted (`task_0001`, `run_0002`,
    /// `appr_0003`, an agent id), already sanitized. The validator rejects any structured
    /// id-shaped token in the brain reply that is NOT in this set (an invented id).
    pub ids: Vec<String>,
    /// The grounded deterministic reply (the truth the brain may rephrase), redacted + bounded.
    pub grounded_reply: String,
}

/// A validated after-action wording proposal. Only [`parse_after_action`] builds this, after
/// rejecting unknown fields, sanitizing + redacting the text, clamping length, and validating
/// every claim against the [`ActionEnvelope`]. The rationale is audit text only.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainAfterAction {
    pub text: String,
    pub confidence: f32,
    pub rationale: String,
}

/// Decide whether an actionful turn is eligible for post-execution reply shaping, and which
/// result kind it is. Returns `None` when the turn must keep its deterministic reply:
///
/// - a NON-actionful turn (those are shaped by the clarify / brainstorm / free-form paths);
/// - a TOOL turn (`invoked_tool` / `tool_output` / `tool_error` / `ToolDiscovery`): a tool
///   result is grounded in real kernel output and the long-standing wall keeps the brain from
///   ever narrating (and possibly overclaiming) it — preserved here;
/// - a high-risk action that is NOT a proposal (defensive: a plugin install / permission grant
///   is always an approval-gated `Propose`, so it can only be narrated as a proposal — never
///   as "installed"/"granted").
///
/// Otherwise: `AwaitingApproval` → [`ActionResultKind::Proposed`], `Executed` →
/// [`ActionResultKind::Executed`].
pub fn after_action_kind(turn: &PrimeTurn) -> Option<ActionResultKind> {
    if !crate::ai::is_actionful(turn) {
        return None;
    }
    // Preserve the tool-output wall: a tool turn keeps its grounded deterministic reply.
    if turn.invoked_tool.is_some()
        || turn.tool_output.is_some()
        || turn.tool_error.is_some()
        || matches!(turn.intent, relux_core::PrimeIntent::ToolDiscovery)
    {
        return None;
    }
    // An orchestration RUN turn's reply is the real, grounded batch result (per-brief
    // ran/completed/failed/blocked + the next action) — like a tool result, it is honest
    // kernel output the brain must not re-narrate and possibly overclaim. Keep it deterministic.
    if matches!(turn.action, Some(PrimeAction::RunOrchestration { .. })) {
        return None;
    }
    match turn.disposition {
        PrimeDisposition::AwaitingApproval => Some(ActionResultKind::Proposed),
        PrimeDisposition::Executed => {
            // A high-risk action that somehow reached Executed (it should always be a Propose)
            // is not narrated — only a proposal of it is. Defensive fail-closed.
            if is_high_risk_action(turn.action.as_ref()) {
                return None;
            }
            Some(ActionResultKind::Executed)
        }
        PrimeDisposition::Answered | PrimeDisposition::NeedsClarification => None,
    }
}

/// Whether an action is a high-risk, protected one (a plugin install or a permission grant).
/// These are only ever narrated as proposals; they are never claimed installed/granted.
fn is_high_risk_action(action: Option<&PrimeAction>) -> bool {
    matches!(
        action,
        Some(PrimeAction::InstallPlugin { .. }) | Some(PrimeAction::GrantPermission { .. })
    )
}

/// Build the sanitized [`ActionEnvelope`] for an actionful turn + its [`ActionResultKind`].
/// Pure: derives the facts/ids/label/grounded-reply from the already-executed turn, with the
/// grounded reply control-stripped, secret-redacted, and length-clamped.
pub fn build_action_envelope(turn: &PrimeTurn, kind: ActionResultKind) -> ActionEnvelope {
    let executed = kind == ActionResultKind::Executed;

    let mut ids: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let id = sanitize_id(raw);
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    };
    if let Some(t) = turn.created_task.as_ref() {
        push(&t.0);
    }
    if let Some(r) = turn.started_run.as_ref() {
        push(&r.0);
    }
    if let Some(a) = turn.approval.as_ref() {
        push(&a.0);
    }
    if let Some(a) = turn.created_agent.as_ref() {
        push(&a.0);
    }
    // The action's TARGET ids (an update/assign/start references an existing id not minted here).
    match turn.action.as_ref() {
        Some(PrimeAction::UpdateTask { task_id, .. }) => push(task_id),
        Some(PrimeAction::AssignTask { task_id, agent_id }) => {
            push(task_id);
            push(agent_id);
        }
        Some(PrimeAction::StartRun { task_id }) => push(task_id),
        Some(PrimeAction::InstallPlugin { plugin_id }) => push(plugin_id),
        Some(PrimeAction::GrantPermission { subject_id, .. }) => push(subject_id),
        _ => {}
    }

    // Facts are true ONLY on an executed turn; a proposed/failed turn executed nothing, so every
    // completion claim is gated off (the cards below are absent on those turns anyway).
    let facts = ActionFacts {
        task_created: executed && turn.created_task.is_some(),
        run_started: executed && turn.started_run.is_some(),
        agent_created: executed && turn.created_agent.is_some(),
        task_updated: executed && matches!(turn.action, Some(PrimeAction::UpdateTask { .. })),
        task_assigned: executed && matches!(turn.action, Some(PrimeAction::AssignTask { .. })),
    };

    ActionEnvelope {
        kind,
        action_label: action_label(turn, kind),
        facts,
        ids,
        grounded_reply: redact_secrets(&sanitize_block(&turn.reply, MAX_GROUNDED_CHARS)),
    }
}

/// A short, human description of what the action did, grounded in the executed turn. Used in the
/// prompt so the brain knows exactly what to confirm. Never invents detail beyond the turn.
fn action_label(turn: &PrimeTurn, kind: ActionResultKind) -> String {
    match turn.action.as_ref() {
        Some(PrimeAction::CreateTask { .. }) => "created a task".to_string(),
        Some(PrimeAction::CreateAndRunTask { .. }) => {
            if turn.started_run.is_some() {
                "created a task and started its run".to_string()
            } else {
                "created a task".to_string()
            }
        }
        Some(PrimeAction::UpdateTask { .. }) => "updated a task".to_string(),
        Some(PrimeAction::AssignTask { .. }) => "assigned a task to an agent".to_string(),
        Some(PrimeAction::StartRun { .. }) => "started a run".to_string(),
        Some(PrimeAction::RetryRun { .. }) => "retried a run".to_string(),
        Some(PrimeAction::CreateAgent { .. }) => "created an operative (agent)".to_string(),
        Some(PrimeAction::OrchestrateGoal { .. }) => "created a set of tasks".to_string(),
        Some(PrimeAction::InstallPlugin { .. }) => match kind {
            ActionResultKind::Proposed => {
                "proposed installing a plugin (awaiting your approval)".to_string()
            }
            _ => "installed a plugin".to_string(),
        },
        Some(PrimeAction::GrantPermission { .. }) => match kind {
            ActionResultKind::Proposed => {
                "proposed granting a permission (awaiting your approval)".to_string()
            }
            _ => "granted a permission".to_string(),
        },
        _ => match kind {
            ActionResultKind::Proposed => "proposed an action (awaiting your approval)".to_string(),
            ActionResultKind::Failed => "attempted an action".to_string(),
            ActionResultKind::Executed => "completed an action".to_string(),
        },
    }
}

/// The strict, self-contained prompt handed to a brain to narrate ONE already-executed (or
/// proposed/failed) action. It pins Prime's identity, hands the brain the exact sanitized
/// result as the only ground truth, steers succeeded / proposed / failed wording (the openclaw
/// follow-up split), forbids inventing ids or claiming unexecuted work, and demands JSON-only
/// output. Kept ASCII + self-contained so it works as a one-shot CLI stdin prompt.
pub fn build_after_action_prompt(message: &str, envelope: &ActionEnvelope) -> String {
    let ids = if envelope.ids.is_empty() {
        "(none)".to_string()
    } else {
        envelope.ids.join(", ")
    };
    let common = "You are Prime, a general-purpose local AI agent narrating an action you just \
took on a local Relux control plane at the user's request. Use plain ASCII. Do NOT invent any \
task id, run id, approval id, plugin, agent, or number. Only mention an id that appears in the \
result below.";
    let steer = match envelope.kind {
        ActionResultKind::Executed => {
            "The action below has ALREADY been performed — it is DONE. Write a concise, natural \
confirmation for the user, grounded ONLY in the exact result. You MAY mention the ids shown. Do \
NOT claim any additional action that the result does not show (for example, do not say a run \
started unless the result says one did). Stay consistent with the grounded reply; you may phrase \
it better but must not contradict it."
        }
        ActionResultKind::Proposed => {
            "The action below was PROPOSED and is NOT done — it is waiting for the user's human \
approval. Never say it is installed, granted, created, started, enabled, or done. Say it was \
proposed / requested and needs the user's approval before anything changes. Be concise and honest."
        }
        ActionResultKind::Failed => {
            "The action below DID NOT complete. Do NOT claim success or that anything changed. \
Briefly and honestly tell the user it did not go through, consistent with the grounded reply. Do \
not invent a reason the result does not give."
        }
    };
    format!(
        "{common}\n\n{steer}\n\nRespond with JSON ONLY (no prose, no code fences) in EXACTLY this \
shape:\n{{\"text\":\"<a concise, natural confirmation>\",\"confidence\":<0.0-1.0>}}\n\n\
Rules:\n\
- text: a SHORT, natural confirmation (a sentence or two). No lecture, no preamble.\n\
- Do NOT add any field other than text and confidence.\n\n\
Exact result:\n\
- status: {status}\n\
- action: {action}\n\
- ids: {ids}\n\
- grounded reply (the truth you may rephrase): {grounded}\n\n\
User message:\n{message}",
        common = common,
        steer = steer,
        status = envelope.kind.label(),
        action = envelope.action_label,
        ids = ids,
        grounded = envelope.grounded_reply,
        message = message,
    )
}

/// Parse a brain's raw reply into a validated [`BrainAfterAction`], or `Err` with a short reason
/// on anything malformed/unsupported/contradicting. The schema/allowlist + claim gate.
///
/// After the allowlist + sanitize + redact + clamp, every CLAIM is validated against the
/// `envelope`: a completion claim is honored ONLY when its fact is confirmed; an
/// `installed`/`granted` claim is always rejected (never executed by Prime); a success claim on
/// a `Failed` envelope is rejected; and any structured id-shaped token not in `envelope.ids`
/// fails the reply closed (an invented id).
pub fn parse_after_action(raw: &str, envelope: &ActionEnvelope) -> Result<BrainAfterAction, String> {
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

    // Sanitize control chars + clamp, then mask secret-shaped tokens / absolute paths.
    let text = redact_secrets(&sanitize_block(raw_text, MAX_REPLY_CHARS));
    if text.is_empty() {
        return Err("empty text".to_string());
    }

    let lowered = text.to_lowercase();

    // Completion-claim gating: each group is permitted ONLY when its fact is confirmed.
    let groups = completion_groups(&envelope.facts);
    for group in &groups {
        if !group.confirmed && group.markers.iter().any(|m| lowered.contains(m)) {
            return Err("text claims an action that did not happen".to_string());
        }
    }

    // A failure must never be narrated as a success.
    if envelope.kind == ActionResultKind::Failed
        && SUCCESS_MARKERS.iter().any(|m| lowered.contains(m))
    {
        return Err("text claims success on a failed action".to_string());
    }

    // No invented structured id: any task_/run_/appr_/approval_ token must be in the envelope.
    if let Some(bad) = unknown_structured_id(&text, &envelope.ids) {
        return Err(format!("text references an unknown id '{bad}'"));
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

    Ok(BrainAfterAction {
        text,
        confidence,
        rationale,
    })
}

/// Reconcile a validated after-action proposal against the deterministic reply, returning the
/// wording to show, or `None` to keep the deterministic reply (low confidence, or a pure echo —
/// no point attributing a brain for a no-op). The claim/contradiction validation already ran in
/// [`parse_after_action`]; this is the confidence / echo gate.
pub fn reconcile_after_action(
    deterministic_reply: &str,
    proposal: &BrainAfterAction,
) -> Option<String> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }
    if proposal
        .text
        .trim()
        .eq_ignore_ascii_case(deterministic_reply.trim())
    {
        return None;
    }
    Some(proposal.text.clone())
}

/// Build the completion-claim groups for the given facts. The `installed` / `granted` groups
/// are NEVER confirmed (Prime never executes an install/grant — they are always approval-gated
/// proposals), so an "installed"/"granted" claim is rejected on every turn.
fn completion_groups(facts: &ActionFacts) -> Vec<CompletionGroup> {
    vec![
        CompletionGroup {
            markers: &[
                "created a task",
                "created the task",
                "task created",
                "added a task",
                "added the task",
                "made a task",
                "logged a task",
                "new task is now",
            ],
            confirmed: facts.task_created,
        },
        CompletionGroup {
            markers: &[
                "started the run",
                "started a run",
                "run started",
                "kicked off",
                "now running",
                "started running",
                "launched the run",
                "run is underway",
            ],
            confirmed: facts.run_started,
        },
        CompletionGroup {
            markers: &[
                "created an agent",
                "created the agent",
                "created an operative",
                "new operative",
                "added an agent",
                "onboarded",
            ],
            confirmed: facts.agent_created,
        },
        CompletionGroup {
            markers: &[
                "updated the task",
                "updated task",
                "changed the task",
                "applied the update",
                "renamed the task",
                "marked the task",
            ],
            confirmed: facts.task_updated,
        },
        CompletionGroup {
            markers: &["assigned the task", "assigned task", "now assigned to", "assigned it to"],
            confirmed: facts.task_assigned,
        },
        // Never confirmed: Prime never executes an install / grant (always an approval).
        CompletionGroup {
            markers: &["installed the plugin", "is now installed", "plugin is installed", "i installed", "i've installed", "ive installed"],
            confirmed: false,
        },
        CompletionGroup {
            markers: &["permission granted", "i granted", "i've granted", "ive granted", "now has access", "access granted", "grant is in effect"],
            confirmed: false,
        },
    ]
}

/// Return the first structured id-shaped token (`task_…`, `run_…`, `appr_…`, `approval_…`) in
/// `text` that is NOT in `allowed` (lowercased), or `None` when every such token is known. This
/// is the invented-id check; agent ids are free-form slugs and are not matched here (the
/// completion-claim gating already guards against claiming an agent that was not created).
fn unknown_structured_id(text: &str, allowed: &[String]) -> Option<String> {
    let allowed_lc: Vec<String> = allowed.iter().map(|s| s.to_lowercase()).collect();
    for raw in text.split(|c: char| c.is_whitespace()) {
        let token = trim_token(raw).to_lowercase();
        if is_structured_id(&token) && !allowed_lc.contains(&token) {
            return Some(token);
        }
    }
    None
}

/// Whether a token looks like a structured kernel id: a known prefix + `_` + an alphanumeric
/// tail (`task_0001`, `run_0002`, `appr_0003`, `approval_0003`).
fn is_structured_id(token: &str) -> bool {
    let Some((prefix, tail)) = token.split_once('_') else {
        return false;
    };
    if !matches!(prefix, "task" | "run" | "appr" | "approval") {
        return false;
    }
    !tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Strip leading/trailing punctuation around a token so `"task_0001."` / `"(run_0002)"` still
/// match. Keeps inner `_`/`-` and alphanumerics.
fn trim_token(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
}

/// Normalize a concrete id for the allowed set: control-stripped, trimmed, length-clamped. Ids
/// are short and well-formed; this only guards a pathological value.
fn sanitize_id(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !c.is_whitespace())
        .take(80)
        .collect()
}

/// Mask secret-shaped tokens and absolute filesystem paths in a user-facing string, replacing
/// each with [`REDACTION`]. Conservative + dependency-free (token scan, no regex): it targets
/// known secret prefixes, high-entropy opaque blobs, and absolute unix/windows paths — never an
/// ordinary word. Mirrors openclaw's `sanitizeUserFacingText` redaction posture.
pub fn redact_secrets(s: &str) -> String {
    s.split(' ')
        .map(|tok| {
            if token_is_sensitive(tok) {
                REDACTION
            } else {
                tok
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a whitespace-delimited token should be redacted. A token is sensitive when it carries
/// a known secret prefix, is a long high-entropy opaque blob, or is an absolute path.
fn token_is_sensitive(tok: &str) -> bool {
    let core = trim_token_keep_path(tok);
    if core.is_empty() {
        return false;
    }
    let lower = core.to_ascii_lowercase();
    // Known secret prefixes (API keys / tokens).
    const SECRET_PREFIXES: &[&str] = &[
        "sk-",
        "sk_live_",
        "sk_test_",
        "rk_live_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxs-",
        "akia",
        "asia",
        "aiza",
        "bearer",
    ];
    if SECRET_PREFIXES.iter().any(|p| lower.starts_with(p)) && core.len() >= 8 {
        return true;
    }
    // An absolute filesystem path: unix `/a/b` or windows `C:\a\b` / `C:/a/b`.
    if is_absolute_path(core) {
        return true;
    }
    // A long, opaque, high-entropy blob (key-like): long, alnum-ish, mixes letters + digits.
    if core.len() >= 40 && looks_like_blob(core) {
        return true;
    }
    false
}

/// Whether a token is an absolute filesystem path.
fn is_absolute_path(tok: &str) -> bool {
    if tok.starts_with('/') && tok.len() > 1 && tok[1..].contains('/') {
        return true;
    }
    // Windows drive path: `C:\...` or `C:/...`.
    let bytes = tok.as_bytes();
    if bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    false
}

/// Whether a token is a long, opaque, key-like blob: only url-safe-base64/hex chars and a mix of
/// at least one letter and one digit (so an ordinary long word or a sentence fragment is not hit).
fn looks_like_blob(tok: &str) -> bool {
    let mut has_alpha = false;
    let mut has_digit = false;
    for c in tok.chars() {
        if c.is_ascii_alphabetic() {
            has_alpha = true;
        } else if c.is_ascii_digit() {
            has_digit = true;
        } else if !matches!(c, '+' | '/' | '=' | '_' | '-') {
            return false;
        }
    }
    has_alpha && has_digit
}

/// Trim surrounding punctuation but KEEP path/secret characters (`/`, `\`, `:`, `.`, `_`, `-`,
/// `+`, `=`) so an absolute path / token is detected intact.
fn trim_token_keep_path(s: &str) -> &str {
    s.trim_matches(|c: char| {
        !c.is_ascii_alphanumeric()
            && !matches!(c, '/' | '\\' | ':' | '.' | '_' | '-' | '+' | '=')
    })
}

/// Sanitize a single-line string: control chars → space, collapse whitespace, trim, clamp.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect::<String>().trim().to_string()
}

/// Sanitize a multi-line block: drop control chars except `\n`, collapse intra-line whitespace,
/// drop blank lines, trim, clamp. Shared shape with [`crate::prime_clarify`].
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
    use relux_core::{ApprovalId, PrimeAction, PrimeDisposition, PrimeIntent, TaskId};

    /// A minimal turn builder for the tests. `StateSummary` is not needed here.
    fn base_turn() -> PrimeTurn {
        PrimeTurn {
            intent: PrimeIntent::TaskCreation,
            reply: "Created task_0001.".to_string(),
            disposition: PrimeDisposition::Executed,
            action: None,
            created_task: None,
            started_run: None,
            created_agent: None,
            approval: None,
            invoked_tool: None,
            tool_output: None,
            tool_error: None,
            suggested_actions: vec![],
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
            assign_slots: None,
            update: None,
            context_reads: vec![],
            tool_plan_proposal: None,
            pending_tool_approval: None,
            tool_trace: vec![],
        }
    }

    fn executed_create() -> PrimeTurn {
        let mut t = base_turn();
        t.intent = PrimeIntent::TaskCreation;
        t.action = Some(PrimeAction::CreateTask {
            title: "Fix the login redirect".to_string(),
        });
        t.created_task = Some(TaskId::new("task_0001".to_string()));
        t.reply = "Created task_0001: Fix the login redirect.".to_string();
        t
    }

    fn proposed_install() -> PrimeTurn {
        let mut t = base_turn();
        t.intent = PrimeIntent::PluginInstallation;
        t.disposition = PrimeDisposition::AwaitingApproval;
        t.action = Some(PrimeAction::InstallPlugin {
            plugin_id: "relux-tools-github".to_string(),
        });
        t.approval = Some(ApprovalId::new("appr_0001".to_string()));
        t.reply = "Installing a plugin needs your approval (appr_0001).".to_string();
        t
    }

    #[test]
    fn after_action_kind_gates_executed_proposed_and_skips_the_rest() {
        // Executed safe action -> Executed.
        assert_eq!(
            after_action_kind(&executed_create()),
            Some(ActionResultKind::Executed)
        );
        // Awaiting approval -> Proposed.
        assert_eq!(
            after_action_kind(&proposed_install()),
            Some(ActionResultKind::Proposed)
        );
        // A non-actionful (Answered) turn is not shaped here.
        let mut chat = base_turn();
        chat.disposition = PrimeDisposition::Answered;
        chat.reply = "Hi!".to_string();
        assert_eq!(after_action_kind(&chat), None);
        // A tool turn keeps its deterministic reply (the tool-output wall).
        let mut tool = executed_create();
        tool.invoked_tool = Some("relux-tools-echo/echo.say".to_string());
        assert_eq!(after_action_kind(&tool), None);
        // A high-risk action that somehow executed is not narrated (defensive).
        let mut bad = base_turn();
        bad.disposition = PrimeDisposition::Executed;
        bad.action = Some(PrimeAction::GrantPermission {
            subject_id: "researcher".to_string(),
            permission: "tool:x:y".to_string(),
        });
        assert_eq!(after_action_kind(&bad), None);
    }

    #[test]
    fn envelope_collects_ids_facts_and_redacts_the_grounded_reply() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        assert_eq!(env.kind, ActionResultKind::Executed);
        assert!(env.facts.task_created);
        assert!(!env.facts.run_started);
        assert!(env.ids.contains(&"task_0001".to_string()));
        assert!(env.action_label.contains("created a task"));

        // A proposed install carries the approval id and an awaiting-approval label, no facts.
        let env = build_action_envelope(&proposed_install(), ActionResultKind::Proposed);
        assert!(!env.facts.task_created && !env.facts.run_started);
        assert!(env.ids.contains(&"appr_0001".to_string()));
        assert!(env.action_label.contains("awaiting your approval"));
    }

    #[test]
    fn accepts_a_valid_executed_confirmation() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        let p = parse_after_action(
            r#"{"text":"Done - I created task_0001 to fix the login redirect.","confidence":0.9}"#,
            &env,
        )
        .unwrap();
        assert!(p.text.contains("task_0001"));
        assert_eq!(
            reconcile_after_action("Created task_0001: Fix the login redirect.", &p).as_deref(),
            Some("Done - I created task_0001 to fix the login redirect.")
        );
    }

    #[test]
    fn rejects_a_claim_of_an_action_that_did_not_happen() {
        // A create executed but NO run started -> claiming a run started is rejected.
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        assert!(parse_after_action(
            r#"{"text":"Created task_0001 and started the run.","confidence":0.95}"#,
            &env
        )
        .is_err());
    }

    #[test]
    fn proposed_install_must_not_say_installed() {
        let env = build_action_envelope(&proposed_install(), ActionResultKind::Proposed);
        // Claiming the plugin is installed on a still-pending proposal is rejected.
        assert!(parse_after_action(
            r#"{"text":"I installed the plugin for you.","confidence":0.95}"#,
            &env
        )
        .is_err());
        assert!(parse_after_action(
            r#"{"text":"The plugin is now installed.","confidence":0.95}"#,
            &env
        )
        .is_err());
        // Proposal-language is accepted.
        let ok = parse_after_action(
            r#"{"text":"I've proposed installing relux-tools-github - it needs your approval (appr_0001).","confidence":0.9}"#,
            &env,
        )
        .unwrap();
        assert!(ok.text.to_lowercase().contains("approval"));
    }

    #[test]
    fn failed_action_must_not_claim_success() {
        let mut t = executed_create();
        t.reply = "I could not create the task - the title was empty.".to_string();
        let env = build_action_envelope(&t, ActionResultKind::Failed);
        assert!(parse_after_action(
            r#"{"text":"Success - the task was completed successfully.","confidence":0.95}"#,
            &env
        )
        .is_err());
        // An honest failure narration is accepted.
        let ok = parse_after_action(
            r#"{"text":"That did not go through - the title was empty, so no task was made.","confidence":0.9}"#,
            &env,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn rejects_an_invented_id() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        // task_0001 is the only real id; a different one is an invented id.
        assert!(parse_after_action(
            r#"{"text":"Created task_9999 for you.","confidence":0.9}"#,
            &env
        )
        .is_err());
        // An invented run/approval id is likewise rejected.
        assert!(parse_after_action(
            r#"{"text":"Created task_0001; run_4242 is underway.","confidence":0.9}"#,
            &env
        )
        .is_err());
    }

    #[test]
    fn strips_envelope_noise_clamps_and_redacts_secrets() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        // Control chars (tab, carriage return) are stripped; a newline survives only as a line
        // separator in the short block, never as a raw control char run.
        let p = parse_after_action(
            "{\"text\":\"Created\\ttask_0001\\r for you.\",\"confidence\":0.9}",
            &env,
        )
        .unwrap();
        assert!(!p.text.contains('\t') && !p.text.contains('\r'));
        assert_eq!(p.text, "Created task_0001 for you.");

        // A secret-shaped token is redacted out of the brain reply.
        let p = parse_after_action(
            r#"{"text":"Created task_0001 with key sk-ABCDEF0123456789ABCDEF for the run.","confidence":0.9}"#,
            &env,
        )
        .unwrap();
        assert!(p.text.contains("[redacted]"));
        assert!(!p.text.contains("sk-ABCDEF0123456789ABCDEF"));

        // An absolute path is redacted.
        let p = parse_after_action(
            r#"{"text":"Created task_0001; logs at /home/alice/.config/relux/secret.toml now.","confidence":0.9}"#,
            &env,
        )
        .unwrap();
        assert!(p.text.contains("[redacted]"));
        assert!(!p.text.contains("/home/alice/.config"));
    }

    #[test]
    fn rejects_unsupported_fields_and_empty_text() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        assert!(parse_after_action("not json", &env).is_err());
        // A smuggled action/authority key fails the whole proposal closed.
        assert!(parse_after_action(
            r#"{"text":"Created task_0001.","run":true,"confidence":0.9}"#,
            &env
        )
        .is_err());
        assert!(parse_after_action(r#"{"confidence":0.9}"#, &env).is_err());
        assert!(parse_after_action(r#"{"text":"   ","confidence":0.9}"#, &env).is_err());
    }

    #[test]
    fn reconcile_drops_low_confidence_and_pure_echo() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        let low = parse_after_action(
            r#"{"text":"Created task_0001 to fix the login.","confidence":0.3}"#,
            &env,
        )
        .unwrap();
        assert!(reconcile_after_action("anything", &low).is_none());
        // A pure echo of the deterministic reply attributes no brain.
        let echo = parse_after_action(
            r#"{"text":"Created task_0001: Fix the login redirect.","confidence":0.9}"#,
            &env,
        )
        .unwrap();
        assert!(
            reconcile_after_action("created task_0001: fix the login redirect.", &echo).is_none()
        );
    }

    #[test]
    fn prompt_carries_the_result_and_the_right_steer() {
        let env = build_action_envelope(&executed_create(), ActionResultKind::Executed);
        let prompt = build_after_action_prompt("create a task to fix login", &env);
        assert!(prompt.contains("JSON ONLY"));
        assert!(prompt.contains("ALREADY"));
        assert!(prompt.contains("task_0001"));
        assert!(prompt.contains("created a task"));

        let env = build_action_envelope(&proposed_install(), ActionResultKind::Proposed);
        let prompt = build_after_action_prompt("install the github plugin", &env);
        assert!(prompt.contains("PROPOSED") || prompt.contains("waiting for the user's human approval"));
        assert!(prompt.contains("Never say it is installed"));
    }
}
