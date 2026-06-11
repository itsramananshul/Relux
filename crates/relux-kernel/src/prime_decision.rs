//! The UNIFIED Prime brain decision envelope — one provider call that carries the
//! intent + every validated slot proposal + optional clarifying wording for ONE Prime
//! turn, instead of the prior fragmented stack of separate intent / slot / wording
//! calls.
//!
//! ## Why this exists
//!
//! Prime's brain stack grew one specialized call at a time: intent
//! ([`crate::prime_intent`]), then task slots ([`crate::prime_slots`]), agent slots
//! ([`crate::prime_agent_slots`]), admin slots ([`crate::prime_admin_slots`]),
//! assignment slots ([`crate::prime_assign_slots`]), update slots
//! ([`crate::prime_update_slots`]), and clarify wording ([`crate::prime_clarify`]).
//! Each is correct and fail-closed, but a single Prime turn could fire the brain TWO or
//! THREE times in series (intent, then slots for the resolved intent, then wording for a
//! clarify). That is slow, costly, and — worse — less coherent than how Hermes / Codex /
//! Claude actually work: ONE model response carries both the answer and the structured
//! actions in a single turn (`docs/RELUX_MASTER_PLAN.md` §10.1, §17.1).
//!
//! This module adds that one-shot shape. A configured brain may return a single JSON
//! envelope carrying any subset of: an intent classification, task / agent / plugin /
//! permission / assignment / update slots, and a clarifying-wording proposal. The kernel
//! still validates and executes EXACTLY as before — every section is run through its
//! existing validator, the fail-closed intent gate is unchanged, and every durable change
//! still flows through `decide` → `prime_execute`. The brain authors a *proposal*; it
//! runs nothing.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/conversation_loop.py` `run_conversation(...)` — a SINGLE model
//!   response carries both `content` (the answer) and `tool_calls` (the structured
//!   actions) in one assistant message (`_m.get("tool_calls")`, ~L630-875); the loop then
//!   validates the chosen tool against the name allowlist BEFORE acting (~L3116-3162). We
//!   mirror the *one response carries everything* shape: [`parse_decision`] lifts one
//!   envelope that carries the intent AND the slots AND the wording, and each piece is
//!   validated against its existing allowlist before it can shape anything. The brain
//!   never executes — unlike Hermes, every Relux durable change still flows through the
//!   deterministic kernel path.
//! - **openclaw** `src/shared/balanced-json.ts` `extractBalancedJsonPrefix` and
//!   `src/agents/cli-output.ts` `parseCliOutput` — lift the first balanced `{...}` out of
//!   a noisy reply and surface only the parsed object, never the raw stdout. We reuse the
//!   SAME scanner ([`crate::prime_intent::extract_json_object`]); on the CLI path the
//!   server runs `parse_adapter_result` FIRST so the raw `--output-format json` envelope
//!   never reaches this parser or the UI.
//! - **openclaw** `src/agents/tools/update-plan-tool.ts` `readPlanSteps` (L39-74) — a
//!   structured payload is validated FIELD-BY-FIELD and COMPOSITIONALLY (each plan step is
//!   checked independently against its schema + status allowlist; a bad one is an input
//!   error). We adopt the compositional shape: [`parse_decision`] rejects any UNKNOWN
//!   top-level key outright (fail the whole envelope closed — the brain may not smuggle an
//!   un-modeled authority key), then validates each KNOWN section through its own existing
//!   validator; an invalid nested section is DROPPED (that section falls back to its
//!   specialized path / the deterministic rail) while the rest of the envelope stands.
//!
//! ## The safety contract (binding)
//!
//! The unified envelope changes only HOW the brain is *asked* (one call) and HOW its reply
//! is *parsed* (one object, strictly allowlisted). It changes nothing about authority:
//!
//! - Each section reuses the SAME validator the specialized path uses — no weaker
//!   duplicate logic. A task section is [`crate::prime_slots::parse_task_slots`]; intent is
//!   [`crate::prime_intent::parse_intent_proposal`]; etc.
//! - The fail-closed intent gate ([`crate::prime_intent::reconcile_intent`]) still runs at
//!   the kernel chokepoint, so guarded chat can never be promoted to work.
//! - Slots are still reconciled against the live state at the kernel chokepoint
//!   ([`crate::KernelState::prime_turn_with_brain`]); a slot for a section that does not
//!   match the resolved action is simply ignored.
//! - The raw provider envelope NEVER leaks: only the parsed, validated, sanitized fields
//!   survive; on any failure the caller falls back to the specialized paths and the
//!   deterministic rails.

use crate::prime_intent::extract_json_object;
use relux_core::StateSummary;

/// Max characters kept from the brain's free-text provenance note. Audit/provenance only.
const MAX_PROVENANCE_CHARS: usize = 240;

/// Confidence stamped on a bare-string `reply` so the downstream brainstorm chokepoint (which
/// defaults a missing confidence to 0.5, below its 0.6 honor floor) does not silently drop a
/// deliberately-simple committed reply. Kept just above the floor — an object reply carries its
/// own confidence and is never re-stamped.
const BARE_REPLY_CONFIDENCE: f32 = 0.7;

/// Bounded catalogs in the grounding prompt so a brain can resolve an assignment / update
/// by description against REAL ids (the kernel still validates every id).
const MAX_PROMPT_TASKS: usize = 12;
const MAX_PROMPT_AGENTS: usize = 12;

/// The bounded observe-then-act decision budget: how many unified decision calls ONE Prime turn
/// may make. Each round the brain either requests read-only context tools (observe) or commits its
/// decision (act / answer). Kept small because each round is a FULL decision call — the loop exists
/// to let the brain inspect a *little* live state before choosing its one action, not to run an
/// open-ended agent. This is the Hermes `run_conversation` max-iterations cap
/// (`reference/hermes-agent-main/agent/conversation_loop.py` ~L598) applied to the decision call.
pub const MAX_DECISION_ROUNDS: usize = 3;

/// The only top-level keys a unified decision envelope may carry. Any other top-level key
/// fails the WHOLE envelope closed (openclaw's `additionalProperties: false` discipline):
/// the brain may not smuggle an un-modeled authority key past the parser. Nested-section
/// validation is delegated to each section's own validator.
const ALLOWED_TOP_LEVEL_KEYS: &[&str] = &[
    "classification",
    "task",
    "agent",
    "plugin",
    "permission",
    "assign",
    "update",
    "wording",
    // Read-only context tools the brain wants the kernel to run BEFORE it answers — the unified
    // counterpart of the standalone read-only loop. `context_reads` is accepted as an alias.
    "tool_requests",
    "context_reads",
    // The single WRITE-capable tool the brain requests this turn (a mutating action mapped to an
    // existing safe `Act` / approval-gated `Propose`). At most one per turn. `tool_call` alias.
    "action_request",
    "tool_call",
    // The free-form conversational reply for a non-clarify chat turn (greeting / direct
    // answer / explanation). `assistant_message` is accepted as an alias for the same field.
    "reply",
    "assistant_message",
    // The advisory presentation polish for a multi-step plan-preview card (wording only).
    "plan_polish",
    "confidence",
    "rationale",
    "source",
    "provenance",
];

/// A unified decision a brain *proposes* for one Prime turn, with every section already
/// validated through its existing specialized validator. Only [`parse_decision`] builds
/// this. Every field is optional: the brain includes only the sections that apply, and the
/// kernel uses only the sections that match the turn it actually produces.
///
/// This is presentation/proposal data — it executes nothing. The kernel re-validates and
/// applies each section at its single chokepoint exactly as it does for the specialized
/// paths.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrimeBrainDecision {
    /// The proposed intent (validated against the `PrimeIntent` allowlist + clamped). Fed
    /// to the unchanged fail-closed [`crate::prime_intent::reconcile_intent`] gate.
    pub classification: Option<crate::prime_intent::BrainIntentProposal>,
    /// Task slots for a create turn.
    pub task: Option<crate::prime_slots::BrainTaskSlots>,
    /// Agent slots for an `AgentCreation` turn.
    pub agent: Option<crate::prime_agent_slots::BrainAgentSlots>,
    /// Advisory plugin reference for a `PluginInstallation` `Propose` turn.
    pub plugin: Option<crate::prime_admin_slots::BrainPluginRef>,
    /// Advisory permission subject for a `PermissionChange` `Propose` turn.
    pub permission: Option<crate::prime_admin_slots::BrainPermissionSlots>,
    /// Assignment slots for an `AssignTask` turn the deterministic extractors missed.
    pub assign: Option<crate::prime_assign_slots::BrainAssignSlots>,
    /// By-id update slots for a `TaskUpdate` turn the deterministic rail could not resolve.
    pub update: Option<crate::prime_update_slots::BrainUpdateSlots>,
    /// Read-only context tool requests the brain made in this ONE unified decision envelope,
    /// already validated against the read-only allowlist
    /// ([`crate::prime_tools::validate_tool_request`]): a mutating / unknown / made-up tool name is
    /// dropped at parse time and NEVER executed. On a non-actionful inspection turn the server runs
    /// these deterministically against the live snapshot
    /// ([`crate::prime_tools::execute_requested_reads`]) and grounds the reply in the observations —
    /// instead of running the separate sidecar tool loop (no duplicate execution). Empty when the
    /// brain requested no context tools (the sidecar [`crate::prime_tools::ContextLoop`] is then the
    /// fallback). Bounded by the loop's round cap at parse time.
    pub context_requests: Vec<crate::prime_tools::ToolCall>,
    /// The single WRITE-capable tool the brain requested this turn, already validated against the
    /// write allowlist + mapped to an EXISTING action's slot
    /// ([`crate::prime_write_tools::parse_write_tool_request`]): a mutating / unknown / made-up tool
    /// name (`task.delete`, `shell.run`) is dropped at parse time and can NEVER act, and a batched
    /// multi-tool request is refused (at most ONE mutating tool per turn). On an explicitly-commanded
    /// turn the kernel feeds this tool's synthesized intent through the fail-closed gate and its
    /// validated slot through the unchanged chokepoint, so casual chat can never trigger it and every
    /// existence/approval gate still applies. `None` when the brain requested no write tool.
    pub action_request: Option<crate::prime_write_tools::ParsedWriteTool>,
    /// The raw wording sub-object (`{text, confidence, rationale?}`) re-serialized to JSON,
    /// NOT yet validated against a `ClarifyKind`. It is validated and reconciled later via
    /// [`Self::validated_wording`] against the turn's actual kind + deterministic text
    /// (reusing [`crate::prime_clarify`]), because the eligible kind is known only after the
    /// kernel produces the turn. `None` when the brain proposed no wording.
    pub wording: Option<String>,
    /// The raw free-form conversational reply sub-object (`{text, confidence, rationale?}`)
    /// re-serialized to JSON, NOT yet validated. It is validated later via
    /// [`Self::validated_reply`] — through the SAME block-sanitize + action-claim chokepoint a
    /// brainstorm reply uses ([`crate::prime_clarify::parse_clarify`] with
    /// [`crate::prime_clarify::ClarifyKind::Brainstorm`]) — and applied ONLY on a non-actionful,
    /// non-clarify conversational turn (greeting / direct answer / explanation). Carried raw
    /// because eligibility is known only after the kernel produces the turn. `None` when the
    /// brain proposed no reply. A brain that emits a bare string is normalized to `{text:...}`.
    pub reply: Option<String>,
    /// The raw plan-polish sub-object (`{summary?, steps?, questions?, risks?}`) re-serialized to
    /// JSON, NOT yet validated against an authoritative proposal. It is validated later via
    /// [`Self::validated_polish`] through the SAME [`crate::ai::polish_from_cli_text`] →
    /// `validate_polish` chokepoint the specialized polish uses, so it can change only the
    /// wording (summary / step titles / questions / risks) and NEVER the step count, order, or
    /// agent ids — and a step title is kept only when its index matches the authoritative step
    /// exactly. Carried raw because the authoritative steps exist only after the kernel produces
    /// the turn. `None` when the brain proposed no polish.
    pub plan_polish: Option<String>,
    /// The brain's overall self-reported confidence in the decision (clamped `[0,1]`). The
    /// per-section confidence floors still apply independently; this is provenance only.
    pub confidence: f32,
    /// The brain's free-text provenance note (clamped). Audit/provenance only.
    pub provenance: String,
}

impl PrimeBrainDecision {
    /// How many distinct sections this envelope carried — the count drives the small
    /// "one brain decision" provenance chip (shown only when the single call genuinely
    /// produced more than one proposal, the thing that makes the unified path worth
    /// attributing over the specialized one).
    pub fn section_count(&self) -> usize {
        [
            self.classification.is_some(),
            self.task.is_some(),
            self.agent.is_some(),
            self.plugin.is_some(),
            self.permission.is_some(),
            self.assign.is_some(),
            self.update.is_some(),
            self.wording.is_some(),
            !self.context_requests.is_empty(),
            self.action_request.is_some(),
            self.reply.is_some(),
            self.plan_polish.is_some(),
        ]
        .into_iter()
        .filter(|&present| present)
        .count()
    }

    /// Validate and reconcile the carried wording against the turn's actual
    /// [`crate::prime_clarify::ClarifyKind`] and deterministic reply text, returning the
    /// polished text to show or `None` to keep the deterministic wording.
    ///
    /// This reuses the SAME validators the specialized polish path uses
    /// ([`crate::prime_clarify::parse_clarify`] → `reconcile_clarify`): a clarify is forced
    /// to exactly one question, an action-claim is rejected, low confidence / a pure echo is
    /// dropped — no weaker duplicate logic. Because the eligible kind is known only after the
    /// kernel produces the turn, the wording is carried raw and validated here.
    pub fn validated_wording(
        &self,
        kind: crate::prime_clarify::ClarifyKind,
        deterministic_text: &str,
    ) -> Option<String> {
        let raw = self.wording.as_ref()?;
        let parsed = crate::prime_clarify::parse_clarify(raw, kind).ok()?;
        crate::prime_clarify::reconcile_clarify(deterministic_text, &parsed, kind)
    }

    /// Validate the carried free-form conversational reply against the turn's deterministic
    /// reply text, returning the polished reply to show or `None` to keep the deterministic
    /// one.
    ///
    /// This reuses the EXACT chokepoint a brainstorm reply uses
    /// ([`crate::prime_clarify::parse_clarify`] with
    /// [`crate::prime_clarify::ClarifyKind::Brainstorm`] → `reconcile_clarify`): control chars
    /// are stripped, the text is clamped, a reply that claims a completed action is rejected
    /// wholesale, and a low-confidence or pure-echo reply is dropped — no weaker duplicate
    /// logic. The caller applies this ONLY on a non-actionful, non-clarify conversational turn,
    /// so the brain is never near an action and can never narrate a state change that did not
    /// happen.
    pub fn validated_reply(&self, deterministic_text: &str) -> Option<String> {
        use crate::prime_clarify::ClarifyKind;
        let raw = self.reply.as_ref()?;
        let parsed = crate::prime_clarify::parse_clarify(raw, ClarifyKind::Brainstorm).ok()?;
        crate::prime_clarify::reconcile_clarify(deterministic_text, &parsed, ClarifyKind::Brainstorm)
    }

    /// Validate the carried plan-polish against the turn's AUTHORITATIVE proposal, returning the
    /// advisory overlay to attach or `None` to leave the deterministic preview unpolished.
    ///
    /// This reuses the EXACT chokepoint the specialized polish uses
    /// ([`crate::ai::polish_from_cli_text`] → `validate_polish`): a step title is accepted only
    /// when its index matches the authoritative step exactly (any merge / split / reorder / add /
    /// rename drops the titles entirely), and summary / questions / risks are trimmed and bounded.
    /// So the overlay can change only the WORDING — never the step count, order, or agent ids —
    /// and `model_label` stamps provenance. Because the brain proposes the polish before it sees
    /// the authoritative steps, its step titles usually fail the index match and drop, while the
    /// proposal-independent summary / questions / risks survive; the dedicated specialized polish
    /// call is the fallback when this yields nothing usable.
    pub fn validated_polish(
        &self,
        proposal: &relux_core::PrimeProposal,
        model_label: &str,
    ) -> Option<relux_core::PrimeProposalPolish> {
        let raw = self.plan_polish.as_ref()?;
        crate::ai::polish_from_cli_text(proposal, raw, model_label)
    }
}

/// The strict, self-contained prompt handed to a brain to produce ONE unified decision for
/// ONE message, grounded in the live board so an assignment / update references a REAL id.
///
/// Mirrors the specialized prompts: the allowed intent labels are listed, each optional
/// section's shape is spelled out, the conversational-safety rules are explicit (musing /
/// questions stay chat; only an explicit instruction is work; never invent ids; never claim
/// an action), and JSON-only output is demanded so nothing un-validated leaks downstream.
/// Kept ASCII and self-contained so it works as a one-shot CLI stdin prompt.
///
/// `observations` is the rendered result of any read-only context tools the brain already
/// requested earlier in this turn's bounded observe-then-act loop (empty on the FIRST round, so a
/// single-round turn's prompt is byte-for-byte the prior unparameterized prompt). When non-empty it
/// is injected with a steer to commit (omit `tool_requests`) once the brain has observed enough —
/// the Hermes "the model gives its final answer when it stops requesting tools" shape.
pub fn build_decision_prompt(message: &str, summary: &StateSummary, observations: &str) -> String {
    let labels = intent_labels().join(", ");
    let tools = crate::prime_tools::read_only_tool_names();
    let write_tools = crate::prime_write_tools::write_tool_names();
    let (tasks, agents) = board_catalog(summary);
    let mut prompt = format!(
        "You are the single decision stage for Prime, the operator of a local Relux control \
plane (tasks, runs, agents, plugins, permissions, approvals, an audit log). For the user's \
message, return ONE JSON object describing your decision. You perform NO action and create \
nothing this turn: you only propose. Never claim you created a task, started a run, installed \
a plugin, granted a permission, or assigned work. Never invent a task id, agent id, plugin, or \
number. Use plain ASCII.\n\n\
Respond with JSON ONLY (no prose, no code fences). Include ONLY the sections that apply; omit \
the rest. The shape is:\n\
{{\n\
  \"classification\": {{\"intent\":\"<one label>\",\"confidence\":0.0-1.0}},\n\
  \"task\": {{\"title\":\"<imperative title>\",\"details\":\"<optional>\",\"assignee\":\"<optional existing agent id>\",\"priority\":<optional 1-9>,\"confidence\":0.0-1.0}},\n\
  \"agent\": {{\"name\":\"<agent name>\",\"role\":\"<optional>\",\"adapter\":\"<optional existing adapter id>\",\"persona\":\"<optional>\",\"confidence\":0.0-1.0}},\n\
  \"plugin\": {{\"plugin_id\":\"<plugin id>\",\"confidence\":0.0-1.0}},\n\
  \"permission\": {{\"subject_kind\":\"agent\",\"subject_id\":\"<existing agent id>\",\"permission\":\"<optional>\",\"confidence\":0.0-1.0}},\n\
  \"assign\": {{\"task_id\":\"<existing task id>\",\"agent_id\":\"<existing agent id>\",\"confidence\":0.0-1.0}},\n\
  \"update\": {{\"task_id\":\"<existing task id>\",\"title\":\"<optional>\",\"details\":\"<optional>\",\"priority\":<optional 1-9>,\"status\":\"<optional blocked|cancelled>\",\"assignee\":\"<optional existing agent id>\",\"confidence\":0.0-1.0}},\n\
  \"wording\": {{\"text\":\"<one clarifying question, or a short brainstorm reply>\",\"confidence\":0.0-1.0}},\n\
  \"tool_requests\": [{{\"tool\":\"<read-only tool>\",\"args\":{{...}}}}],\n\
  \"action_request\": {{\"tool\":\"<one write tool>\",\"args\":{{...}}}},\n\
  \"reply\": {{\"text\":\"<a short, natural conversational answer>\",\"confidence\":0.0-1.0}},\n\
  \"plan_polish\": {{\"summary\":\"<clearer one-line plan summary>\",\"questions\":[\"<optional>\"],\"risks\":[\"<optional>\"]}},\n\
  \"confidence\": 0.0-1.0\n\
}}\n\n\
Rules:\n\
- classification.intent MUST be exactly one of: {labels}. Casual chat, musing, or a question \
(\"how does X work?\", \"we should...\") is brainstorming, NOT work. Only an explicit \
instruction to DO something is a work intent. If genuinely ambiguous, prefer brainstorming.\n\
- Include a slot section ONLY when its action clearly applies to this message: \"task\" for a \
create, \"agent\" for creating an operative, \"plugin\"/\"permission\" for an install/grant \
request, \"assign\" to assign an existing task to an existing agent, \"update\" to change an \
existing task by id.\n\
- assign/update/permission ids and the task assignee MUST come from the lists below; if you \
are unsure of an id, omit that field. Never invent an id.\n\
- Include \"wording\" ONLY when the turn is a clarifying question or a brainstorm reply. For a \
clarify it MUST be EXACTLY ONE concrete question ending in '?'. Never assert a completed action.\n\
- Include \"tool_requests\" ONLY when answering needs you to inspect live state first (e.g. a \
specific task's detail, a run, the full crew). Each entry MUST name a READ-ONLY tool from: \
{tools}. These tools ONLY read; they change nothing. The kernel runs them and you answer grounded \
in the results. Never request a tool that writes, deletes, or runs anything.\n\
- Include \"action_request\" ONLY when the user EXPLICITLY instructs Prime to DO one thing. It \
names exactly ONE write tool from: {write_tools}. Casual chat, musing, or a question is NEVER an \
action_request. At most ONE per turn (do not batch). The kernel still validates every id and gates \
plugin.install / permission.grant behind a human approval — you only request; you change nothing.\n\
- Include \"reply\" with a short, natural conversational answer when the turn is plain \
conversation (a greeting, a direct factual answer, an explanation) rather than a clarifying \
question. Keep it brief; never claim you created, started, installed, granted, or changed \
anything.\n\
- Include \"plan_polish\" ONLY when proposing a multi-step plan, to improve WORDING: a clearer \
summary and at most a few advisory questions/risks. Do NOT change the number, order, or owners \
of steps.\n\
- Do NOT add any key other than those shown above.\n\n\
Tasks on the board:\n{tasks}\n\nAgents:\n{agents}\n\nUser message:\n{message}"
    );
    // Inject the live reads gathered earlier this turn (the observe-then-act loop). On the first
    // round this is empty and the prompt is unchanged; once the kernel has run the brain's
    // read-only tools it re-asks with the results and a steer to commit.
    let observations = observations.trim();
    if !observations.is_empty() {
        prompt.push_str(&format!(
            "\n\nYou have ALREADY inspected live state this turn through read-only tools (these \
reads changed nothing):\n{observations}\n\n\
If this is enough to answer the user or to choose your one action, return your full decision now \
and OMIT \"tool_requests\". Include \"tool_requests\" again ONLY if you still must inspect more \
state before deciding."
        ));
    }
    prompt
}

/// The wire labels offered to the brain (the snake_case `PrimeIntent` serialization).
/// Advisory only: [`crate::prime_intent::parse_intent_proposal`] validates against
/// `PrimeIntent`'s own deserializer, so a drifted label simply fails that section.
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

/// Build the bounded `(tasks, agents)` grounding catalogs — `"<task_id>: <title>"` from the
/// ready queue then recent tasks (deduped) and the agent roster — so a brain can resolve an
/// assignment / update by description. Grounding, not authority: the kernel validates every
/// id. Mirrors [`crate::prime_assign_slots::build_assign_slots_prompt`].
fn board_catalog(summary: &StateSummary) -> (String, String) {
    let mut seen: Vec<String> = Vec::new();
    let mut task_lines: Vec<String> = Vec::new();
    for b in summary.queued.iter().chain(summary.recent.iter()) {
        let id = b.id.0.clone();
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        task_lines.push(format!("  - {id}: {}", b.title));
        if task_lines.len() >= MAX_PROMPT_TASKS {
            break;
        }
    }
    if task_lines.is_empty() {
        task_lines.push("  (no tasks on the board)".to_string());
    }
    let agents = if summary.all_agent_ids.is_empty() {
        "  (no agents)".to_string()
    } else {
        summary
            .all_agent_ids
            .iter()
            .take(MAX_PROMPT_AGENTS)
            .map(|a| format!("  - {a}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    (task_lines.join("\n"), agents)
}

/// Re-serialize a section's JSON value and validate it through `parser`, returning the
/// validated section or `None` (dropping just that section). This is the compositional reuse
/// seam: each KNOWN section is validated by its OWN existing validator (no weaker duplicate
/// logic), and a section the parser rejects (bad shape, unsupported nested field, missing
/// required field) is dropped while the rest of the envelope stands — openclaw's
/// `readPlanSteps` per-entry validation, but non-fatal at the envelope level because every
/// dropped section has a specialized / deterministic fallback.
/// Normalize a text-bearing section (the free-form `reply`) into the `{text, ...}` object
/// shape the brainstorm validator expects. A brain may emit it as a bare string or as an
/// object; an object is carried verbatim, a bare string becomes `{"text": <string>, ...}`, and
/// any other JSON value (number/array/bool/null) is dropped. Returns the re-serialized JSON.
///
/// A bare string is the brain's whole committed reply with no stated confidence; since the
/// downstream brainstorm chokepoint defaults a missing confidence to 0.5 (below its honor
/// floor) and would silently drop it, a bare string is stamped at [`BARE_REPLY_CONFIDENCE`]
/// (above the floor) so a deliberately-simple reply is honored. An object form is trusted to
/// carry its own confidence and is never re-stamped.
fn normalize_text_section(value: &serde_json::Value) -> Option<String> {
    if value.is_object() {
        serde_json::to_string(value).ok()
    } else if let Some(s) = value.as_str() {
        serde_json::to_string(&serde_json::json!({
            "text": s,
            "confidence": BARE_REPLY_CONFIDENCE,
        }))
        .ok()
    } else {
        None
    }
}

fn validate_section<T>(
    value: Option<&serde_json::Value>,
    parser: impl Fn(&str) -> Result<T, String>,
) -> Option<T> {
    let value = value?;
    // Re-serialize the sub-object so the existing parser (which lifts the first balanced
    // `{...}`) sees exactly the shape it expects. A non-object value serializes to e.g. a
    // bare string, which has no `{` and is rejected — fail closed for that section.
    let json = serde_json::to_string(value).ok()?;
    parser(&json).ok()
}

/// Parse a brain's raw reply into a validated [`PrimeBrainDecision`], or `Err` with a short
/// reason on anything unusable (no JSON object, an unknown top-level key, or zero usable
/// sections). This is the strict, compositional envelope gate described in the module docs.
///
/// - The reply must contain a balanced top-level JSON object ([`extract_json_object`]).
/// - ANY unknown top-level key fails the WHOLE envelope closed.
/// - Each KNOWN section is validated by its existing specialized validator; an invalid
///   section is dropped (its fallback applies) — see [`validate_section`].
/// - At least one usable section must survive, else the caller falls back to the
///   specialized paths / deterministic rails (the brain is strictly additive).
pub fn parse_decision(raw: &str) -> Result<PrimeBrainDecision, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    // Fail the whole envelope closed on ANY unknown top-level key — the brain may not smuggle
    // an un-modeled authority key past the parser (openclaw `additionalProperties: false`).
    for key in obj.keys() {
        if !ALLOWED_TOP_LEVEL_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported top-level field '{key}'"));
        }
    }

    let classification = validate_section(obj.get("classification"), |s| {
        crate::prime_intent::parse_intent_proposal(s)
    });
    let task = validate_section(obj.get("task"), crate::prime_slots::parse_task_slots);
    let agent = validate_section(obj.get("agent"), crate::prime_agent_slots::parse_agent_slots);
    let plugin = validate_section(obj.get("plugin"), crate::prime_admin_slots::parse_plugin_ref);
    let permission = validate_section(
        obj.get("permission"),
        crate::prime_admin_slots::parse_permission_slots,
    );
    let assign = validate_section(
        obj.get("assign"),
        crate::prime_assign_slots::parse_assign_slots,
    );
    let update = validate_section(
        obj.get("update"),
        crate::prime_update_slots::parse_update_slots,
    );

    // Carry the wording sub-object raw (re-serialized) ONLY when it is a JSON object; it is
    // validated later against the turn's `ClarifyKind`. A non-object wording is dropped.
    let wording = obj
        .get("wording")
        .filter(|v| v.is_object())
        .and_then(|v| serde_json::to_string(v).ok());

    // Read-only context tool requests: a list of `{tool, args}` the brain wants run BEFORE it
    // answers. Each entry is validated against the READ-ONLY allowlist via
    // `prime_tools::validate_tool_request` — a mutating / unknown / made-up name is DROPPED here and
    // can never execute — and the list is capped at the loop's round budget so it stays bounded.
    // A non-array `tool_requests` yields no requests. `context_reads` is accepted as an alias.
    let context_requests = obj
        .get("tool_requests")
        .or_else(|| obj.get("context_reads"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(crate::prime_tools::validate_tool_request)
                .take(crate::prime_tools::MAX_TOOL_ROUNDS)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // The single WRITE-capable tool request: validated against the write allowlist and mapped to
    // an EXISTING action's slot via `prime_write_tools::parse_write_tool_request`. A mutating /
    // unknown / made-up name is DROPPED here (fail closed) and can never act; a batched multi-tool
    // request is refused. `tool_call` is accepted as an alias.
    let action_request = obj
        .get("action_request")
        .or_else(|| obj.get("tool_call"))
        .and_then(crate::prime_write_tools::parse_write_tool_request);

    // Carry the free-form reply raw (re-serialized), validated later via `validated_reply`. A
    // brain may emit `reply` as a bare string ("Hello!") or as a `{text, confidence}` object;
    // normalize a bare string to `{text:...}` so the existing brainstorm validator sees the
    // shape it expects. `assistant_message` is accepted as an alias for the same field.
    let reply = obj
        .get("reply")
        .or_else(|| obj.get("assistant_message"))
        .and_then(normalize_text_section);

    // Carry the plan-polish sub-object raw (re-serialized) ONLY when it is a JSON object; it is
    // validated later against the authoritative proposal via `validated_polish`. A non-object
    // polish is dropped.
    let plan_polish = obj
        .get("plan_polish")
        .filter(|v| v.is_object())
        .and_then(|v| serde_json::to_string(v).ok());

    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;

    let provenance = obj
        .get("provenance")
        .or_else(|| obj.get("rationale"))
        .and_then(|v| v.as_str())
        .map(|s| {
            s.trim()
                .chars()
                .take(MAX_PROVENANCE_CHARS)
                .collect::<String>()
        })
        .unwrap_or_default();

    let decision = PrimeBrainDecision {
        classification,
        task,
        agent,
        plugin,
        permission,
        assign,
        update,
        context_requests,
        action_request,
        wording,
        reply,
        plan_polish,
        confidence,
        provenance,
    };

    // An envelope that produced no usable section is a failure — the caller falls back to the
    // specialized paths so the brain stays strictly additive (never a silent no-op that
    // suppresses the deterministic outcome).
    if decision.section_count() == 0 {
        return Err("no usable section in decision".to_string());
    }
    Ok(decision)
}

/// What the bounded observe-then-act loop should do after one decision round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionStep {
    /// The brain requested read-only context and the kernel executed it; re-call the decision brain
    /// with these rendered observations injected so it can act / answer grounded in live state.
    Continue(String),
    /// The decision is terminal — committed (no more reads requested), made no progress (re-asked
    /// for what it already saw), or hit the round cap. Use whatever the loop last produced.
    Stop,
}

/// The bounded **observe-then-act** decision driver: the unified decision call made to LOOP, so one
/// Prime turn can inspect a little live state through the governed READ-ONLY tools before the brain
/// commits its single action / answer.
///
/// ## Why this exists
///
/// The unified [`PrimeBrainDecision`] is otherwise a SINGLE call: the brain must choose its action
/// from the static board snapshot baked into the prompt, with no chance to drill into a specific
/// task / run / the crew first. The read-only context loop ([`crate::prime_tools::ContextLoop`])
/// could observe, but only on a NON-actionful inspection turn and only to ground a reply — never to
/// inform the action. This driver closes that gap: each round the brain may request read-only tools
/// (observe) OR commit (act / answer); the kernel executes only the read-only requests
/// deterministically between rounds and re-calls the brain grounded in the results, bounded by
/// [`MAX_DECISION_ROUNDS`].
///
/// ## Safety (binding)
///
/// The loop is **observe-then-act, read-only between rounds**. Between decision calls it executes
/// ONLY the validated read-only [`PrimeBrainDecision::context_requests`]
/// (via [`crate::prime_tools::execute_requested_reads`] against the pre-taken snapshot) — there is
/// no mutation, no approval, and no path to `prime_execute` here. The eventual ACTION (the terminal
/// decision's `action_request` / classification / slots) still flows through the UNCHANGED
/// fail-closed [`crate::prime_intent::reconcile_intent`] gate and `decide` → `prime_execute` /
/// approval at the kernel chokepoint — this driver adds NO new authority. It is bounded
/// ([`MAX_DECISION_ROUNDS`]) and stops on no progress (a brain that re-requests what it already
/// observed), exactly like the read-only loop's stop-on-repeat. The synchronous test twin
/// [`run_decision_loop`] and the async provider drivers share this SAME stepper, so the control
/// flow (cap, read-only execution, stop-on-progress) is pinned once.
///
/// Mirrors Hermes `run_conversation` (`reference/hermes-agent-main/agent/conversation_loop.py`): a
/// bounded loop where each round the model either requests tools (the loop continues, injecting the
/// results) or returns its final answer (the loop ends) — but the Relux brain executes NOTHING; the
/// kernel runs the (read-only) tools and, later, the one governed action.
pub struct DecisionLoop {
    snapshot: crate::prime_tools::ContextSnapshot,
    reads: Vec<crate::prime_tools::ContextRead>,
    decision: Option<PrimeBrainDecision>,
    round: usize,
}

impl DecisionLoop {
    /// Start a loop over an owned, bounded read-only state snapshot (cloned in, exactly like
    /// [`crate::prime_tools::ContextLoop::new`], so the executors stay pure over it and the
    /// provider rounds run lock-free).
    pub fn new(snapshot: &crate::prime_tools::ContextSnapshot) -> Self {
        Self {
            snapshot: snapshot.clone(),
            reads: Vec::new(),
            decision: None,
            round: 0,
        }
    }

    /// Step the loop with the brain's parsed decision for this round (`None` on a provider failure).
    ///
    /// A decision with NO read-only requests is **committed** (terminal); at the round cap we
    /// likewise stop and use whatever the brain last produced. Otherwise the brain wants to OBSERVE
    /// first: the kernel executes its validated read-only requests against the snapshot, accumulates
    /// only the NEW reads, keeps the decision as an interim fallback, and asks the caller to re-call
    /// the brain with the rendered observations. A brain that re-requests what it already saw makes
    /// no progress and the loop stops (stop-on-repeat), so it can never spin.
    pub fn step(&mut self, decision: Option<PrimeBrainDecision>) -> DecisionStep {
        self.round += 1;
        let Some(d) = decision else {
            // A provider failure ends the loop; keep any interim decision already gathered.
            return DecisionStep::Stop;
        };
        if d.context_requests.is_empty() || self.round >= MAX_DECISION_ROUNDS {
            self.decision = Some(d);
            return DecisionStep::Stop;
        }
        let before = self.reads.len();
        for read in
            crate::prime_tools::execute_requested_reads(&self.snapshot, &d.context_requests)
        {
            if self
                .reads
                .iter()
                .any(|r| r.tool == read.tool && r.detail == read.detail)
            {
                continue;
            }
            self.reads.push(read);
        }
        // Keep this decision as the interim fallback (used if the next round's provider call fails),
        // grounded in everything observed up to and including this round.
        self.decision = Some(d);
        if self.reads.len() == before {
            // No NEW observation gained: the brain re-asked for what it already saw — stop.
            DecisionStep::Stop
        } else {
            DecisionStep::Continue(crate::prime_tools::render_observations(&self.reads))
        }
    }

    /// Consume the loop, returning the terminal decision (if any) and every read gathered along the
    /// way (provenance for the turn's `context_reads`, and grounding for the reply).
    pub fn into_parts(self) -> (Option<PrimeBrainDecision>, Vec<crate::prime_tools::ContextRead>) {
        (self.decision, self.reads)
    }
}

/// Drive the bounded observe-then-act decision loop with a SYNCHRONOUS brain closure — the testable
/// twin that pins the loop's control flow with a scripted brain and NO provider. The closure
/// receives the rendered observations gathered so far (empty on the first round) and returns the
/// brain's parsed decision, or `None` to abort (a provider failure) — exactly what the async
/// drivers do. The async provider drivers share the SAME [`DecisionLoop`] stepper, so this twin
/// pins their behavior.
pub fn run_decision_loop<F>(
    snapshot: &crate::prime_tools::ContextSnapshot,
    mut brain: F,
) -> (Option<PrimeBrainDecision>, Vec<crate::prime_tools::ContextRead>)
where
    F: FnMut(&str) -> Option<PrimeBrainDecision>,
{
    let mut lp = DecisionLoop::new(snapshot);
    let mut observations = String::new();
    loop {
        let decision = brain(&observations);
        match lp.step(decision) {
            DecisionStep::Continue(obs) => observations = obs,
            DecisionStep::Stop => break,
        }
    }
    lp.into_parts()
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::PrimeIntent;

    /// A minimal grounding summary with the given agent roster (no tasks). `StateSummary`
    /// has no `Default`, so the tests build it explicitly like the other slot modules.
    fn summary_with_agents(agents: &[&str]) -> StateSummary {
        StateSummary {
            plugins: 0,
            agents: agents.len(),
            tasks_total: 0,
            tasks_open: 0,
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: agents.iter().map(|s| s.to_string()).collect(),
            all_task_ids: vec![],
            queued: vec![],
            recent: vec![],
        }
    }

    #[test]
    fn build_prompt_carries_schema_safety_rules_and_board_grounding() {
        let summary = summary_with_agents(&["researcher"]);
        let prompt = build_decision_prompt("assign the readme task to research", &summary, "");
        assert!(prompt.contains("\"classification\""));
        assert!(prompt.contains("\"task\""));
        assert!(prompt.contains("\"wording\""));
        assert!(prompt.contains("JSON ONLY"));
        assert!(prompt.contains("Never invent"));
        // The allowed labels and the live roster are both grounded into the prompt.
        assert!(prompt.contains("task_creation"));
        assert!(prompt.contains("researcher"));
        // With no observations the prompt carries no observe-then-act injection (round 0).
        assert!(!prompt.contains("ALREADY inspected live state"));
    }

    #[test]
    fn build_prompt_injects_observations_and_a_commit_steer() {
        let summary = summary_with_agents(&["researcher"]);
        let base = build_decision_prompt("start the ready task", &summary, "");
        let observed =
            build_decision_prompt("start the ready task", &summary, "[list_tasks] 1 task\ntask_0001: Fix login");
        // The base prompt is a strict prefix of the observed one (the injection is appended, so a
        // single-round turn is byte-for-byte the prior unparameterized prompt).
        assert!(observed.starts_with(&base));
        assert!(observed.contains("ALREADY inspected live state"));
        assert!(observed.contains("task_0001: Fix login"));
        // The steer tells the brain to commit once it has observed enough.
        assert!(observed.contains("OMIT \"tool_requests\""));
    }

    #[test]
    fn parses_a_full_valid_decision() {
        // One envelope carrying intent + task slots + wording, the unified shape.
        let raw = r#"{
            "classification": {"intent":"task_creation","confidence":0.9},
            "task": {"title":"Fix the login redirect bug","priority":7,"confidence":0.9},
            "wording": {"text":"Which environment is affected?","confidence":0.8},
            "confidence": 0.9,
            "provenance": "explicit create"
        }"#;
        let d = parse_decision(raw).unwrap();
        assert_eq!(
            d.classification.as_ref().unwrap().intent,
            PrimeIntent::TaskCreation
        );
        assert_eq!(d.task.as_ref().unwrap().title, "Fix the login redirect bug");
        assert_eq!(d.task.as_ref().unwrap().priority, Some(7));
        assert!(d.wording.is_some());
        assert_eq!(d.confidence, 0.9);
        assert_eq!(d.provenance, "explicit create");
        assert_eq!(d.section_count(), 3);
    }

    #[test]
    fn extracts_from_a_noisy_reply_with_prose_and_fences() {
        let raw = "Here is my decision:\n```json\n{\"classification\":{\"intent\":\"brainstorming\",\
                   \"confidence\":0.7}}\n```\nHope that helps.";
        let d = parse_decision(raw).unwrap();
        assert_eq!(
            d.classification.as_ref().unwrap().intent,
            PrimeIntent::Brainstorming
        );
    }

    #[test]
    fn unknown_top_level_key_fails_the_whole_envelope_closed() {
        // A smuggled un-modeled authority key fails the WHOLE envelope (not just a section),
        // so the caller falls back to the specialized paths rather than acting on a partial,
        // possibly-misunderstood decision.
        let raw = r#"{"classification":{"intent":"task_creation","confidence":0.9},
                      "execute":true}"#;
        let err = parse_decision(raw).unwrap_err();
        assert!(err.contains("unsupported top-level field"), "got: {err}");
    }

    #[test]
    fn an_invalid_nested_section_is_dropped_but_the_envelope_stands() {
        // The task section carries a smuggled unsupported field (`run_tool`) — its own
        // validator fails it closed, so ONLY the task section is dropped; the valid intent
        // section survives and the envelope is still usable.
        let raw = r#"{
            "classification": {"intent":"task_creation","confidence":0.9},
            "task": {"title":"Fix it","run_tool":"shell","confidence":0.9}
        }"#;
        let d = parse_decision(raw).unwrap();
        assert!(d.classification.is_some());
        assert!(d.task.is_none(), "the invalid task section must be dropped");
        assert_eq!(d.section_count(), 1);
    }

    #[test]
    fn an_off_allowlist_intent_label_drops_only_the_classification() {
        // A hallucinated intent fails parse_intent_proposal, so the classification is dropped,
        // but a valid task section keeps the envelope usable (intent then falls back to the
        // deterministic classifier).
        let raw = r#"{
            "classification": {"intent":"delete_everything","confidence":1.0},
            "task": {"title":"Tidy the docs","confidence":0.9}
        }"#;
        let d = parse_decision(raw).unwrap();
        assert!(d.classification.is_none());
        assert_eq!(d.task.as_ref().unwrap().title, "Tidy the docs");
    }

    #[test]
    fn a_non_object_section_is_dropped_fail_closed() {
        // A section that is a bare string (not the expected object) has no balanced `{...}`
        // and is dropped, never coerced.
        let raw = r#"{
            "classification": {"intent":"greeting","confidence":0.9},
            "task": "fix the bug"
        }"#;
        let d = parse_decision(raw).unwrap();
        assert!(d.task.is_none());
        assert!(d.classification.is_some());
    }

    #[test]
    fn an_envelope_with_no_usable_section_is_an_error() {
        // Only metadata, no real section → the caller must fall back to the specialized paths.
        assert!(parse_decision(r#"{"confidence":0.9,"provenance":"hmm"}"#).is_err());
        assert!(parse_decision("not json at all").is_err());
        // Every section invalid → still a failure (no usable section survives).
        assert!(parse_decision(r#"{"task":{"bogus":1}}"#).is_err());
    }

    #[test]
    fn validated_wording_reuses_the_clarify_validators() {
        use crate::prime_clarify::ClarifyKind;
        let raw = r#"{"wording":{"text":"Which task should I update?","confidence":0.9}}"#;
        let d = parse_decision(raw).unwrap();
        // A confident, distinct, single-question wording is honored for a Clarify turn.
        assert_eq!(
            d.validated_wording(ClarifyKind::Clarify, "Which task and what change?")
                .as_deref(),
            Some("Which task should I update?")
        );

        // A multi-question wording is rejected by the SAME parse_clarify validator (the
        // unified path applies no weaker logic than the specialized one).
        let bad = parse_decision(
            r#"{"wording":{"text":"Which task? And what field?","confidence":0.9}}"#,
        )
        .unwrap();
        assert!(bad
            .validated_wording(ClarifyKind::Clarify, "anything")
            .is_none());

        // An action-claim wording is rejected too.
        let claim = parse_decision(
            r#"{"wording":{"text":"I created the task for you.","confidence":0.95}}"#,
        )
        .unwrap();
        assert!(claim
            .validated_wording(ClarifyKind::Brainstorm, "anything")
            .is_none());
    }

    #[test]
    fn assign_and_update_sections_reuse_their_validators() {
        let raw = r#"{
            "classification": {"intent":"assign_task","confidence":0.9},
            "assign": {"task_id":"task_0001","agent_id":"researcher","confidence":0.9}
        }"#;
        let d = parse_decision(raw).unwrap();
        let a = d.assign.unwrap();
        assert_eq!(a.task_id.as_deref(), Some("task_0001"));
        assert_eq!(a.agent_id.as_deref(), Some("researcher"));

        let upd = parse_decision(
            r#"{"update":{"task_id":"task_0002","priority":8,"confidence":0.9}}"#,
        )
        .unwrap();
        assert_eq!(upd.update.unwrap().task_id.as_deref(), Some("task_0002"));
    }

    #[test]
    fn carries_a_free_form_reply_and_validates_it_via_the_brainstorm_chokepoint() {
        // A greeting/chat turn: the envelope carries the conversational answer in the one call.
        let d = parse_decision(
            r#"{"classification":{"intent":"greeting","confidence":0.9},
                "reply":{"text":"Hey - I'm Prime. What would you like to do?","confidence":0.9}}"#,
        )
        .unwrap();
        assert_eq!(d.section_count(), 2);
        assert_eq!(
            d.validated_reply("Hi, I'm Prime.").as_deref(),
            Some("Hey - I'm Prime. What would you like to do?")
        );

        // A bare-string reply is normalized to the {text} shape the validator expects.
        let bare = parse_decision(r#"{"reply":"Sure, happy to help."}"#).unwrap();
        assert_eq!(
            bare.validated_reply("deterministic").as_deref(),
            Some("Sure, happy to help.")
        );

        // `assistant_message` is accepted as an alias for the same field.
        let alias =
            parse_decision(r#"{"assistant_message":{"text":"All good here.","confidence":0.9}}"#)
                .unwrap();
        assert!(alias.validated_reply("x").is_some());
    }

    #[test]
    fn a_reply_that_claims_a_completed_action_is_rejected() {
        // The SAME action-claim rail a brainstorm reply uses: the brain can never narrate a
        // state change that did not happen, even in the free-form reply.
        let d = parse_decision(
            r#"{"reply":{"text":"I created the task and started the run.","confidence":0.95}}"#,
        )
        .unwrap();
        assert!(d.reply.is_some(), "the section is carried raw");
        assert!(
            d.validated_reply("here is the grounded reply").is_none(),
            "an action claim must be rejected at validation"
        );
    }

    #[test]
    fn carries_plan_polish_and_validates_it_against_the_authoritative_proposal() {
        use relux_core::{PrimeProposal, PrimeProposalStep};
        let proposal = PrimeProposal {
            goal: "ship the dashboard".to_string(),
            multi_step: true,
            steps: vec![
                PrimeProposalStep {
                    index: 1,
                    title: "Design".to_string(),
                    role: "design".to_string(),
                    agent: "prime".to_string(),
                },
                PrimeProposalStep {
                    index: 2,
                    title: "Build".to_string(),
                    role: "build".to_string(),
                    agent: "prime".to_string(),
                },
            ],
            agents: vec!["prime".to_string()],
            polish: None,
        };

        // Summary + advisory questions/risks survive; matching step titles apply.
        let d = parse_decision(
            r#"{"plan_polish":{"summary":"Two phases: design then build.",
                 "steps":[{"index":1,"title":"Sketch the IA"},{"index":2,"title":"Implement"}],
                 "questions":["Which framework?"],"risks":["Scope creep"]}}"#,
        )
        .unwrap();
        let polish = d.validated_polish(&proposal, "Claude CLI").unwrap();
        assert_eq!(polish.summary.as_deref(), Some("Two phases: design then build."));
        assert_eq!(polish.step_titles.len(), 2);
        assert_eq!(polish.model.as_deref(), Some("Claude CLI"));

        // Structural drift (a step index the proposal does not have) drops the titles entirely
        // through the SAME validate_polish chokepoint, but the summary still survives.
        let drift = parse_decision(
            r#"{"plan_polish":{"summary":"Refined.",
                 "steps":[{"index":1,"title":"A"},{"index":2,"title":"B"},{"index":3,"title":"C"}]}}"#,
        )
        .unwrap();
        let polish = drift.validated_polish(&proposal, "m").unwrap();
        assert!(polish.step_titles.is_empty(), "drifted titles must be dropped");
        assert_eq!(polish.summary.as_deref(), Some("Refined."));
    }

    #[test]
    fn carries_read_only_tool_requests_validated_against_the_allowlist() {
        // The unified envelope may request read-only context tools up front; each is validated
        // through the SAME read-only allowlist the loop uses, so the kernel can run them
        // deterministically with no second multi-round brain loop.
        let d = parse_decision(
            r#"{
                "classification":{"intent":"status_question","confidence":0.9},
                "tool_requests":[
                    {"tool":"get_task","args":{"task_id":"task_0001"}},
                    {"tool":"list_agents"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(d.context_requests.len(), 2);
        assert_eq!(d.context_requests[0].tool, "get_task");
        assert_eq!(
            d.context_requests[0].args.get("task_id").unwrap().as_str(),
            Some("task_0001")
        );
        assert_eq!(d.context_requests[1].tool, "list_agents");
        // The section counts toward the usable total alongside the classification.
        assert_eq!(d.section_count(), 2);
    }

    #[test]
    fn a_mutating_tool_request_is_rejected_never_executed() {
        // A mutating / unknown tool named in tool_requests is DROPPED at parse time (fail closed),
        // so it can never reach an executor. The valid read-only request alongside it survives.
        let d = parse_decision(
            r#"{
                "tool_requests":[
                    {"tool":"delete_task","args":{"task_id":"task_0001"}},
                    {"tool":"run_shell","args":{"cmd":"rm -rf /"}},
                    {"tool":"board_summary"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(d.context_requests.len(), 1, "only the read-only request survives");
        assert_eq!(d.context_requests[0].tool, "board_summary");

        // An envelope whose ONLY tool requests are mutating has no usable section -> the caller
        // falls back to the specialized/sidecar paths (the brain is strictly additive).
        assert!(parse_decision(r#"{"tool_requests":[{"tool":"delete_task"}]}"#).is_err());
    }

    #[test]
    fn context_reads_is_accepted_as_an_alias_for_tool_requests() {
        let d = parse_decision(r#"{"context_reads":[{"tool":"list_runs"}]}"#).unwrap();
        assert_eq!(d.context_requests.len(), 1);
        assert_eq!(d.context_requests[0].tool, "list_runs");
    }

    #[test]
    fn tool_requests_are_bounded_by_the_round_cap() {
        // A brain that lists more requests than the round budget is capped at parse time, so the
        // deterministic execution can never exceed the loop's bound.
        let many: Vec<String> = (0..crate::prime_tools::MAX_TOOL_ROUNDS + 6)
            .map(|_| r#"{"tool":"board_summary"}"#.to_string())
            .collect();
        let raw = format!(r#"{{"tool_requests":[{}]}}"#, many.join(","));
        let d = parse_decision(&raw).unwrap();
        assert!(d.context_requests.len() <= crate::prime_tools::MAX_TOOL_ROUNDS);
    }

    #[test]
    fn carries_a_write_tool_request_validated_against_the_allowlist() {
        // The unified envelope may request ONE write-capable tool; it is validated through the
        // write allowlist + the existing per-action validator, and mapped to an intent + slot.
        let d = parse_decision(
            r#"{
                "classification":{"intent":"task_creation","confidence":0.9},
                "action_request":{"tool":"task.create","args":{"title":"Fix the login redirect"}}
            }"#,
        )
        .unwrap();
        let wt = d.action_request.as_ref().expect("a validated write tool");
        assert_eq!(wt.tool, "task.create");
        assert_eq!(wt.intent, PrimeIntent::TaskCreation);
        assert!(!wt.gated);
        // The section counts toward the usable total alongside the classification.
        assert_eq!(d.section_count(), 2);
    }

    #[test]
    fn a_mutating_or_unknown_write_tool_is_dropped_at_parse_time() {
        // An off-allowlist (mutating-sounding / made-up) write tool name is DROPPED — never mapped
        // to an action — so an envelope whose only mutating section is bogus has no usable section
        // and the caller falls back to the deterministic path (the brain is strictly additive).
        assert!(parse_decision(
            r#"{"action_request":{"tool":"task.delete","args":{"task_id":"task_0001"}}}"#
        )
        .is_err());
        assert!(parse_decision(r#"{"action_request":{"tool":"shell.run","args":{}}}"#).is_err());

        // A valid read-only request alongside a bogus write tool keeps the read-only one; the
        // write tool is simply absent.
        let d = parse_decision(
            r#"{
                "tool_requests":[{"tool":"board_summary"}],
                "action_request":{"tool":"task.delete","args":{}}
            }"#,
        )
        .unwrap();
        assert!(d.action_request.is_none());
        assert_eq!(d.context_requests.len(), 1);
    }

    #[test]
    fn carries_an_orchestration_write_tool_request() {
        // The unified envelope may request the governed `orchestration.create` write tool; it
        // validates through the write allowlist + the orchestration-slot validator and maps to
        // the Orchestration intent (a safe Act, not approval-gated).
        let d = parse_decision(
            r#"{
                "classification":{"intent":"orchestration","confidence":0.9},
                "action_request":{"tool":"orchestration.create",
                  "args":{"goal":"research it, build it, and test it"}}
            }"#,
        )
        .unwrap();
        let wt = d.action_request.as_ref().expect("a validated write tool");
        assert_eq!(wt.tool, "orchestration.create");
        assert_eq!(wt.intent, PrimeIntent::Orchestration);
        assert!(!wt.gated, "orchestration.create is a safe Act");
        match &wt.slot {
            crate::prime_write_tools::WriteToolSlot::Orchestration(s) => {
                assert_eq!(s.goal, "research it, build it, and test it");
            }
            other => panic!("expected an orchestration slot, got {other:?}"),
        }
    }

    #[test]
    fn carries_an_orchestration_start_write_tool_request() {
        // The unified envelope may request the governed `orchestration.start` write tool; it
        // validates through the write allowlist + the run-orchestration-slot parser and maps to
        // the OrchestrationRun intent (a safe Act, not approval-gated).
        let d = parse_decision(
            r#"{
                "classification":{"intent":"orchestration_run","confidence":0.9},
                "action_request":{"tool":"orchestration.start",
                  "args":{"orchestration_id":"orch_0001"}}
            }"#,
        )
        .unwrap();
        let wt = d.action_request.as_ref().expect("a validated write tool");
        assert_eq!(wt.tool, "orchestration.start");
        assert_eq!(wt.intent, PrimeIntent::OrchestrationRun);
        assert!(!wt.gated, "orchestration.start is a safe Act");
        match &wt.slot {
            crate::prime_write_tools::WriteToolSlot::RunOrchestration(s) => {
                assert_eq!(s.orchestration_id, "orch_0001");
            }
            other => panic!("expected a run-orchestration slot, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_is_accepted_as_an_alias_and_gated_tools_are_marked() {
        // `tool_call` is an alias for `action_request`.
        let d =
            parse_decision(r#"{"tool_call":{"tool":"permission.grant","args":{"subject_kind":"agent","subject_id":"researcher"}}}"#)
                .unwrap();
        let wt = d.action_request.unwrap();
        assert_eq!(wt.intent, PrimeIntent::PermissionChange);
        assert!(wt.gated, "permission.grant maps to an approval-gated Propose");
    }

    #[test]
    fn reply_and_plan_polish_count_toward_the_section_total() {
        // A reply-only envelope is usable (a pure conversational turn).
        let d = parse_decision(r#"{"reply":{"text":"Hello.","confidence":0.9}}"#).unwrap();
        assert_eq!(d.section_count(), 1);
        // A non-object reply (number) is dropped, leaving no usable section -> error.
        assert!(parse_decision(r#"{"reply":42}"#).is_err());
    }

    // ---- the bounded observe-then-act decision loop -------------------------------------------

    use crate::prime_tools::{AgentView, ContextSnapshot, RunView, TaskView};
    use relux_core::TaskStatus;

    /// A snapshot with one READY (queued) task so an observe round can find it and a `task.start`
    /// action can be chosen against it.
    fn loop_snapshot() -> ContextSnapshot {
        let mut summary = summary_with_agents(&["researcher"]);
        summary.tasks_total = 1;
        summary.tasks_open = 1;
        summary.all_task_ids = vec!["task_0001".to_string()];
        summary.queued = vec![relux_core::TaskBrief {
            id: relux_core::TaskId("task_0001".to_string()),
            title: "Fix the login redirect".to_string(),
            status: TaskStatus::Queued,
            assigned_agent: None,
        }];
        ContextSnapshot {
            summary,
            tasks: vec![TaskView {
                id: "task_0001".to_string(),
                title: "Fix the login redirect".to_string(),
                status: TaskStatus::Queued,
                assignee: None,
                priority: 5,
                detail: None,
            }],
            agents: vec![AgentView {
                id: "researcher".to_string(),
                name: "Research Agent".to_string(),
                role: "Surveys options".to_string(),
                adapter: "relux-adapter-local-prime".to_string(),
                persona: None,
            }],
            runs: Vec::<RunView>::new(),
            plugins: Vec::new(),
            approvals: Vec::new(),
        }
    }

    #[test]
    fn loop_observes_then_acts_grounded_in_the_reads() {
        let snapshot = loop_snapshot();
        let mut rounds = 0usize;
        let mut saw_observations_on_act = false;
        let (decision, reads) = run_decision_loop(&snapshot, |observations| {
            rounds += 1;
            if observations.is_empty() {
                // Round 1: inspect the board first (observe), commit nothing yet.
                parse_decision(r#"{"tool_requests":[{"tool":"list_tasks","args":{}}]}"#).ok()
            } else {
                // Round 2: the brain was re-called WITH the observations and now acts grounded in
                // them — exactly the observe-then-act shape.
                saw_observations_on_act = observations.contains("task_0001");
                parse_decision(
                    r#"{"action_request":{"tool":"task.start","args":{"task_id":"task_0001"}}}"#,
                )
                .ok()
            }
        });
        assert_eq!(rounds, 2, "one observe round, then one commit round");
        assert!(saw_observations_on_act, "the act round saw the gathered reads");
        let wt = decision.expect("a terminal decision").action_request.expect("a write tool");
        assert_eq!(wt.tool, "task.start");
        // The reads the loop executed between rounds are returned for grounding / provenance.
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].tool, "list_tasks");
    }

    #[test]
    fn loop_is_byte_for_byte_single_shot_when_the_brain_commits_immediately() {
        let snapshot = loop_snapshot();
        let mut rounds = 0usize;
        let (decision, reads) = run_decision_loop(&snapshot, |_observations| {
            rounds += 1;
            // The brain commits on the first round (no tool_requests): the loop runs exactly once
            // and gathers nothing, identical to the prior single decision call.
            parse_decision(r#"{"reply":{"text":"Hi there.","confidence":0.9}}"#).ok()
        });
        assert_eq!(rounds, 1);
        assert!(reads.is_empty());
        assert!(decision.unwrap().reply.is_some());
    }

    #[test]
    fn loop_is_bounded_by_the_round_cap() {
        let snapshot = loop_snapshot();
        let mut rounds = 0usize;
        // A brain that keeps observing with a DIFFERENT tool every round (so it always makes
        // progress and never repeats) is still bounded by MAX_DECISION_ROUNDS.
        let tools = ["board_summary", "list_tasks", "list_agents", "get_task"];
        let (_decision, _reads) = run_decision_loop(&snapshot, |_observations| {
            let tool = tools[rounds.min(tools.len() - 1)];
            rounds += 1;
            let args = if tool == "get_task" { r#"{"task_id":"task_0001"}"# } else { "{}" };
            parse_decision(&format!(r#"{{"tool_requests":[{{"tool":"{tool}","args":{args}}}]}}"#))
                .ok()
        });
        assert_eq!(rounds, MAX_DECISION_ROUNDS, "the loop never exceeds the round cap");
    }

    #[test]
    fn loop_stops_on_no_progress_when_the_brain_re_requests_the_same_read() {
        let snapshot = loop_snapshot();
        let mut rounds = 0usize;
        // The brain requests the SAME read every round: round 1 gathers it, round 2 gains no new
        // observation and the loop stops (stop-on-repeat) well before the cap.
        let (_decision, reads) = run_decision_loop(&snapshot, |_observations| {
            rounds += 1;
            parse_decision(r#"{"tool_requests":[{"tool":"list_tasks","args":{}}]}"#).ok()
        });
        assert_eq!(rounds, 2);
        assert_eq!(reads.len(), 1, "the repeated read is gathered once, not twice");
    }

    #[test]
    fn loop_keeps_the_interim_decision_when_a_later_round_fails() {
        let snapshot = loop_snapshot();
        let mut rounds = 0usize;
        // Round 1 observes AND carries a classification; round 2 the provider fails (None). The loop
        // keeps the interim decision (grounded in the round-1 read) rather than discarding the turn.
        let (decision, reads) = run_decision_loop(&snapshot, |observations| {
            rounds += 1;
            if observations.is_empty() {
                parse_decision(
                    r#"{"classification":{"intent":"status_question","confidence":0.8},
                        "tool_requests":[{"tool":"list_tasks","args":{}}]}"#,
                )
                .ok()
            } else {
                None
            }
        });
        assert_eq!(rounds, 2);
        assert_eq!(reads.len(), 1);
        assert!(decision.expect("interim decision kept").classification.is_some());
    }
}
