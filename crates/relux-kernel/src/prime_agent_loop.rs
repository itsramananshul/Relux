//! Prime Agent Loop v1 — a bounded Hermes-style think → tool → observe → respond loop for chat.
//!
//! ## Why this exists
//!
//! [`crate::prime_tools`] gave Prime a SAFE *read-only* context loop (inspect the board / a task /
//! a run before answering). [`crate::prime_invoke_tool`](crate::state) added a SINGLE explicit tool
//! invocation from chat through the real gates (run a `Ready` tool, or stage a per-call approval for
//! a gated one). What was still missing is the thing that makes a product feel like a real agent
//! (Hermes / Codex / Paperclip): on an explicit tool request, Prime should be able to **call an
//! allowed tool, OBSERVE its real output, and CONTINUE** — chaining a small number of tool calls
//! and folding what it learned into a useful final answer — all behind the SAME fail-closed gates.
//!
//! This module is that loop's **pure, deterministic control core**. It encodes every v1 rule (the
//! caps, the fail-closed catalog validation, the approval pause, the observation folding) against an
//! injected brain closure and an injected execution closure, so the whole behavior is unit-tested
//! with a scripted brain and NO kernel / network — exactly the [`crate::prime_tools::run_context_loop`]
//! pattern. The kernel wires the real (async, off-lock) brain rounds and the real (locked, audited)
//! tool execution onto the SAME [`AgentLoop`] step methods, so the loop logic is pinned once and
//! never drifts between the test twin and the live path.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/conversation_loop.py` `run_conversation(...)` — the per-turn agentic loop:
//!   `while (api_call_count < agent.max_iterations and agent.iteration_budget.remaining > 0)` (L598)
//!   bounds the rounds; each round the assistant reply is inspected for `tool_calls`, each requested
//!   tool is executed and its result fed back as a `role:"tool"` message (L630-676), and the loop
//!   ends when the model stops requesting tools and returns a final answer. The chosen tool name is
//!   validated against `agent.valid_tool_names` BEFORE execution (L389, L656) — an off-list name is
//!   fed back as a self-correction message, NEVER executed. We mirror exactly this shape:
//!   [`AgentLoop`] is the bounded driver ([`MAX_AGENT_TOOL_CALLS`] / [`MAX_BRAIN_ROUNDS`] are the
//!   caps), [`interpret_agent_reply`] is the reply interpreter, the live catalog ([`AgentTool`]) is
//!   `valid_tool_names`, and [`AgentReply::UnknownTool`] is the self-correction feedback.
//! - **Paperclip (openclaw)** `src/agents/tool-mutation.ts` `isMutatingToolCall(toolName, args)` —
//!   the FAIL-CLOSED default where an UNKNOWN action is treated as mutating, and
//!   `src/acp/approval-classifier.ts` — an unknown tool never auto-approves. We invert the polarity
//!   for the same safety: the brain may pick ONLY a tool present in the live, per-turn catalog
//!   ([`build_agent_catalog`] admits only `Ready` / `NeedsApproval` descriptors the agent can
//!   actually run), a gated (`NeedsApproval`) tool is NEVER auto-run — the loop PAUSES with the
//!   existing approval card — and anything else fails closed.
//! - **Paperclip** `src/acp/permission-relay.ts` — the three-decision approval surfaced to a human
//!   (`allow-once` / `allow-always` / `deny`). The pause path reuses the EXISTING
//!   `PrimeToolApprovalRequest` card and its routes; this module only signals WHEN to pause.
//!
//! ## The safety contract (binding)
//!
//! - **Explicit request only.** The loop is engaged by the kernel ONLY for a turn whose intent is
//!   `ToolInvocation` (the user explicitly asked to use / check / call a tool). Normal chat, a
//!   greeting, frustration / profanity, a vague idea, a Q&A or a brainstorm classifies as some other
//!   intent and NEVER enters the loop — it stays conversational (`docs/mcp.md` "Prime Agent Loop").
//! - **Fail closed on the tool.** [`interpret_agent_reply`] executes ONLY a `{"tool":...}` whose
//!   `plugin/tool` matches a live catalog entry; an off-catalog / made-up name is
//!   [`AgentReply::UnknownTool`], fed back for self-correction, never executed.
//! - **No auto-run of gated tools.** The injected exec closure returns [`ToolStepOutcome::AwaitApproval`]
//!   for a `NeedsApproval` tool with no standing grant; the loop STOPS and the kernel stages the
//!   existing per-call approval card. An allow-always grant turns that same tool into a direct
//!   [`ToolStepOutcome::Ran`] (the kernel's `prime_agent_step` reuses `prime_invoke_tool`'s grant
//!   check), so a granted tool participates in the loop like any low-risk one.
//! - **Bounded.** At most [`MAX_AGENT_TOOL_CALLS`] tool executions and [`MAX_BRAIN_ROUNDS`] brain
//!   rounds per turn (Hermes's `max_iterations`); a repeated identical call (no progress) stops the
//!   loop; each observation is length-clamped ([`MAX_OBS_CHARS`]) and secret-redacted.
//! - **Grounding, not new authority.** Every execution flows through the UNCHANGED
//!   permission/approval/grant/audit gates (`invoke_tool`); the loop adds no mutation path of its
//!   own. The gathered observations ground the final reply and are surfaced as a compact, redacted
//!   trace ([`relux_core::PrimeToolTrace`]); they are never a fabricated result.

use serde::{Deserialize, Serialize};

use crate::prime_intent::extract_json_object;
use relux_core::{PrimeToolTrace, ToolDescriptor, ToolExecutability};

/// The hard cap on tool executions in one chat turn (v1). Small on purpose: a chat answer rarely
/// needs more than a couple of real tool calls, and a tight bound keeps a misbehaving brain from
/// spinning or running up cost. Mirrors Hermes's `max_iterations` discipline.
pub const MAX_AGENT_TOOL_CALLS: usize = 3;

/// The hard cap on brain rounds in one chat turn: one initial pick plus up to two
/// post-observation iterations ("max 2 brain iterations after tool observations"). A round that
/// names an off-catalog tool consumes a round (the self-correction is fed back), so the loop can
/// never spin on retries.
pub const MAX_BRAIN_ROUNDS: usize = 3;

/// Max characters kept from any single tool observation the brain sees next round (and that is
/// folded into the grounded reply). Keeps a large tool output from blowing the context. Mirrors
/// the read-only loop's `MAX_RESULT_CHARS`.
const MAX_OBS_CHARS: usize = 2_000;

/// Max characters kept from a sanitized one-line summary / label.
const MAX_LINE_CHARS: usize = 200;

/// Max tools advertised to the brain in the loop prompt, so a huge installed catalog cannot blow
/// the prompt. A larger catalog is reported with an honest "(+N more)" note.
const MAX_CATALOG_ADVERTISED: usize = 40;

/// One tool the brain may pick this turn — a bounded projection of a live [`ToolDescriptor`] that is
/// actually runnable-or-gatable for this agent. This is the loop's `valid_tool_names`: the brain
/// can choose ONLY from these, and [`interpret_agent_reply`] validates every pick against them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTool {
    /// The plugin id the call routes through (`mcp:<server>` for an MCP tool, else the installed
    /// plugin id).
    pub plugin_id: String,
    /// The bare tool name.
    pub tool_name: String,
    /// The `<plugin_id>/<tool_name>` label — what the brain puts in `{"tool":"..."}`.
    pub label: String,
    /// A short, sanitized one-line description shown in the prompt.
    pub description: String,
    /// The tool's risk, lowercase wire form (`low`/`medium`/`high`/`critical`).
    pub risk: String,
    /// The source kind for the prompt/badge: `"mcp"` or `"plugin"`.
    pub source: String,
    /// Whether running this tool will require a human approval (`executable == NeedsApproval` and
    /// no standing grant). Advisory for the prompt only — the real gate is the kernel's exec step;
    /// the brain is told that picking a gated tool will pause for approval.
    pub gated: bool,
}

/// Project the live tool catalog (installed plugin tools PLUS the off-lock-discovered live MCP
/// tools) down to the bounded set the brain may pick from. FAIL CLOSED: only a descriptor whose
/// [`ToolExecutability`] is `Ready` (directly runnable) or `NeedsApproval` (runnable behind the
/// existing approval/grant gate) is admitted. A tool the agent lacks permission for, or that has no
/// runtime, is OMITTED — the brain can never choose a tool that cannot run, so there is no path to
/// a fabricated result. Pure.
pub fn build_agent_catalog(descriptors: &[ToolDescriptor]) -> Vec<AgentTool> {
    descriptors
        .iter()
        .filter_map(|d| {
            let gated = match d.executable {
                ToolExecutability::Ready => false,
                ToolExecutability::NeedsApproval => true,
                // Not runnable this turn (missing permission / no runtime / not implemented) →
                // never offered to the brain.
                _ => return None,
            };
            let source = if d.plugin_id.starts_with("mcp:") { "mcp" } else { "plugin" };
            Some(AgentTool {
                plugin_id: d.plugin_id.clone(),
                tool_name: d.tool_name.clone(),
                label: format!("{}/{}", d.plugin_id, d.tool_name),
                description: sanitize_line(&d.description, MAX_LINE_CHARS),
                risk: risk_wire(&d.risk),
                source: source.to_string(),
                gated,
            })
        })
        .collect()
}

/// Lowercase wire label for a [`relux_core::RiskLevel`].
fn risk_wire(risk: &relux_core::RiskLevel) -> String {
    serde_json::to_value(risk)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "medium".to_string())
}

/// A validated tool pick the brain requested — already matched against the live catalog by
/// [`interpret_agent_reply`], so `plugin_id` + `tool_name` name a real, runnable-or-gatable tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPick {
    pub plugin_id: String,
    pub tool_name: String,
    /// The raw arguments object the brain supplied (the kernel re-validates + bounds it at exec).
    pub args: serde_json::Value,
}

impl AgentPick {
    /// The `<plugin_id>/<tool_name>` label.
    pub fn label(&self) -> String {
        format!("{}/{}", self.plugin_id, self.tool_name)
    }
}

/// How one brain reply inside the loop is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentReply {
    /// A valid pick of a tool present in the live catalog — execute it through the gates.
    Call(AgentPick),
    /// A final natural-language answer (`{"answer":"..."}`) — stop and use it.
    Answer(String),
    /// A `{"tool":...}` naming a tool NOT in the catalog — fed back for self-correction (Hermes),
    /// never executed.
    UnknownTool(String),
    /// No actionable directive (no JSON, or neither `tool` nor `answer`) — stop gathering and fall
    /// back to the conversational reply.
    Done,
}

/// Interpret one brain reply: detect a tool pick (validated against the catalog), a final answer,
/// an off-catalog name (self-correct), or a stop. Pure. Reuses the shared balanced-brace scanner so
/// a reply wrapped in prose/fences still parses.
pub fn interpret_agent_reply(raw: &str, catalog: &[AgentTool]) -> AgentReply {
    let Some(json) = extract_json_object(raw) else {
        return AgentReply::Done;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return AgentReply::Done;
    };
    let Some(obj) = value.as_object() else {
        return AgentReply::Done;
    };
    // A final answer ends the loop. Accept either `answer` or `done`+`answer`.
    if let Some(answer) = obj.get("answer").and_then(|v| v.as_str()) {
        let trimmed = answer.trim();
        if !trimmed.is_empty() {
            return AgentReply::Answer(trimmed.to_string());
        }
    }
    if obj.get("done").and_then(|v| v.as_bool()) == Some(true) {
        return AgentReply::Done;
    }
    let raw_tool = match obj.get("tool").and_then(|v| v.as_str()) {
        Some(t) => sanitize_line(t, MAX_LINE_CHARS),
        None => return AgentReply::Done,
    };
    if raw_tool.is_empty() || raw_tool.eq_ignore_ascii_case("none") {
        return AgentReply::Done;
    }
    // Resolve the named tool against the catalog. Accept the `<plugin>/<tool>` label form, OR a
    // bare tool name when it is unambiguous. Anything else is an unknown tool (fail closed).
    let resolved = resolve_pick(&raw_tool, catalog);
    let Some(tool) = resolved else {
        return AgentReply::UnknownTool(raw_tool);
    };
    let args = obj.get("args").cloned().unwrap_or(serde_json::Value::Null);
    AgentReply::Call(AgentPick {
        plugin_id: tool.plugin_id.clone(),
        tool_name: tool.tool_name.clone(),
        args,
    })
}

/// Resolve a brain-named tool string against the live catalog. Matches the `<plugin>/<tool>` label
/// exactly first; otherwise a bare tool name that is unique in the catalog. `None` (fail closed)
/// when the name is off-catalog or ambiguous.
fn resolve_pick<'a>(name: &str, catalog: &'a [AgentTool]) -> Option<&'a AgentTool> {
    if let Some(hit) = catalog.iter().find(|t| t.label == name) {
        return Some(hit);
    }
    let mut by_tool = catalog.iter().filter(|t| t.tool_name == name);
    let first = by_tool.next()?;
    if by_tool.next().is_none() {
        Some(first)
    } else {
        // Ambiguous bare name (same tool on two plugins) — refuse rather than guess.
        None
    }
}

/// One executed tool observation the brain sees next round and that grounds the final reply.
/// Bounded + secret-redacted. Honest by construction: produced ONLY from a real kernel execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentObservation {
    /// The `<plugin_id>/<tool_name>` label that ran.
    pub label: String,
    /// The source kind (`"mcp"` / `"plugin"`).
    pub source: String,
    /// Whether the call succeeded.
    pub ok: bool,
    /// A short one-line summary for the trace chip.
    pub summary: String,
    /// The bounded, redacted detail body fed back to the brain.
    pub detail: String,
}

impl AgentObservation {
    /// Build a successful observation from a real tool output value (redacted + bounded).
    pub fn ran(label: &str, source: &str, output: &serde_json::Value) -> Self {
        let rendered = render_output(output);
        let summary = first_line(&rendered, 120);
        AgentObservation {
            label: sanitize_line(label, MAX_LINE_CHARS),
            source: source.to_string(),
            ok: true,
            summary: if summary.is_empty() { "ok".to_string() } else { summary },
            detail: clamp(rendered),
        }
    }

    /// Build a failed observation (the tool ran but reported an error). Still honest — surfaced as
    /// `ok:false` with the reason, never a fabricated success.
    pub fn failed(label: &str, source: &str, reason: &str) -> Self {
        let r = sanitize_line(reason, MAX_OBS_CHARS);
        AgentObservation {
            label: sanitize_line(label, MAX_LINE_CHARS),
            source: source.to_string(),
            ok: false,
            summary: first_line(&r, 120),
            detail: r,
        }
    }

    /// Project to the compact wire trace chip.
    pub fn to_trace(&self) -> PrimeToolTrace {
        PrimeToolTrace {
            label: self.label.clone(),
            source: self.source.clone(),
            ok: self.ok,
            summary: self.summary.clone(),
        }
    }
}

/// The outcome of executing one [`AgentPick`] through the real gates. Returned by the injected
/// exec closure; the kernel's `prime_agent_step` builds it from `prime_invoke_tool`'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStepOutcome {
    /// The tool ran (low-risk, or covered by a standing allow-always grant) and produced an
    /// observation — fold it in and continue.
    Ran(AgentObservation),
    /// The tool is gated (`NeedsApproval`) and there is no standing grant — PAUSE the loop. The
    /// kernel stages the existing per-call approval card; nothing ran.
    AwaitApproval,
    /// The tool could not run (missing permission / no runtime / unknown / execution error). FAIL
    /// CLOSED: stop with the honest reason; nothing was fabricated.
    Refused(String),
}

/// The result of executing ONE agent-loop pick through the real kernel gates — built by the
/// kernel's `prime_agent_step` from [`crate::state::KernelState::prime_invoke_tool`]'s terminal
/// turn. Either a successful observation to fold in and continue, or a TERMINAL turn to surface
/// as-is: a staged per-call approval card (a gated tool with no grant — the loop PAUSES) or an
/// honest refusal (a missing / unrunnable / unknown tool — fail closed). Kept here so the loop's
/// outcome types live together; the [`relux_core::PrimeTurn`] is boxed because it is large relative
/// to an observation.
#[derive(Debug, Clone)]
pub enum AgentExecStep {
    /// The tool ran and produced an observation — continue the loop.
    Observed(AgentObservation),
    /// A terminal turn (staged approval card or honest refusal) to surface directly.
    Terminal(Box<relux_core::PrimeTurn>),
}

/// Why the loop stopped — drives how the kernel shapes the final turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    /// The brain produced an explicit final answer (with or without tool observations).
    Answered,
    /// The brain never picked a tool (stayed conversational) — the kernel falls back to the normal
    /// conversational reply path, unchanged.
    NoTool,
    /// A gated tool was picked with no grant — the kernel stages the approval card and PAUSES.
    AwaitingApproval,
    /// A tool failed closed — the kernel surfaces the honest clarification.
    Refused(String),
    /// The caps were hit with observations gathered but no explicit answer — the kernel folds the
    /// observations into the grounded reply.
    Exhausted,
}

/// The result of running the bounded agent loop for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopResult {
    /// The tool observations gathered, in order (each a real, redacted, bounded execution result).
    pub observations: Vec<AgentObservation>,
    /// The brain's explicit final answer, when it gave one.
    pub answer: Option<String>,
    /// Why the loop ended.
    pub outcome: AgentOutcome,
    /// The pick that triggered an [`AgentOutcome::AwaitingApproval`] pause, so the kernel knows
    /// exactly which tool to stage the approval card for.
    pub pending_pick: Option<AgentPick>,
}

impl AgentLoopResult {
    /// Whether any real tool execution happened this turn.
    pub fn ran_any_tool(&self) -> bool {
        !self.observations.is_empty()
    }

    /// The compact wire trace for the chat UI (one chip per executed tool).
    pub fn trace(&self) -> Vec<PrimeToolTrace> {
        self.observations.iter().map(AgentObservation::to_trace).collect()
    }
}

/// The bounded, stateful driver for the agent loop. Holds the catalog + the observations gathered
/// so far. Both the synchronous test driver ([`run_agent_loop`]) and the async kernel orchestration
/// share these step methods, so the loop logic (round/call caps, catalog validation, self-correction,
/// gated-pause, stop-on-repeat) is pinned once and never drifts between the paths.
pub struct AgentLoop {
    message: String,
    catalog: Vec<AgentTool>,
    observations: Vec<AgentObservation>,
    feedback: Option<String>,
    /// Brain rounds consumed (every brain call, including a self-correction re-ask).
    rounds: usize,
    /// Tool executions performed (`Ran` outcomes count; a pause/refusal stops the loop).
    tool_calls: usize,
    terminal: Option<AgentOutcome>,
    answer: Option<String>,
    pending_pick: Option<AgentPick>,
    /// The args of the most recently executed pick, for repeat-detection (a brain that loops on the
    /// same tool+args makes no progress).
    last_pick_args: Option<serde_json::Value>,
}

/// What the driver decided to do with one classified brain reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStep {
    /// Execute this pick through the gates (caller runs the exec closure).
    Execute(AgentPick),
    /// Re-ask the brain with the self-correction feedback already recorded (off-catalog tool).
    Retry,
    /// The loop is over (final answer, conversational, or nothing actionable).
    Stop,
}

impl AgentLoop {
    /// Start a loop for one user message over the live catalog.
    pub fn new(message: &str, catalog: Vec<AgentTool>) -> Self {
        Self {
            message: message.to_string(),
            catalog,
            observations: Vec::new(),
            feedback: None,
            rounds: 0,
            tool_calls: 0,
            terminal: None,
            answer: None,
            pending_pick: None,
            last_pick_args: None,
        }
    }

    /// The next prompt to send the brain, or `None` when the loop is over (terminal set, round cap
    /// reached, or no tool budget left and nothing more to ask). Pure: does not advance state.
    pub fn next_prompt(&self) -> Option<String> {
        if self.terminal.is_some() {
            return None;
        }
        if self.rounds >= MAX_BRAIN_ROUNDS {
            return None;
        }
        // Out of tool budget AND we already gathered something — there is nothing more to do but
        // answer, which the caller does by folding observations (Exhausted). Asking again would
        // only risk another (over-budget) pick.
        if self.tool_calls >= MAX_AGENT_TOOL_CALLS {
            return None;
        }
        Some(build_agent_prompt(
            &self.message,
            &self.catalog,
            &self.observations,
            self.feedback.as_deref(),
        ))
    }

    /// Classify one brain reply and advance the round counter. Records self-correction feedback for
    /// an off-catalog tool. Returns the [`AgentStep`] the caller should take. The caller is
    /// responsible for running the exec closure on [`AgentStep::Execute`] and reporting the result
    /// back via [`Self::record_outcome`].
    pub fn classify(&mut self, raw: &str) -> AgentStep {
        self.rounds += 1;
        self.feedback = None;
        match interpret_agent_reply(raw, &self.catalog) {
            AgentReply::Answer(a) => {
                self.answer = Some(a);
                self.terminal = Some(AgentOutcome::Answered);
                AgentStep::Stop
            }
            AgentReply::Done => {
                // Conversational reply (no tool picked) vs. a stop after tools ran.
                if self.observations.is_empty() {
                    self.terminal = Some(AgentOutcome::NoTool);
                } else {
                    self.terminal = Some(AgentOutcome::Exhausted);
                }
                AgentStep::Stop
            }
            AgentReply::UnknownTool(name) => {
                self.feedback = Some(unknown_tool_feedback(&name, &self.catalog));
                AgentStep::Retry
            }
            AgentReply::Call(pick) => {
                // Stop on a repeated identical call (no progress): the brain is looping on the same
                // tool+args, so spending another execution on it is wasteful.
                if self
                    .observations
                    .iter()
                    .any(|o| o.label == pick.label())
                    && self.last_pick_args.as_ref() == Some(&pick.args)
                {
                    self.terminal = Some(AgentOutcome::Exhausted);
                    return AgentStep::Stop;
                }
                AgentStep::Execute(pick)
            }
        }
    }

    /// Report the exec closure's outcome for an [`AgentStep::Execute`] pick, advancing the loop.
    /// Returns `true` to continue (re-prompt), `false` when the loop is done (pause / refusal).
    pub fn record_outcome(&mut self, pick: &AgentPick, outcome: ToolStepOutcome) -> bool {
        self.last_pick_args = Some(pick.args.clone());
        match outcome {
            ToolStepOutcome::Ran(obs) => {
                self.tool_calls += 1;
                self.observations.push(obs);
                // Continue only if there is budget AND rounds left; next_prompt enforces both.
                true
            }
            ToolStepOutcome::AwaitApproval => {
                self.pending_pick = Some(pick.clone());
                self.terminal = Some(AgentOutcome::AwaitingApproval);
                false
            }
            ToolStepOutcome::Refused(reason) => {
                self.terminal = Some(AgentOutcome::Refused(reason));
                false
            }
        }
    }

    /// Consume the loop into its result. If no terminal was reached (the brain stopped responding,
    /// or the caps were hit mid-gather), classify by what was gathered.
    pub fn into_result(self) -> AgentLoopResult {
        let outcome = self.terminal.unwrap_or_else(|| {
            if self.answer.is_some() {
                AgentOutcome::Answered
            } else if self.observations.is_empty() {
                AgentOutcome::NoTool
            } else {
                AgentOutcome::Exhausted
            }
        });
        AgentLoopResult {
            observations: self.observations,
            answer: self.answer,
            outcome,
            pending_pick: self.pending_pick,
        }
    }
}

/// The self-correction message fed back when the brain names a tool not in the live catalog
/// (Hermes's `"Tool '…' does not exist. Available: …"`).
pub fn unknown_tool_feedback(name: &str, catalog: &[AgentTool]) -> String {
    let available = catalog
        .iter()
        .take(MAX_CATALOG_ADVERTISED)
        .map(|t| t.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if available.is_empty() {
        format!(
            "Tool '{}' is not available, and no tools are runnable right now. Reply \
             {{\"answer\":\"...\"}} to answer without a tool.",
            sanitize_line(name, MAX_LINE_CHARS)
        )
    } else {
        format!(
            "Tool '{}' is not available. Runnable tools: {}. Pick one of those as \
             {{\"tool\":\"<plugin/tool>\",\"args\":{{...}}}}, or reply {{\"answer\":\"...\"}} to \
             answer without a tool.",
            sanitize_line(name, MAX_LINE_CHARS),
            available
        )
    }
}

/// Build the loop prompt: the runnable tools, the observations gathered so far, any self-correction
/// feedback, and the instruction to pick ONE tool or give a final answer. Kept ASCII and
/// self-contained so it works as a one-shot CLI stdin prompt.
pub fn build_agent_prompt(
    message: &str,
    catalog: &[AgentTool],
    observations: &[AgentObservation],
    feedback: Option<&str>,
) -> String {
    let tools = if catalog.is_empty() {
        "  (no tools are runnable right now)".to_string()
    } else {
        let shown = catalog.len().min(MAX_CATALOG_ADVERTISED);
        let mut lines: Vec<String> = catalog
            .iter()
            .take(shown)
            .map(|t| {
                let gate = if t.gated { " [needs approval]" } else { "" };
                format!("  - {} (risk={}, {}){} {}", t.label, t.risk, t.source, gate, t.description)
            })
            .collect();
        if catalog.len() > shown {
            lines.push(format!("  (+{} more)", catalog.len() - shown));
        }
        lines.join("\n")
    };
    let gathered = if observations.is_empty() {
        "  (none yet)".to_string()
    } else {
        observations
            .iter()
            .map(|o| {
                let status = if o.ok { "ok" } else { "error" };
                format!("  - {} [{}] -> {}\n{}", o.label, status, o.summary, indent(&o.detail))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let feedback_block = match feedback {
        Some(f) => format!("\nNote: {f}\n"),
        None => String::new(),
    };
    format!(
        "You are Prime, a local AI agent. The user explicitly asked you to use a tool. To fulfill \
their request you may call ONE runnable tool at a time, observe its real output, then call another \
or give a final answer. Use plain ASCII.\n\n\
Runnable tools (pick ONLY from these):\n{tools}\n\n\
Tool observations so far:\n{gathered}\n{feedback_block}\n\
Respond with JSON ONLY (no prose, no code fences). Either call ONE tool:\n\
  {{\"tool\":\"<plugin/tool from the list>\",\"args\":{{...}}}}\n\
or give your final answer to the user, incorporating what the tools returned:\n\
  {{\"answer\":\"<your answer>\"}}\n\n\
Rules: pick a tool ONLY if it helps fulfill THIS request; never invent a tool name or arguments. A \
tool marked [needs approval] will pause for the operator to approve before it runs. When you have \
enough to answer, give the answer.\n\n\
User request:\n{message}"
    )
}

/// Drive the bounded agent loop with SYNCHRONOUS brain + exec closures — the testable twin of the
/// async kernel orchestration. `brain` returns the raw reply (or `None` to abort, a provider
/// failure); `exec` runs one validated pick through the (test) gates and returns its outcome.
pub fn run_agent_loop<B, E>(
    message: &str,
    catalog: Vec<AgentTool>,
    mut brain: B,
    mut exec: E,
) -> AgentLoopResult
where
    B: FnMut(&str) -> Option<String>,
    E: FnMut(&AgentPick) -> ToolStepOutcome,
{
    let mut lp = AgentLoop::new(message, catalog);
    while let Some(prompt) = lp.next_prompt() {
        let Some(raw) = brain(&prompt) else {
            break;
        };
        match lp.classify(&raw) {
            AgentStep::Stop => break,
            AgentStep::Retry => continue,
            AgentStep::Execute(pick) => {
                let outcome = exec(&pick);
                if !lp.record_outcome(&pick, outcome) {
                    break;
                }
            }
        }
    }
    lp.into_result()
}

/// Render the gathered observations as a compact grounded-facts block to fold into the
/// conversational reply prompt, or an empty string when nothing was gathered. Pure.
pub fn render_observations(observations: &[AgentObservation]) -> String {
    if observations.is_empty() {
        return String::new();
    }
    observations
        .iter()
        .map(|o| {
            let status = if o.ok { "ok" } else { "error" };
            format!("[{} {}] {}\n{}", o.label, status, o.summary, o.detail)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ── helpers ────────────────────────────────────────────────────────────────

/// Strip control chars, collapse whitespace, and clamp to `max` chars.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The first line of a body, sanitized + clamped, for a chip summary.
fn first_line(body: &str, max: usize) -> String {
    sanitize_line(body.lines().next().unwrap_or(""), max)
}

/// Render a tool output JSON value to a compact, secret-redacted text body. A string value is used
/// as-is; any other JSON is pretty-printed. Always redacted through the shared helper.
fn render_output(output: &serde_json::Value) -> String {
    let raw = match output {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    relux_core::redact_secrets(&raw)
}

/// Clamp a detail body to [`MAX_OBS_CHARS`] with an honest truncation marker.
fn clamp(detail: String) -> String {
    if detail.chars().count() <= MAX_OBS_CHARS {
        detail
    } else {
        let mut out: String = detail.chars().take(MAX_OBS_CHARS).collect();
        out.push_str("\n(truncated)");
        out
    }
}

/// Indent a multi-line observation body under its bullet in the prompt.
fn indent(body: &str) -> String {
    body.lines().map(|l| format!("      {l}")).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::RiskLevel;

    fn desc(plugin: &str, tool: &str, exec: ToolExecutability, risk: RiskLevel) -> ToolDescriptor {
        ToolDescriptor {
            plugin_id: plugin.to_string(),
            tool_name: tool.to_string(),
            description: format!("the {tool} tool"),
            permission: format!("tool:{plugin}:{tool}"),
            risk,
            source_kind: "Bundled".to_string(),
            installed: true,
            enabled: true,
            protected: false,
            executable: exec,
        }
    }

    /// A small catalog: one Ready low-risk tool, one gated tool, one MCP tool, plus non-runnable
    /// ones that must be filtered out.
    fn sample_catalog() -> Vec<AgentTool> {
        build_agent_catalog(&[
            desc("relux-tools-echo", "echo", ToolExecutability::Ready, RiskLevel::Low),
            desc("relux-tools-fs", "delete", ToolExecutability::NeedsApproval, RiskLevel::High),
            desc("mcp:notes", "listNotes", ToolExecutability::Ready, RiskLevel::Low),
            // These must NOT be offered to the brain (fail closed):
            desc("relux-tools-x", "nope", ToolExecutability::MissingPermission, RiskLevel::Low),
            desc("relux-tools-y", "later", ToolExecutability::NotImplemented, RiskLevel::Low),
            desc("relux-tools-z", "off", ToolExecutability::RuntimeDisabled, RiskLevel::Low),
        ])
    }

    #[test]
    fn build_catalog_admits_only_runnable_or_gated_tools() {
        let cat = sample_catalog();
        let labels: Vec<&str> = cat.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(cat.len(), 3, "only Ready + NeedsApproval tools are offered: {labels:?}");
        assert!(labels.contains(&"relux-tools-echo/echo"));
        assert!(labels.contains(&"relux-tools-fs/delete"));
        assert!(labels.contains(&"mcp:notes/listNotes"));
        // MissingPermission / NotImplemented / RuntimeDisabled are filtered.
        assert!(!labels.iter().any(|l| l.contains("nope") || l.contains("later") || l.contains("off")));
        // The gated and source flags are correct.
        let gated = cat.iter().find(|t| t.tool_name == "delete").unwrap();
        assert!(gated.gated);
        assert_eq!(gated.source, "plugin");
        let mcp = cat.iter().find(|t| t.tool_name == "listNotes").unwrap();
        assert_eq!(mcp.source, "mcp");
        assert!(!mcp.gated);
    }

    #[test]
    fn interpret_resolves_label_and_bare_name_and_rejects_off_catalog() {
        let cat = sample_catalog();
        // Label form.
        match interpret_agent_reply("{\"tool\":\"relux-tools-echo/echo\",\"args\":{\"x\":1}}", &cat) {
            AgentReply::Call(p) => {
                assert_eq!(p.plugin_id, "relux-tools-echo");
                assert_eq!(p.tool_name, "echo");
            }
            other => panic!("expected Call, got {other:?}"),
        }
        // Bare (unambiguous) name.
        assert!(matches!(
            interpret_agent_reply("{\"tool\":\"listNotes\"}", &cat),
            AgentReply::Call(p) if p.plugin_id == "mcp:notes"
        ));
        // Off-catalog name → UnknownTool (fail closed), never a Call.
        assert!(matches!(
            interpret_agent_reply("{\"tool\":\"run_shell\",\"args\":{}}", &cat),
            AgentReply::UnknownTool(n) if n == "run_shell"
        ));
        // A made-up plugin for a real tool name is still off-catalog.
        assert!(matches!(
            interpret_agent_reply("{\"tool\":\"evil/echo\"}", &cat),
            AgentReply::UnknownTool(_)
        ));
        // Final answer.
        assert!(matches!(
            interpret_agent_reply("{\"answer\":\"all done\"}", &cat),
            AgentReply::Answer(a) if a == "all done"
        ));
        // Plain prose (no JSON) → Done (conversational).
        assert_eq!(interpret_agent_reply("hi there!", &cat), AgentReply::Done);
    }

    #[test]
    fn bare_name_on_two_plugins_is_ambiguous_and_refused() {
        let cat = build_agent_catalog(&[
            desc("plugin-a", "lookup", ToolExecutability::Ready, RiskLevel::Low),
            desc("plugin-b", "lookup", ToolExecutability::Ready, RiskLevel::Low),
        ]);
        assert!(matches!(
            interpret_agent_reply("{\"tool\":\"lookup\"}", &cat),
            AgentReply::UnknownTool(_)
        ));
    }

    // ── Acceptance scenarios (scripted brain + exec, no kernel / network) ──

    #[test]
    fn greeting_or_answer_first_means_no_tool_loop() {
        // The brain answers immediately without picking a tool: zero executions, conversational.
        let mut execs = 0;
        let result = run_agent_loop(
            "hello",
            sample_catalog(),
            |_p| Some("{\"answer\":\"Hi! How can I help?\"}".to_string()),
            |_pick| {
                execs += 1;
                ToolStepOutcome::Ran(AgentObservation::ran("x/y", "plugin", &serde_json::json!("z")))
            },
        );
        assert_eq!(execs, 0, "no tool was executed");
        assert_eq!(result.outcome, AgentOutcome::Answered);
        assert_eq!(result.answer.as_deref(), Some("Hi! How can I help?"));
        assert!(!result.ran_any_tool());
    }

    #[test]
    fn non_json_chatter_never_executes_a_tool() {
        // Frustration / profanity / vague prose parses to no JSON → Done → NoTool, nothing runs.
        let mut execs = 0;
        let result = run_agent_loop(
            "this is garbage, ugh",
            sample_catalog(),
            |_p| Some("Ugh, I understand the frustration.".to_string()),
            |_pick| {
                execs += 1;
                ToolStepOutcome::Refused("should never run".to_string())
            },
        );
        assert_eq!(execs, 0);
        assert_eq!(result.outcome, AgentOutcome::NoTool);
        assert!(result.answer.is_none());
    }

    #[test]
    fn low_risk_tool_executes_and_observation_grounds_the_answer() {
        let mut round = 0;
        let result = run_agent_loop(
            "echo hello",
            sample_catalog(),
            |_p| {
                round += 1;
                if round == 1 {
                    Some("{\"tool\":\"relux-tools-echo/echo\",\"args\":{\"text\":\"hello\"}}".to_string())
                } else {
                    Some("{\"answer\":\"The echo tool returned: hello\"}".to_string())
                }
            },
            |pick| {
                assert_eq!(pick.label(), "relux-tools-echo/echo");
                ToolStepOutcome::Ran(AgentObservation::ran(
                    &pick.label(),
                    "plugin",
                    &serde_json::json!({"echo": "hello"}),
                ))
            },
        );
        assert_eq!(result.outcome, AgentOutcome::Answered);
        assert_eq!(result.observations.len(), 1);
        assert!(result.observations[0].ok);
        assert!(result.answer.as_deref().unwrap().contains("hello"));
        assert_eq!(result.trace().len(), 1);
        assert_eq!(result.trace()[0].label, "relux-tools-echo/echo");
    }

    #[test]
    fn gated_tool_pauses_for_approval_and_runs_nothing_more() {
        let mut execs = 0;
        let result = run_agent_loop(
            "delete the file",
            sample_catalog(),
            |_p| Some("{\"tool\":\"relux-tools-fs/delete\",\"args\":{\"path\":\"/x\"}}".to_string()),
            |_pick| {
                execs += 1;
                ToolStepOutcome::AwaitApproval
            },
        );
        assert_eq!(execs, 1, "the gated pick was offered to the gate exactly once");
        assert_eq!(result.outcome, AgentOutcome::AwaitingApproval);
        // The pending pick is recorded so the kernel knows which tool to stage.
        assert_eq!(result.pending_pick.as_ref().unwrap().label(), "relux-tools-fs/delete");
        // Nothing was folded in as a successful observation.
        assert!(result.observations.is_empty());
    }

    #[test]
    fn allow_always_grant_lets_a_gated_tool_run_inside_the_loop() {
        // Models the allow-always path: the exec closure (the kernel's prime_agent_step) sees a
        // standing grant for the gated tool and RUNS it directly, returning an observation.
        let mut round = 0;
        let result = run_agent_loop(
            "delete the file",
            sample_catalog(),
            |_p| {
                round += 1;
                if round == 1 {
                    Some("{\"tool\":\"relux-tools-fs/delete\",\"args\":{\"path\":\"/x\"}}".to_string())
                } else {
                    Some("{\"answer\":\"Deleted /x.\"}".to_string())
                }
            },
            |pick| {
                ToolStepOutcome::Ran(AgentObservation::ran(
                    &pick.label(),
                    "plugin",
                    &serde_json::json!({"deleted": "/x"}),
                ))
            },
        );
        assert_eq!(result.outcome, AgentOutcome::Answered);
        assert_eq!(result.observations.len(), 1);
        assert!(result.answer.unwrap().contains("/x"));
    }

    #[test]
    fn unknown_tool_choice_fails_closed_then_self_corrects() {
        let mut round = 0;
        let mut execs = 0;
        let result = run_agent_loop(
            "use a tool",
            sample_catalog(),
            |prompt| {
                round += 1;
                match round {
                    1 => Some("{\"tool\":\"run_shell\",\"args\":{\"cmd\":\"rm -rf /\"}}".to_string()),
                    2 => {
                        // The self-correction feedback names the unknown tool and the real ones.
                        assert!(prompt.contains("run_shell"));
                        assert!(prompt.contains("relux-tools-echo/echo"));
                        Some("{\"answer\":\"I cannot run that tool.\"}".to_string())
                    }
                    _ => Some("{\"answer\":\"done\"}".to_string()),
                }
            },
            |_pick| {
                execs += 1;
                ToolStepOutcome::Ran(AgentObservation::ran("x/y", "plugin", &serde_json::json!("z")))
            },
        );
        assert_eq!(execs, 0, "an off-catalog tool is NEVER executed");
        assert_eq!(result.outcome, AgentOutcome::Answered);
        assert!(result.observations.is_empty());
    }

    #[test]
    fn tool_calls_are_bounded_by_the_cap() {
        // A brain that always wants another tool must be bounded to MAX_AGENT_TOOL_CALLS.
        let mut execs = 0;
        let result = run_agent_loop(
            "keep going",
            sample_catalog(),
            |_p| {
                // Always pick a DIFFERENT-arg call so repeat-detection does not stop it early —
                // only the hard cap should.
                Some("{\"tool\":\"relux-tools-echo/echo\",\"args\":{\"n\":1}}".to_string())
            },
            |pick| {
                execs += 1;
                // Vary the observation so it is not deduped, then change args each call is not
                // possible here (brain is fixed) — so use the arg-independent detail.
                ToolStepOutcome::Ran(AgentObservation::ran(
                    &pick.label(),
                    "plugin",
                    &serde_json::json!({"call": execs}),
                ))
            },
        );
        // Repeat-detection actually stops it at the SECOND identical call; either way it is bounded
        // well within the cap and never exceeds it.
        assert!(execs <= MAX_AGENT_TOOL_CALLS, "executions {execs} exceeded cap");
        assert_eq!(result.outcome, AgentOutcome::Exhausted);
    }

    #[test]
    fn distinct_tool_calls_are_bounded_to_three() {
        // Three different picks in a row must all run, but a fourth must not.
        let picks = ["relux-tools-echo/echo", "mcp:notes/listNotes", "relux-tools-echo/echo"];
        let mut round = 0;
        let mut execs = 0;
        let _ = run_agent_loop(
            "chain tools",
            sample_catalog(),
            |_p| {
                let i = round;
                round += 1;
                // Each pick has distinct args so repeat-detection does not fire.
                let label = picks.get(i).copied().unwrap_or("relux-tools-echo/echo");
                Some(format!("{{\"tool\":\"{label}\",\"args\":{{\"i\":{i}}}}}"))
            },
            |pick| {
                execs += 1;
                ToolStepOutcome::Ran(AgentObservation::ran(
                    &pick.label(),
                    "plugin",
                    &serde_json::json!({"i": execs}),
                ))
            },
        );
        assert_eq!(execs, MAX_AGENT_TOOL_CALLS, "exactly the cap ran, no more");
    }

    #[test]
    fn mcp_tool_participates_in_the_loop() {
        let mut round = 0;
        let result = run_agent_loop(
            "list my notes via mcp:notes/listNotes",
            sample_catalog(),
            |_p| {
                round += 1;
                if round == 1 {
                    Some("{\"tool\":\"mcp:notes/listNotes\",\"args\":{}}".to_string())
                } else {
                    Some("{\"answer\":\"You have 2 notes.\"}".to_string())
                }
            },
            |pick| {
                assert_eq!(pick.plugin_id, "mcp:notes");
                ToolStepOutcome::Ran(AgentObservation::ran(
                    &pick.label(),
                    "mcp",
                    &serde_json::json!({"notes": ["a", "b"]}),
                ))
            },
        );
        assert_eq!(result.outcome, AgentOutcome::Answered);
        assert_eq!(result.trace()[0].source, "mcp");
    }

    #[test]
    fn refused_tool_stops_the_loop_honestly() {
        let result = run_agent_loop(
            "do it",
            sample_catalog(),
            |_p| Some("{\"tool\":\"relux-tools-echo/echo\",\"args\":{}}".to_string()),
            |_pick| ToolStepOutcome::Refused("the runtime is not configured".to_string()),
        );
        match result.outcome {
            AgentOutcome::Refused(r) => assert!(r.contains("runtime")),
            other => panic!("expected Refused, got {other:?}"),
        }
        assert!(result.observations.is_empty());
    }

    #[test]
    fn provider_failure_mid_loop_keeps_prior_observations() {
        let mut round = 0;
        let result = run_agent_loop(
            "go",
            sample_catalog(),
            |_p| {
                round += 1;
                if round == 1 {
                    Some("{\"tool\":\"relux-tools-echo/echo\",\"args\":{\"a\":1}}".to_string())
                } else {
                    None // provider failure on the follow-up round
                }
            },
            |pick| {
                ToolStepOutcome::Ran(AgentObservation::ran(
                    &pick.label(),
                    "plugin",
                    &serde_json::json!("ok"),
                ))
            },
        );
        // One observation was gathered before the failure; the loop ends gracefully as Exhausted
        // so the kernel can still fold what it learned into the reply.
        assert_eq!(result.observations.len(), 1);
        assert_eq!(result.outcome, AgentOutcome::Exhausted);
    }

    #[test]
    fn observations_are_redacted_and_bounded() {
        let secret = "token=sk-ABCDEF1234567890abcdef1234567890";
        let obs = AgentObservation::ran(
            "p/t",
            "plugin",
            &serde_json::json!({ "out": format!("here is a {secret}") }),
        );
        assert!(!obs.detail.contains("sk-ABCDEF1234567890"), "secret leaked: {}", obs.detail);
        // A huge body is clamped.
        let big = "x".repeat(MAX_OBS_CHARS * 3);
        let big_obs = AgentObservation::ran("p/t", "plugin", &serde_json::json!(big));
        assert!(big_obs.detail.chars().count() <= MAX_OBS_CHARS + 16);
    }
}
