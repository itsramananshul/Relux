//! The first SAFE Prime tool/context loop — READ-ONLY context tools only.
//!
//! ## Why this exists
//!
//! Every prior Prime brain stage ([`crate::prime_intent`], [`crate::prime_slots`], …, and
//! the unified [`crate::prime_decision`]) is *propose-only*: the brain answers from ONE static
//! [`relux_core::StateSummary`] snapshot baked into the prompt (bounded to a handful of tasks
//! and agents). It cannot drill into a specific task's details, inspect a run, or enumerate the
//! crew before answering. That is the gap the master plan calls out — Prime "can classify and
//! propose, but it does not inspect live control-plane state through a governed tool interface
//! before answering the way Hermes / Codex / Paperclip-like agents do"
//! (`docs/RELUX_MASTER_PLAN.md` §10.1, §17.1; `docs/prime-processing-audit.md`).
//!
//! This module adds the FIRST safe slice of that capability: a bounded, governed loop in which a
//! configured brain may request **read-only context tools** (inspect the board, a task, the
//! crew, an agent, the runs), the kernel **validates the requested tool against a read-only
//! allowlist**, **executes it deterministically against a state snapshot**, **injects the result
//! back**, and lets the brain ask again or answer. Nothing here mutates state, mints work, or
//! grants authority — it only lets Prime *look* before it speaks.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **Hermes** `agent/conversation_loop.py` `run_conversation(...)` — the per-turn agentic loop:
//!   the model's reply is inspected for tool calls (`if assistant_message.tool_calls:`), each tool
//!   is executed and its result injected back as a `role:tool` message, and the loop repeats up to
//!   a **max-iterations cap** (`while api_call_count < agent.max_iterations …`); when the model
//!   stops requesting tools the loop ends with its final answer. Crucially, the chosen tool name is
//!   **validated against `agent.valid_tool_names` BEFORE execution** — an off-list name is fed back
//!   as a `"Tool '…' does not exist. Available: …"` message for self-correction, **never executed**.
//!   We mirror exactly this shape: [`ContextLoop`] is the bounded driver ([`MAX_TOOL_ROUNDS`] is the
//!   iteration cap), [`interpret_reply`] is the tool-call detector, [`classify_tool`] is the
//!   name-allowlist gate, [`unknown_tool_feedback`] is the self-correction message, and a reply
//!   carrying no tool call ends the loop. We deliberately differ in that the brain's *answer* is
//!   still shaped by the existing action-free reply path; the loop only GATHERS read-only context.
//! - **Paperclip (openclaw)** `src/agents/tool-mutation.ts` `isMutatingToolCall(toolName, args)` —
//!   a single FAIL-CLOSED classifier mapping a tool+action to read-only vs. mutating, where an
//!   UNKNOWN action defaults to *mutating*. We invert the polarity for the same safety: [`classify_tool`]
//!   admits ONLY a name on the explicit read-only allowlist ([`READ_ONLY_TOOLS`]); ANY other name is
//!   `Refused` and never executed. The first slice ships read-only tools only, so the allowlist *is*
//!   the read-only set.
//! - **Paperclip** `src/agents/tools/common.ts` `readStringParam(…, {required})` + `ToolInputError`
//!   and `sessions-spawn-tool.ts` `UNSUPPORTED_*_PARAM_KEYS` — typed param extraction that fails on
//!   bad input and rejects unsupported keys. Mirrored in the per-tool arg reading
//!   ([`read_id_arg`] requires + sanitizes the id; an unknown/missing id yields an honest `ok:false`
//!   read, never a fabricated record).
//! - **Paperclip** `src/agents/cli-output.ts` `parseCliOutput` + `src/shared/balanced-json.ts`
//!   `extractBalancedJsonPrefix` — lift the first balanced `{...}` out of a noisy reply and surface
//!   only the parsed object. We reuse the SAME scanner ([`crate::prime_intent::extract_json_object`]);
//!   on the CLI path the driver runs `parse_adapter_result` FIRST so the raw `--output-format json`
//!   envelope never reaches [`interpret_reply`].
//!
//! ## The safety contract (binding)
//!
//! - **Read-only, full stop.** Every tool in [`READ_ONLY_TOOLS`] is a pure read of a
//!   [`ContextSnapshot`] (an owned, bounded projection of live state taken once under the kernel
//!   lock). No executor mutates anything, and there is no path from this module to `prime_execute`,
//!   an approval, or any durable change.
//! - **Fail closed on the tool name.** [`classify_tool`] executes ONLY an allowlisted name; an
//!   off-list / unknown / mutating-sounding name is `Refused`, fed back for self-correction, and
//!   never run. The brain cannot smuggle a write through a creative tool name.
//! - **Bounded.** The loop runs at most [`MAX_TOOL_ROUNDS`] rounds and stops early on a repeated
//!   call (no progress) or a final answer, so a misbehaving brain cannot spin. Each result is
//!   length-clamped ([`MAX_RESULT_CHARS`]) and lists are bounded ([`MAX_LIST_ITEMS`]).
//! - **Grounding, not authority.** Gathered reads are folded into the conversational reply's
//!   grounded facts and surfaced as provenance ([`relux_core::PrimeContextRead`]); they never become
//!   an intent, a slot, or an action. The deterministic fallback (no brain, or the loop gathers
//!   nothing) is byte-for-byte the prior reply path.

use serde::{Deserialize, Serialize};

use crate::prime_intent::extract_json_object;
use relux_core::{PrimeContextRead, PrimeIntent, PrimeTurn, StateSummary, TaskStatus};

/// Whether a (non-actionful) turn benefits from inspecting live state through the read-only tool
/// loop before answering. The loop runs only for these inspection / explanation / conversational
/// intents — a status question, an explanation request, a direct question, or brainstorming —
/// where drilling into a specific task / agent / run genuinely sharpens the answer. A greeting, a
/// plan preview, or any actionful turn needs no lookup. The caller ADDITIONALLY gates on
/// `!is_actionful(turn)`, so the loop can never run on a turn that changed state.
pub fn turn_wants_context(turn: &PrimeTurn) -> bool {
    matches!(
        turn.intent,
        PrimeIntent::StatusQuestion
            | PrimeIntent::ExplanationRequest
            | PrimeIntent::DirectAnswer
            | PrimeIntent::Brainstorming
    )
}

/// The bounded number of brain rounds the read-only loop will run for one turn — Hermes's
/// `max_iterations` cap, kept small because a context lookup needs only a few reads. A brain
/// that has not finished gathering by then simply answers with what it has.
pub const MAX_TOOL_ROUNDS: usize = 4;

/// Max characters kept from any single tool result the brain sees next round (Hermes's
/// `_sanitize_tool_error` 2000-char clamp). Keeps a large board from blowing the context.
const MAX_RESULT_CHARS: usize = 2_000;

/// Max items rendered in any list tool's output. A larger board is reported with an honest
/// "(+N more)" note rather than silently truncated.
const MAX_LIST_ITEMS: usize = 25;

/// Max characters kept from a sanitized string argument (an id). Ids are short; this only
/// guards against a pathological argument.
const MAX_ARG_CHARS: usize = 80;

/// One read-only context tool descriptor offered to the brain. Presentation/grounding only —
/// the executable surface is [`execute_context_tool`], gated by [`classify_tool`].
#[derive(Debug, Clone, Copy)]
pub struct ContextTool {
    /// The tool's wire name (what the brain puts in `{"tool": "..."}`).
    pub name: &'static str,
    /// A one-line description shown in the loop prompt.
    pub summary: &'static str,
    /// A one-line hint of the accepted `args` shape (JSON), shown in the loop prompt.
    pub args_hint: &'static str,
}

/// The explicit allowlist of READ-ONLY context tools. [`classify_tool`] executes ONLY a tool
/// whose name is in this list; everything else is refused. Because the first slice ships
/// read-only tools only, this list IS the read-only set — there is no mutating tool to classify
/// against yet (Paperclip's `isMutatingToolCall` shape, with the safe default being "not on the
/// allowlist ⇒ refused").
pub const READ_ONLY_TOOLS: &[ContextTool] = &[
    ContextTool {
        name: "board_summary",
        summary: "Counts across the whole board: tasks (open/blocked/failed/waiting), active runs, \
                  agents, plugins, pending approvals.",
        args_hint: "{}",
    },
    ContextTool {
        name: "list_tasks",
        summary: "List tasks with id, title, status, assignee, priority. Optional status filter.",
        args_hint: "{\"status\":\"<optional: queued|running|blocked|completed|failed|...>\"}",
    },
    ContextTool {
        name: "get_task",
        summary: "Full detail of one task by id (title, status, priority, assignee, details).",
        args_hint: "{\"task_id\":\"<existing task id>\"}",
    },
    ContextTool {
        name: "list_agents",
        summary: "The crew roster: agent id, name, role/description, adapter.",
        args_hint: "{}",
    },
    ContextTool {
        name: "get_agent",
        summary: "Detail of one agent by id (name, role, adapter, persona).",
        args_hint: "{\"agent_id\":\"<existing agent id>\"}",
    },
    ContextTool {
        name: "list_runs",
        summary: "Recent and active runs: run id, task, agent, status.",
        args_hint: "{}",
    },
    ContextTool {
        name: "get_run",
        summary: "Full detail of one run by id (status, task, agent, adapter, timing, summary, error).",
        args_hint: "{\"run_id\":\"<existing run id>\"}",
    },
    ContextTool {
        name: "list_plugins",
        summary: "Installed plugins/adapters: id, version, kind, enabled, protected, source, tool count.",
        args_hint: "{}",
    },
    ContextTool {
        name: "list_approvals",
        summary: "Pending and recent approvals: id, status, risk, requester, action. Optional status filter.",
        args_hint: "{\"status\":\"<optional: pending|approved|rejected>\"}",
    },
];

/// The fail-closed read-only classification of a requested tool name. The first slice has only
/// read-only tools, so the meaningful distinction is "on the allowlist (run it) vs. anything else
/// (refuse it)". Mirrors Paperclip's `isMutatingToolCall` discipline: an unknown name is treated
/// as unsafe and refused, never executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// A known read-only context tool. Safe to execute against the snapshot.
    ReadOnly,
    /// Not on the read-only allowlist. Refused — fed back for self-correction, never executed.
    Refused,
}

/// Classify a requested tool name against the read-only allowlist. Fail-closed: anything not in
/// [`READ_ONLY_TOOLS`] is [`ToolKind::Refused`].
pub fn classify_tool(name: &str) -> ToolKind {
    if READ_ONLY_TOOLS.iter().any(|t| t.name == name) {
        ToolKind::ReadOnly
    } else {
        ToolKind::Refused
    }
}

/// A compact, owned projection of one task for the read-only tools (no `input`/permissions/etc.,
/// just what Prime speaks about). Built by `KernelState::context_snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    pub assignee: Option<String>,
    pub priority: u8,
    /// A short, sanitized one-line detail lifted from the task input when present (never the raw
    /// JSON; bounded). `None` when the task carries no human-readable detail.
    pub detail: Option<String>,
}

/// A compact, owned projection of one agent for the read-only tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentView {
    pub id: String,
    pub name: String,
    pub role: String,
    pub adapter: String,
    pub persona: Option<String>,
}

/// A compact, owned projection of one run for the read-only tools. `list_runs` renders only the
/// first four fields; `get_run` additionally surfaces the adapter, logical timing, and the
/// **redacted, bounded** `summary`/`error`. The raw provider usage/cost envelope is deliberately
/// NOT projected here (never shipped to the brain or the UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunView {
    pub id: String,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    /// The adapter plugin id that executed (or would execute) this run.
    pub adapter: String,
    /// The kernel logical-clock start/end stamps (NOT wall-clock instants), when present.
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// The real measured wall-clock duration of the adapter subprocess, in ms; only set for CLI
    /// adapter runs.
    pub duration_ms: Option<u64>,
    /// A short, redacted one-line run summary lifted from the run record (never the raw envelope).
    pub summary: Option<String>,
    /// A short, redacted one-line error, when the run failed.
    pub error: Option<String>,
}

/// A compact, owned projection of one installed plugin/adapter for the read-only tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginView {
    pub id: String,
    pub version: String,
    /// The plugin kind label (`Adapter` / `ToolSet` / …).
    pub kind: String,
    pub enabled: bool,
    /// Whether the plugin is a protected (bundled) fixture — it cannot be removed.
    pub protected: bool,
    /// How the plugin entered the index (`Bundled` / `LocalDir` / `Zip` / `Github`). The raw
    /// `source_label` (a local path / URL) is deliberately NOT projected.
    pub source_kind: String,
    /// The number of tools the plugin's manifest declares.
    pub tools: usize,
}

/// A compact, owned projection of one approval for the read-only tools. The `action`/`reason` are
/// already human-readable renderings (no secret/token), and are further redacted + bounded here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalView {
    pub id: String,
    /// The approval lifecycle label (`pending` / `approved` / `rejected`).
    pub status: String,
    /// The risk label of the proposed action (`low` / `medium` / `high` / …).
    pub risk: String,
    pub requested_by: String,
    /// A redacted, bounded one-line rendering of the proposed action.
    pub action: String,
    /// A redacted, bounded one-line reason the action needs approval.
    pub reason: String,
}

/// An owned, bounded snapshot of the control-plane state the read-only tools read from. Taken
/// once under the kernel lock (`KernelState::context_snapshot`) so the loop's brain calls run
/// OUTSIDE the lock and the executors stay pure and unit-testable without a kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// The same board counts the decision prompt already uses (reused for `board_summary`).
    pub summary: StateSummary,
    pub tasks: Vec<TaskView>,
    pub agents: Vec<AgentView>,
    pub runs: Vec<RunView>,
    pub plugins: Vec<PluginView>,
    pub approvals: Vec<ApprovalView>,
}

/// A validated, allowlisted read-only tool call the brain requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The allowlisted tool name (already passed [`classify_tool`]).
    pub tool: String,
    /// The raw arguments object the brain supplied (validated per-tool by the executor).
    pub args: serde_json::Map<String, serde_json::Value>,
}

/// How one brain reply inside the loop is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainTurn {
    /// A valid, allowlisted read-only tool call to execute.
    Call(ToolCall),
    /// A tool name NOT on the allowlist — fed back for self-correction (Hermes), never executed.
    UnknownTool(String),
    /// No tool call (a final answer / an explicit `{"done": true}` / no JSON) — stop gathering.
    Done,
}

/// One executed read-only context read: the tool that ran, whether it found what was asked, a
/// one-line human summary (for the provenance chip), and the clamped detail body the brain sees
/// next round. Honest by construction: a missing id yields `ok: false` with a "not found" body,
/// never a fabricated record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRead {
    pub tool: String,
    pub ok: bool,
    pub summary: String,
    pub detail: String,
}

impl ContextRead {
    /// Project this read to the bounded wire provenance type (tool + ok + summary only — the
    /// full detail body stays server-side grounding, never shipped to the client).
    pub fn to_wire(&self) -> PrimeContextRead {
        PrimeContextRead {
            tool: self.tool.clone(),
            ok: self.ok,
            summary: self.summary.clone(),
        }
    }
}

/// Strip control chars, collapse whitespace, and clamp a model/state string to `max` chars.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// Read a required id-shaped string argument: trim, sanitize, and length-clamp it. Returns `None`
/// when the key is missing or empty (the executor then reports an honest "not found"), mirroring
/// Paperclip's `readStringParam(..., {required})` failing on bad input rather than coercing.
fn read_id_arg(args: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    let raw = args.get(key)?.as_str()?;
    let cleaned = sanitize_line(raw, MAX_ARG_CHARS);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// The self-correction message fed back when the brain names a tool that is not on the read-only
/// allowlist (Hermes's `"Tool '…' does not exist. Available: …"`). The loop re-prompts with this;
/// it never executes the off-list name.
pub fn unknown_tool_feedback(name: &str) -> String {
    let available = READ_ONLY_TOOLS
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Tool '{}' is not available. These read-only tools are available: {}. \
         Request one of them, or reply {{\"done\": true}} if you have enough to answer.",
        sanitize_line(name, MAX_ARG_CHARS),
        available
    )
}

/// Interpret one brain reply inside the loop: detect a tool call, validate its name against the
/// allowlist, or end the loop. Pure. Reuses the shared balanced-brace scanner so a reply wrapped
/// in prose/fences still parses; a reply with no JSON object is a final answer ([`BrainTurn::Done`]).
pub fn interpret_reply(raw: &str) -> BrainTurn {
    let Some(json) = extract_json_object(raw) else {
        return BrainTurn::Done;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return BrainTurn::Done;
    };
    let Some(obj) = value.as_object() else {
        return BrainTurn::Done;
    };
    // An explicit done signal ends gathering.
    if obj.get("done").and_then(|v| v.as_bool()) == Some(true) {
        return BrainTurn::Done;
    }
    let tool = match obj.get("tool").and_then(|v| v.as_str()) {
        Some(t) => sanitize_line(t, MAX_ARG_CHARS),
        // No `tool` key and no `done` flag: treat as a final answer rather than re-prompting.
        None => return BrainTurn::Done,
    };
    if tool.is_empty() || tool.eq_ignore_ascii_case("none") {
        return BrainTurn::Done;
    }
    match classify_tool(&tool) {
        ToolKind::ReadOnly => {
            let args = obj
                .get("args")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            BrainTurn::Call(ToolCall { tool, args })
        }
        ToolKind::Refused => BrainTurn::UnknownTool(tool),
    }
}

/// Validate ONE read-only tool request object (`{"tool":..,"args":..}`) the brain proposed inside
/// its UNIFIED decision envelope ([`crate::prime_decision`]), returning an allowlisted [`ToolCall`]
/// or `None`.
///
/// Fail-closed, exactly like [`interpret_reply`]'s tool branch: the name is sanitized and run
/// through [`classify_tool`]; a name NOT on the read-only allowlist (a mutating / unknown / made-up
/// tool such as `delete_task` or `run_shell`) is rejected (`None`), never executed. The `args`
/// object is carried through to the executor, which sanitizes each id at read time. Pure. This is
/// the parse-time gate for the unified-decision tool-request path; the deterministic runtime
/// execution is [`execute_requested_reads`].
pub fn validate_tool_request(value: &serde_json::Value) -> Option<ToolCall> {
    let obj = value.as_object()?;
    let raw = obj.get("tool").and_then(|v| v.as_str())?;
    let tool = sanitize_line(raw, MAX_ARG_CHARS);
    if classify_tool(&tool) != ToolKind::ReadOnly {
        return None;
    }
    let args = obj
        .get("args")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    Some(ToolCall { tool, args })
}

/// Execute one allowlisted read-only tool against the snapshot. PURE: reads only, mutates
/// nothing, fabricates nothing. An unknown id is an honest `ok: false` read. The `tool` is
/// assumed already validated by [`classify_tool`]; a name that somehow is not handled returns a
/// refused read defensively (it can never reach an executor through [`interpret_reply`]).
pub fn execute_context_tool(snapshot: &ContextSnapshot, call: &ToolCall) -> ContextRead {
    match call.tool.as_str() {
        "board_summary" => board_summary_read(snapshot),
        "list_tasks" => list_tasks_read(snapshot, &call.args),
        "get_task" => get_task_read(snapshot, &call.args),
        "list_agents" => list_agents_read(snapshot),
        "get_agent" => get_agent_read(snapshot, &call.args),
        "list_runs" => list_runs_read(snapshot),
        "get_run" => get_run_read(snapshot, &call.args),
        "list_plugins" => list_plugins_read(snapshot),
        "list_approvals" => list_approvals_read(snapshot, &call.args),
        other => ContextRead {
            tool: other.to_string(),
            ok: false,
            summary: format!("'{other}' is not a read-only tool"),
            detail: unknown_tool_feedback(other),
        },
    }
}

fn clamp_detail(detail: String) -> String {
    if detail.chars().count() <= MAX_RESULT_CHARS {
        detail
    } else {
        let mut out: String = detail.chars().take(MAX_RESULT_CHARS).collect();
        out.push_str("\n(truncated)");
        out
    }
}

fn board_summary_read(snapshot: &ContextSnapshot) -> ContextRead {
    let s = &snapshot.summary;
    let detail = format!(
        "tasks_total={} tasks_open={} tasks_blocked={} tasks_failed={} tasks_waiting_approval={} \
runs_active={} agents={} plugins={} pending_approvals={}",
        s.tasks_total,
        s.tasks_open,
        s.tasks_blocked,
        s.tasks_failed,
        s.tasks_waiting_approval,
        s.runs_active,
        s.agents,
        s.plugins,
        s.pending_approvals,
    );
    ContextRead {
        tool: "board_summary".to_string(),
        ok: true,
        summary: format!(
            "{} tasks ({} open), {} active runs, {} agents",
            s.tasks_total, s.tasks_open, s.runs_active, s.agents
        ),
        detail: clamp_detail(detail),
    }
}

/// Render a bounded list of lines with an honest "(+N more)" tail when the list overflows.
fn bounded_lines<T>(items: &[T], mut line: impl FnMut(&T) -> String) -> String {
    let shown = items.len().min(MAX_LIST_ITEMS);
    let mut out: Vec<String> = items.iter().take(shown).map(&mut line).collect();
    if items.len() > shown {
        out.push(format!("(+{} more)", items.len() - shown));
    }
    out.join("\n")
}

fn list_tasks_read(
    snapshot: &ContextSnapshot,
    args: &serde_json::Map<String, serde_json::Value>,
) -> ContextRead {
    // Optional status filter: honored only when it parses to a real `TaskStatus`; an
    // unrecognized filter is ignored (all tasks listed) rather than failing.
    let filter: Option<TaskStatus> = args
        .get("status")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s.trim().to_lowercase())).ok());
    let matched: Vec<&TaskView> = snapshot
        .tasks
        .iter()
        .filter(|t| filter.as_ref().is_none_or(|f| &t.status == f))
        .collect();
    let detail = if matched.is_empty() {
        "(no matching tasks)".to_string()
    } else {
        bounded_lines(&matched, |t| {
            let assignee = t.assignee.as_deref().unwrap_or("unassigned");
            format!(
                "{}: \"{}\" [{}] assignee={} priority={}",
                t.id,
                t.title,
                status_label(&t.status),
                assignee,
                t.priority
            )
        })
    };
    let label = match &filter {
        Some(f) => format!("{} {} task(s)", matched.len(), status_label(f)),
        None => format!("{} task(s)", matched.len()),
    };
    ContextRead {
        tool: "list_tasks".to_string(),
        ok: true,
        summary: label,
        detail: clamp_detail(detail),
    }
}

fn get_task_read(
    snapshot: &ContextSnapshot,
    args: &serde_json::Map<String, serde_json::Value>,
) -> ContextRead {
    let Some(id) = read_id_arg(args, "task_id") else {
        return ContextRead {
            tool: "get_task".to_string(),
            ok: false,
            summary: "get_task needs a task_id".to_string(),
            detail: "Provide {\"task_id\":\"<existing task id>\"}.".to_string(),
        };
    };
    match snapshot.tasks.iter().find(|t| t.id == id) {
        Some(t) => {
            let assignee = t.assignee.as_deref().unwrap_or("unassigned");
            let mut detail = format!(
                "id={}\ntitle=\"{}\"\nstatus={}\npriority={}\nassignee={}",
                t.id,
                t.title,
                status_label(&t.status),
                t.priority,
                assignee
            );
            if let Some(d) = &t.detail {
                detail.push_str(&format!("\ndetail=\"{d}\""));
            }
            ContextRead {
                tool: "get_task".to_string(),
                ok: true,
                summary: format!("{}: \"{}\" [{}]", t.id, t.title, status_label(&t.status)),
                detail: clamp_detail(detail),
            }
        }
        None => ContextRead {
            tool: "get_task".to_string(),
            ok: false,
            summary: format!("no task {id}"),
            detail: format!("Task '{id}' does not exist on the board."),
        },
    }
}

fn list_agents_read(snapshot: &ContextSnapshot) -> ContextRead {
    let detail = if snapshot.agents.is_empty() {
        "(no agents on the roster)".to_string()
    } else {
        bounded_lines(&snapshot.agents, |a| {
            format!("{}: \"{}\" role=\"{}\" adapter={}", a.id, a.name, a.role, a.adapter)
        })
    };
    ContextRead {
        tool: "list_agents".to_string(),
        ok: true,
        summary: format!("{} agent(s) on the roster", snapshot.agents.len()),
        detail: clamp_detail(detail),
    }
}

fn get_agent_read(
    snapshot: &ContextSnapshot,
    args: &serde_json::Map<String, serde_json::Value>,
) -> ContextRead {
    let Some(id) = read_id_arg(args, "agent_id") else {
        return ContextRead {
            tool: "get_agent".to_string(),
            ok: false,
            summary: "get_agent needs an agent_id".to_string(),
            detail: "Provide {\"agent_id\":\"<existing agent id>\"}.".to_string(),
        };
    };
    match snapshot.agents.iter().find(|a| a.id == id) {
        Some(a) => {
            let mut detail = format!(
                "id={}\nname=\"{}\"\nrole=\"{}\"\nadapter={}",
                a.id, a.name, a.role, a.adapter
            );
            if let Some(p) = &a.persona {
                detail.push_str(&format!("\npersona=\"{p}\""));
            }
            ContextRead {
                tool: "get_agent".to_string(),
                ok: true,
                summary: format!("{}: \"{}\"", a.id, a.name),
                detail: clamp_detail(detail),
            }
        }
        None => ContextRead {
            tool: "get_agent".to_string(),
            ok: false,
            summary: format!("no agent {id}"),
            detail: format!("Agent '{id}' does not exist on the roster."),
        },
    }
}

fn list_runs_read(snapshot: &ContextSnapshot) -> ContextRead {
    let detail = if snapshot.runs.is_empty() {
        "(no runs yet)".to_string()
    } else {
        bounded_lines(&snapshot.runs, |r| {
            format!("{}: task={} agent={} status={}", r.id, r.task_id, r.agent_id, r.status)
        })
    };
    ContextRead {
        tool: "list_runs".to_string(),
        ok: true,
        summary: format!("{} run(s)", snapshot.runs.len()),
        detail: clamp_detail(detail),
    }
}

fn get_run_read(
    snapshot: &ContextSnapshot,
    args: &serde_json::Map<String, serde_json::Value>,
) -> ContextRead {
    let Some(id) = read_id_arg(args, "run_id") else {
        return ContextRead {
            tool: "get_run".to_string(),
            ok: false,
            summary: "get_run needs a run_id".to_string(),
            detail: "Provide {\"run_id\":\"<existing run id>\"}.".to_string(),
        };
    };
    match snapshot.runs.iter().find(|r| r.id == id) {
        Some(r) => {
            let mut detail = format!(
                "id={}\nstatus={}\ntask={}\nagent={}\nadapter={}",
                r.id, r.status, r.task_id, r.agent_id, r.adapter
            );
            if let Some(started) = &r.started_at {
                detail.push_str(&format!("\nstarted_at={started}"));
            }
            if let Some(ended) = &r.ended_at {
                detail.push_str(&format!("\nended_at={ended}"));
            }
            if let Some(ms) = r.duration_ms {
                detail.push_str(&format!("\nduration_ms={ms}"));
            }
            if let Some(s) = &r.summary {
                detail.push_str(&format!("\nsummary=\"{s}\""));
            }
            if let Some(e) = &r.error {
                detail.push_str(&format!("\nerror=\"{e}\""));
            }
            ContextRead {
                tool: "get_run".to_string(),
                ok: true,
                summary: format!("{}: task={} [{}]", r.id, r.task_id, r.status),
                detail: clamp_detail(detail),
            }
        }
        None => ContextRead {
            tool: "get_run".to_string(),
            ok: false,
            summary: format!("no run {id}"),
            detail: format!("Run '{id}' does not exist."),
        },
    }
}

fn list_plugins_read(snapshot: &ContextSnapshot) -> ContextRead {
    let detail = if snapshot.plugins.is_empty() {
        "(no plugins installed)".to_string()
    } else {
        bounded_lines(&snapshot.plugins, |p| {
            format!(
                "{}: v{} [{}] enabled={} protected={} source={} tools={}",
                p.id, p.version, p.kind, p.enabled, p.protected, p.source_kind, p.tools
            )
        })
    };
    let enabled = snapshot.plugins.iter().filter(|p| p.enabled).count();
    ContextRead {
        tool: "list_plugins".to_string(),
        ok: true,
        summary: format!("{} plugin(s) ({} enabled)", snapshot.plugins.len(), enabled),
        detail: clamp_detail(detail),
    }
}

fn list_approvals_read(
    snapshot: &ContextSnapshot,
    args: &serde_json::Map<String, serde_json::Value>,
) -> ContextRead {
    // Optional status filter: honored only when it is a recognized lifecycle label; an
    // unrecognized filter is ignored (all approvals listed) rather than failing.
    let filter: Option<String> = args
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| matches!(s.as_str(), "pending" | "approved" | "rejected"));
    let matched: Vec<&ApprovalView> = snapshot
        .approvals
        .iter()
        .filter(|a| filter.as_ref().is_none_or(|f| &a.status == f))
        .collect();
    let detail = if matched.is_empty() {
        "(no matching approvals)".to_string()
    } else {
        bounded_lines(&matched, |a| {
            format!(
                "{}: [{}] risk={} by={} action=\"{}\"",
                a.id, a.status, a.risk, a.requested_by, a.action
            )
        })
    };
    let pending = snapshot.approvals.iter().filter(|a| a.status == "pending").count();
    let label = match &filter {
        Some(f) => format!("{} {} approval(s)", matched.len(), f),
        None => format!("{} approval(s) ({} pending)", snapshot.approvals.len(), pending),
    };
    ContextRead {
        tool: "list_approvals".to_string(),
        ok: true,
        summary: label,
        detail: clamp_detail(detail),
    }
}

/// The wire label for a task status (`snake_case`, matching the serialized form), for the tool
/// output the brain reads. Pure.
fn status_label(status: &TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build the loop prompt: list the read-only tools, the board counts, the reads gathered so far,
/// and (when present) the self-correction feedback. Asks the brain to request ONE more tool as
/// strict JSON or reply `{"done": true}` when it has enough to answer. Kept ASCII and
/// self-contained so it works as a one-shot CLI stdin prompt.
pub fn build_tools_prompt(
    message: &str,
    summary: &StateSummary,
    prior: &[ContextRead],
    feedback: Option<&str>,
) -> String {
    let tools = READ_ONLY_TOOLS
        .iter()
        .map(|t| format!("  - {} {} args={}", t.name, t.summary, t.args_hint))
        .collect::<Vec<_>>()
        .join("\n");
    let board = format!(
        "tasks_total={} tasks_open={} runs_active={} agents={} plugins={} pending_approvals={}",
        summary.tasks_total,
        summary.tasks_open,
        summary.runs_active,
        summary.agents,
        summary.plugins,
        summary.pending_approvals,
    );
    let gathered = if prior.is_empty() {
        "  (none yet)".to_string()
    } else {
        prior
            .iter()
            .map(|r| format!("  - {} -> {}\n{}", r.tool, r.summary, indent(&r.detail)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let feedback_block = match feedback {
        Some(f) => format!("\nNote: {f}\n"),
        None => String::new(),
    };
    format!(
        "You are Prime, the operator of a local Relux control plane. To answer the user's message \
you may FIRST inspect live state with READ-ONLY tools. You perform no action and change nothing: \
these tools only read. Use plain ASCII.\n\n\
Read-only tools available:\n{tools}\n\n\
Board counts:\n  {board}\n\n\
Context you have gathered so far:\n{gathered}\n{feedback_block}\n\
Respond with JSON ONLY (no prose, no code fences). Either request ONE tool:\n\
  {{\"tool\":\"<one tool name above>\",\"args\":{{...}}}}\n\
or, when you have enough to answer the user, reply:\n\
  {{\"done\": true}}\n\n\
Request a tool only when it helps answer THIS message; do not loop pointlessly. Never invent a \
tool name, a task id, or an agent id.\n\n\
User message:\n{message}"
    )
}

/// Indent a multi-line tool-result body under its bullet in the prompt.
fn indent(body: &str) -> String {
    body.lines()
        .map(|l| format!("      {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bounded, stateful driver for the read-only context loop. Holds the snapshot + the reads
/// gathered so far; the async drivers (OpenRouter / CLI) and the synchronous test helper
/// ([`run_context_loop`]) share this exact control flow, so the loop logic (round cap, allowlist
/// validation, self-correction, read-only execution, stop-on-repeat) is pinned once and never
/// drifts between the provider paths.
pub struct ContextLoop {
    message: String,
    snapshot: ContextSnapshot,
    reads: Vec<ContextRead>,
    feedback: Option<String>,
    round: usize,
}

impl ContextLoop {
    /// Start a loop for one user message over a state snapshot. The snapshot is cloned in (it is
    /// bounded) so the loop is self-contained and the executors stay pure over it.
    pub fn new(message: &str, snapshot: &ContextSnapshot) -> Self {
        Self {
            message: message.to_string(),
            snapshot: snapshot.clone(),
            reads: Vec::new(),
            feedback: None,
            round: 0,
        }
    }

    /// The next prompt to send the brain, or `None` when the loop has hit its round cap (gather is
    /// over). Pure: does not advance the round (that happens in [`Self::observe`]).
    pub fn next_prompt(&self) -> Option<String> {
        if self.round >= MAX_TOOL_ROUNDS {
            return None;
        }
        Some(build_tools_prompt(
            &self.message,
            &self.snapshot.summary,
            &self.reads,
            self.feedback.as_deref(),
        ))
    }

    /// Observe the brain's reply to the last prompt, executing any requested allowlisted tool
    /// against the snapshot and advancing the loop. Returns `true` to continue (re-prompt),
    /// `false` when the loop is done (final answer, an off-list-only refusal would continue, a
    /// repeated call with no progress, or the round budget about to be exceeded handled by
    /// `next_prompt`). An off-allowlist name is recorded as self-correction feedback and NOT
    /// executed.
    pub fn observe(&mut self, raw: &str) -> bool {
        self.round += 1;
        self.feedback = None;
        match interpret_reply(raw) {
            BrainTurn::Done => false,
            BrainTurn::UnknownTool(name) => {
                self.feedback = Some(unknown_tool_feedback(&name));
                true
            }
            BrainTurn::Call(call) => {
                // Stop on a repeated identical call: the brain is not making progress, so spending
                // another round (and another provider call) on the same read is wasteful.
                if self
                    .reads
                    .iter()
                    .any(|r| r.tool == call.tool && r.detail == execute_context_tool(&self.snapshot, &call).detail)
                {
                    return false;
                }
                self.reads.push(execute_context_tool(&self.snapshot, &call));
                true
            }
        }
    }

    /// The reads gathered so far.
    pub fn reads(&self) -> &[ContextRead] {
        &self.reads
    }

    /// Consume the loop and take the gathered reads.
    pub fn into_reads(self) -> Vec<ContextRead> {
        self.reads
    }
}

/// Drive the bounded read-only context loop with a SYNCHRONOUS brain closure. The async drivers
/// wrap their network/process call; this synchronous form is the testable twin that pins the loop
/// behavior with a scripted brain and NO provider. The closure returns the brain's raw reply, or
/// `None` to abort the loop (a provider failure) — exactly what the async drivers do.
pub fn run_context_loop<F: FnMut(&str) -> Option<String>>(
    message: &str,
    snapshot: &ContextSnapshot,
    mut brain: F,
) -> Vec<ContextRead> {
    let mut lp = ContextLoop::new(message, snapshot);
    while let Some(prompt) = lp.next_prompt() {
        let Some(raw) = brain(&prompt) else {
            break;
        };
        if !lp.observe(&raw) {
            break;
        }
    }
    lp.into_reads()
}

/// Execute a bounded, PRE-VALIDATED list of read-only tool requests against the snapshot,
/// returning the gathered reads. This is the UNIFIED-DECISION counterpart of the multi-round
/// [`ContextLoop`]: when the brain requests its read-only tools UP FRONT in one decision envelope
/// (instead of over several loop rounds), the kernel executes them here deterministically with NO
/// further brain call, then grounds the reply in the observations — so the unified path issues one
/// fewer brain round than the sidecar loop and never executes the same reads twice.
///
/// PURE: reads only, mutates nothing, fabricates nothing — every entry is already an allowlisted
/// [`ToolCall`] (see [`validate_tool_request`]), so no mutating/unknown name can reach an executor.
/// Bounded exactly like the loop: at most [`MAX_TOOL_ROUNDS`] reads run (extra requests are dropped,
/// matching the round cap), and a repeated identical read (same tool + body) is skipped so a brain
/// that lists the same tool twice does not double-count.
pub fn execute_requested_reads(snapshot: &ContextSnapshot, calls: &[ToolCall]) -> Vec<ContextRead> {
    let mut reads: Vec<ContextRead> = Vec::new();
    for call in calls.iter().take(MAX_TOOL_ROUNDS) {
        let read = execute_context_tool(snapshot, call);
        if reads.iter().any(|r| r.tool == read.tool && r.detail == read.detail) {
            continue;
        }
        reads.push(read);
    }
    reads
}

/// The read-only tool names, comma-joined — grounding for the unified decision prompt's
/// `tool_requests` section so the brain names only a real read-only tool. Pure.
pub fn read_only_tool_names() -> String {
    READ_ONLY_TOOLS
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render the gathered reads as a compact grounded-facts block to fold into the conversational
/// reply prompt, or an empty string when nothing was gathered. The brain that shapes the final
/// reply treats these as factual reads it performed this turn. Bounded by construction (each read
/// is already clamped). Pure.
pub fn render_observations(reads: &[ContextRead]) -> String {
    if reads.is_empty() {
        return String::new();
    }
    reads
        .iter()
        .map(|r| format!("[{}] {}\n{}", r.tool, r.summary, r.detail))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Project the gathered reads to the bounded wire provenance list.
pub fn reads_to_wire(reads: &[ContextRead]) -> Vec<PrimeContextRead> {
    reads.iter().map(ContextRead::to_wire).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(tasks_total: usize, agents: usize) -> StateSummary {
        StateSummary {
            plugins: 1,
            agents,
            tasks_total,
            tasks_open: tasks_total,
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: vec![],
            agent_skills: vec![],
            all_task_ids: vec![],
            queued: vec![],
            recent: vec![],
        }
    }

    fn snapshot() -> ContextSnapshot {
        ContextSnapshot {
            summary: summary(2, 1),
            tasks: vec![
                TaskView {
                    id: "task_0001".to_string(),
                    title: "Fix the login redirect".to_string(),
                    status: TaskStatus::Queued,
                    assignee: Some("researcher".to_string()),
                    priority: 7,
                    detail: Some("Users land on a blank page after SSO.".to_string()),
                },
                TaskView {
                    id: "task_0002".to_string(),
                    title: "Write the README".to_string(),
                    status: TaskStatus::Blocked,
                    assignee: None,
                    priority: 5,
                    detail: None,
                },
            ],
            agents: vec![AgentView {
                id: "researcher".to_string(),
                name: "Research Agent".to_string(),
                role: "Surveys options".to_string(),
                adapter: "relux-adapter-local-prime".to_string(),
                persona: Some("Methodical and concise.".to_string()),
            }],
            runs: vec![RunView {
                id: "run_0001".to_string(),
                task_id: "task_0001".to_string(),
                agent_id: "researcher".to_string(),
                status: "running".to_string(),
                adapter: "relux-adapter-local-prime".to_string(),
                started_at: Some("t0".to_string()),
                ended_at: None,
                duration_ms: None,
                summary: Some("Surveying the login flow.".to_string()),
                error: None,
            }],
            plugins: vec![
                PluginView {
                    id: "relux-adapter-local-prime".to_string(),
                    version: "0.1.0".to_string(),
                    kind: "Adapter".to_string(),
                    enabled: true,
                    protected: true,
                    source_kind: "Bundled".to_string(),
                    tools: 1,
                },
                PluginView {
                    id: "relux-tools-github".to_string(),
                    version: "0.2.0".to_string(),
                    kind: "ToolSet".to_string(),
                    enabled: false,
                    protected: false,
                    source_kind: "Github".to_string(),
                    tools: 3,
                },
            ],
            approvals: vec![ApprovalView {
                id: "appr_0001".to_string(),
                status: "pending".to_string(),
                risk: "high".to_string(),
                requested_by: "prime".to_string(),
                action: "grant tool:relux-tools-github:create_pr to code-agent".to_string(),
                reason: "Granting a permission widens what an actor can do.".to_string(),
            }],
        }
    }

    #[test]
    fn classify_is_fail_closed_on_unknown_names() {
        assert_eq!(classify_tool("get_task"), ToolKind::ReadOnly);
        assert_eq!(classify_tool("board_summary"), ToolKind::ReadOnly);
        assert_eq!(classify_tool("get_run"), ToolKind::ReadOnly);
        assert_eq!(classify_tool("list_plugins"), ToolKind::ReadOnly);
        assert_eq!(classify_tool("list_approvals"), ToolKind::ReadOnly);
        // Anything not on the allowlist is refused — including a plausible-sounding write.
        assert_eq!(classify_tool("delete_task"), ToolKind::Refused);
        assert_eq!(classify_tool("create_task"), ToolKind::Refused);
        assert_eq!(classify_tool("run_shell"), ToolKind::Refused);
        assert_eq!(classify_tool(""), ToolKind::Refused);
    }

    #[test]
    fn interpret_detects_calls_unknown_tools_and_done() {
        // A clean tool call.
        match interpret_reply(r#"{"tool":"get_task","args":{"task_id":"task_0001"}}"#) {
            BrainTurn::Call(c) => {
                assert_eq!(c.tool, "get_task");
                assert_eq!(c.args.get("task_id").unwrap().as_str(), Some("task_0001"));
            }
            other => panic!("expected a call, got {other:?}"),
        }
        // A noisy reply (prose + fences) still parses to the call.
        assert!(matches!(
            interpret_reply("Let me check.\n```json\n{\"tool\":\"list_tasks\",\"args\":{}}\n```"),
            BrainTurn::Call(_)
        ));
        // An off-allowlist name is flagged for self-correction, never a call.
        assert_eq!(
            interpret_reply(r#"{"tool":"delete_task","args":{"task_id":"task_0001"}}"#),
            BrainTurn::UnknownTool("delete_task".to_string())
        );
        // An explicit done, a `none`, a missing tool, and plain prose all end the loop.
        assert_eq!(interpret_reply(r#"{"done": true}"#), BrainTurn::Done);
        assert_eq!(interpret_reply(r#"{"tool":"none"}"#), BrainTurn::Done);
        assert_eq!(interpret_reply(r#"{"answer":"all good"}"#), BrainTurn::Done);
        assert_eq!(interpret_reply("Here is your answer."), BrainTurn::Done);
    }

    #[test]
    fn execute_reads_real_state_and_is_honest_about_misses() {
        let snap = snapshot();
        // board_summary
        let r = execute_context_tool(&snap, &ToolCall { tool: "board_summary".into(), args: Default::default() });
        assert!(r.ok && r.detail.contains("tasks_total=2"));
        // get_task hit
        let mut args = serde_json::Map::new();
        args.insert("task_id".into(), "task_0001".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_task".into(), args });
        assert!(r.ok && r.detail.contains("Fix the login redirect") && r.detail.contains("blank page"));
        // get_task miss -> honest not-found, never fabricated
        let mut args = serde_json::Map::new();
        args.insert("task_id".into(), "task_9999".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_task".into(), args });
        assert!(!r.ok && r.detail.contains("does not exist"));
        // get_task with no id -> honest prompt for the id
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_task".into(), args: Default::default() });
        assert!(!r.ok && r.summary.contains("needs a task_id"));
    }

    #[test]
    fn list_tasks_honors_an_optional_status_filter() {
        let snap = snapshot();
        let mut args = serde_json::Map::new();
        args.insert("status".into(), "blocked".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "list_tasks".into(), args });
        assert!(r.ok);
        assert!(r.detail.contains("task_0002") && !r.detail.contains("task_0001"));
        assert!(r.summary.contains("blocked"));
        // An unrecognized filter is ignored (all tasks listed), never an error.
        let mut args = serde_json::Map::new();
        args.insert("status".into(), "wobbly".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "list_tasks".into(), args });
        assert!(r.detail.contains("task_0001") && r.detail.contains("task_0002"));
    }

    #[test]
    fn get_run_reads_real_runs_and_is_honest_about_misses() {
        let snap = snapshot();
        // A real run id: the detail carries the redacted summary, adapter, and timing.
        let mut args = serde_json::Map::new();
        args.insert("run_id".into(), "run_0001".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_run".into(), args });
        assert!(r.ok);
        assert!(r.detail.contains("task=task_0001") && r.detail.contains("status=running"));
        assert!(r.detail.contains("adapter=relux-adapter-local-prime"));
        assert!(r.detail.contains("Surveying the login flow"));
        // Raw provider usage/cost is never projected, so it can never leak into the body.
        assert!(!r.detail.contains("usage") && !r.detail.contains("cost"));
        // An unknown run id -> honest miss, never fabricated.
        let mut args = serde_json::Map::new();
        args.insert("run_id".into(), "run_9999".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_run".into(), args });
        assert!(!r.ok && r.detail.contains("does not exist"));
        // No run id -> honest prompt for the id.
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_run".into(), args: Default::default() });
        assert!(!r.ok && r.summary.contains("needs a run_id"));
    }

    #[test]
    fn list_plugins_reports_enabled_protected_and_tool_counts() {
        let snap = snapshot();
        let r = execute_context_tool(
            &snap,
            &ToolCall { tool: "list_plugins".into(), args: Default::default() },
        );
        assert!(r.ok);
        assert!(r.summary.contains("2 plugin(s)") && r.summary.contains("1 enabled"));
        assert!(r.detail.contains("relux-adapter-local-prime") && r.detail.contains("protected=true"));
        assert!(r.detail.contains("relux-tools-github") && r.detail.contains("enabled=false"));
        assert!(r.detail.contains("tools=3"));
    }

    #[test]
    fn list_approvals_honors_an_optional_status_filter() {
        let snap = snapshot();
        // No filter: lists all, names the pending count.
        let r = execute_context_tool(
            &snap,
            &ToolCall { tool: "list_approvals".into(), args: Default::default() },
        );
        assert!(r.ok && r.summary.contains("1 pending"));
        assert!(r.detail.contains("appr_0001") && r.detail.contains("risk=high"));
        // A 'pending' filter matches the one pending approval.
        let mut args = serde_json::Map::new();
        args.insert("status".into(), "pending".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "list_approvals".into(), args });
        assert!(r.detail.contains("appr_0001") && r.summary.contains("pending"));
        // An 'approved' filter matches nothing (honest empty), never an error.
        let mut args = serde_json::Map::new();
        args.insert("status".into(), "approved".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "list_approvals".into(), args });
        assert!(r.ok && r.detail.contains("(no matching approvals)"));
        // An unrecognized filter is ignored (all listed), never an error.
        let mut args = serde_json::Map::new();
        args.insert("status".into(), "bogus".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "list_approvals".into(), args });
        assert!(r.ok && r.detail.contains("appr_0001"));
    }

    #[test]
    fn get_agent_reads_the_roster() {
        let snap = snapshot();
        let mut args = serde_json::Map::new();
        args.insert("agent_id".into(), "researcher".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_agent".into(), args });
        assert!(r.ok && r.detail.contains("Research Agent") && r.detail.contains("Methodical"));
        let mut args = serde_json::Map::new();
        args.insert("agent_id".into(), "ghost".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_agent".into(), args });
        assert!(!r.ok && r.detail.contains("does not exist"));
    }

    #[test]
    fn loop_gathers_validates_and_self_corrects_with_a_scripted_brain() {
        let snap = snapshot();
        // Scripted brain: first names an off-list tool (must be refused + fed back), then a real
        // read, then signals done.
        let mut script = vec![
            r#"{"tool":"drop_database","args":{}}"#.to_string(),
            r#"{"tool":"get_task","args":{"task_id":"task_0001"}}"#.to_string(),
            r#"{"done": true}"#.to_string(),
        ]
        .into_iter();
        let mut seen_unknown_feedback = false;
        let reads = run_context_loop("what is task_0001?", &snap, |prompt| {
            if prompt.contains("not available") {
                seen_unknown_feedback = true;
            }
            script.next()
        });
        // The off-list name was refused (no read recorded for it); only the real read landed.
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].tool, "get_task");
        assert!(reads[0].ok);
        assert!(seen_unknown_feedback, "the unknown-tool self-correction must reach the next prompt");
    }

    #[test]
    fn loop_is_bounded_by_the_round_cap() {
        let snap = snapshot();
        // A brain that never says done: it keeps requesting DISTINCT reads. The loop must stop at
        // the round cap regardless.
        let calls = [
            r#"{"tool":"board_summary","args":{}}"#,
            r#"{"tool":"list_tasks","args":{}}"#,
            r#"{"tool":"list_agents","args":{}}"#,
            r#"{"tool":"list_runs","args":{}}"#,
            r#"{"tool":"get_task","args":{"task_id":"task_0002"}}"#,
        ];
        let mut i = 0usize;
        let reads = run_context_loop("tell me everything", &snap, |_| {
            let c = calls[i % calls.len()].to_string();
            i += 1;
            Some(c)
        });
        assert!(reads.len() <= MAX_TOOL_ROUNDS, "the loop must not exceed the round cap");
    }

    #[test]
    fn loop_stops_on_a_repeated_call_with_no_progress() {
        let snap = snapshot();
        // The brain keeps asking for the SAME read; the loop must stop rather than spin.
        let reads = run_context_loop("hmm", &snap, |_| {
            Some(r#"{"tool":"board_summary","args":{}}"#.to_string())
        });
        assert_eq!(reads.len(), 1, "a repeated identical read must not be gathered twice");
    }

    #[test]
    fn render_observations_and_wire_projection_are_bounded_and_provenance_only() {
        let snap = snapshot();
        let reads = run_context_loop("what is task_0001?", &snap, |_| {
            Some(r#"{"tool":"get_task","args":{"task_id":"task_0001"}}"#.to_string())
        });
        let obs = render_observations(&reads);
        assert!(obs.contains("get_task") && obs.contains("Fix the login redirect"));
        let wire = reads_to_wire(&reads);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].tool, "get_task");
        assert!(wire[0].ok);
        // The wire provenance carries only the summary, never the full detail body.
        assert!(!wire[0].summary.contains("blank page"));
    }

    #[test]
    fn no_brain_gathers_nothing() {
        let snap = snapshot();
        let reads = run_context_loop("hi", &snap, |_| None);
        assert!(reads.is_empty());
        assert!(render_observations(&reads).is_empty());
    }

    #[test]
    fn validate_tool_request_is_fail_closed_on_mutating_and_unknown_names() {
        // A read-only request validates into an allowlisted ToolCall, args carried through.
        let v = serde_json::json!({"tool":"get_task","args":{"task_id":"task_0001"}});
        let call = validate_tool_request(&v).expect("a read-only request validates");
        assert_eq!(call.tool, "get_task");
        assert_eq!(call.args.get("task_id").unwrap().as_str(), Some("task_0001"));
        // A mutating / unknown / made-up name is rejected at parse time — never an executable call.
        assert!(validate_tool_request(&serde_json::json!({"tool":"delete_task"})).is_none());
        assert!(validate_tool_request(&serde_json::json!({"tool":"run_shell","args":{}})).is_none());
        assert!(validate_tool_request(&serde_json::json!({"tool":"create_task"})).is_none());
        // A missing/empty tool name or a non-object request is rejected.
        assert!(validate_tool_request(&serde_json::json!({"args":{}})).is_none());
        assert!(validate_tool_request(&serde_json::json!({"tool":""})).is_none());
        assert!(validate_tool_request(&serde_json::json!("get_task")).is_none());
    }

    #[test]
    fn execute_requested_reads_is_bounded_deduped_and_read_only() {
        let snap = snapshot();
        // A pre-validated request list (as the unified decision would carry) runs deterministically
        // with no brain round, in order.
        let calls = vec![
            ToolCall { tool: "board_summary".into(), args: Default::default() },
            ToolCall {
                tool: "get_task".into(),
                args: serde_json::Map::from_iter([("task_id".into(), "task_0001".into())]),
            },
        ];
        let reads = execute_requested_reads(&snap, &calls);
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].tool, "board_summary");
        assert!(reads[1].ok && reads[1].detail.contains("Fix the login redirect"));

        // A repeated identical read is skipped (no double-count), and the list is capped at the
        // round budget so a long request list can never spin.
        let mut many: Vec<ToolCall> =
            vec![ToolCall { tool: "board_summary".into(), args: Default::default() }; 3];
        for _ in 0..MAX_TOOL_ROUNDS + 5 {
            many.push(ToolCall { tool: "list_tasks".into(), args: Default::default() });
        }
        let reads = execute_requested_reads(&snap, &many);
        assert!(reads.len() <= MAX_TOOL_ROUNDS, "the request list is bounded by the round cap");
        // board_summary appears at most once despite three identical entries.
        assert_eq!(reads.iter().filter(|r| r.tool == "board_summary").count(), 1);
    }
}
