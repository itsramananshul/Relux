//! Deterministic + brain-assisted, VALIDATED by-id task UPDATE — the layer that
//! finally wires [`relux_core::PrimeAction::UpdateTask`] as a REAL, safe mutating
//! action instead of an always-clarify dead end.
//!
//! ## Why this exists
//!
//! `TaskUpdate` was the one resolvable-looking clarify the audit deliberately left
//! unwired (`docs/prime-processing-audit.md` "Next recommended slice"): no
//! `UpdateTask` action was executed, so the multi-turn memory could not remember a
//! "raise the priority" → "task_0001 to 8" dialogue, and the keyword
//! `task_update_clarify` could only ask, never apply. Per the master plan
//! (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §10.2 Action Layer, §17.1) a
//! by-id update should be a first-class action: the deterministic rail handles the
//! simple commands ("rename task_0001 to Fix login blank page", "set task_0001
//! priority to 8"), and a configured brain resolves the references the extractors
//! miss ("bump the readme task to high") — both validated hard before any mutation.
//!
//! ## What makes this safe (binding)
//!
//! A task update is a real mutation, but a SAFE, in-scope one (it edits an existing
//! task in the operator's own namespace; it is never risk-gated). Every change is
//! validated before it is applied:
//!
//! - **task_id** must name an EXISTING task; a TERMINAL task (completed / failed /
//!   cancelled / expired) is never edited (the kernel enforces this at apply time).
//! - **field names** are allowlisted; any other key fails the brain proposal closed.
//! - **title / details** are sanitized (control chars stripped) and length-clamped.
//! - **priority** is coerced to a number and clamped to `[1, 9]`.
//! - **status** is honored ONLY for the operator-settable allowlist
//!   ([`SETTABLE_STATUSES`] — `blocked` / `cancelled`). Prime never *decrees* a task
//!   `running` / `completed` / `failed` from chat: those flow through the run
//!   lifecycle (an honest reply is returned instead of a fake completion).
//! - **assignee** is resolved against the live roster (the same fuzzy
//!   exact→prefix→substring matcher the `AssignTask` path uses) and is ALWAYS an
//!   existing agent — the brain can never invent an assignee.
//!
//! On any failure (no brain, low confidence, invalid JSON, unsupported field,
//! unknown task/agent/status) the deterministic outcome stands — a clarify or an
//! honest reply, never a guessed mutation.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **openclaw** `src/agents/tools/update-plan-tool.ts` (`readPlanSteps`, the
//!   `PLAN_STEP_STATUSES` allowlist checked field-by-field, "at most one in_progress"
//!   clamp) — the canonical "validate a structured UPDATE payload against an explicit
//!   schema + a status ALLOWLIST" pattern. Mirrored in [`parse_update_slots`] (field
//!   allowlist) and [`parse_settable_status`] (status allowlist).
//! - **openclaw** `src/agents/tools/common.ts` (`readStringParam`, `ToolInputError`)
//!   with `src/agents/tools/sessions-spawn-tool.ts` (`UNSUPPORTED_*_PARAM_KEYS` rejected
//!   before any param is read; numeric `Math.max(0, Math.floor(...))` clamp) — reject
//!   unsupported keys up front, require/trim strings, clamp ranges, default the rest.
//! - **openclaw** `src/agents/tool-mutation.ts` (`isMutatingToolCall` — a single
//!   fail-closed classifier that defaults an unknown action to *mutating*) — informs
//!   treating the update as an explicit mutating action, applied only after validation.
//! - **openclaw** `src/auto-reply/reply/subagents-utils.ts`
//!   (`resolveSubagentTargetFromRuns`) — resolve a fuzzy reference only to an EXISTING
//!   target, reused via [`crate::prime::resolve_assignee`]; a `task_id` is likewise
//!   honored only when it exists.
//! - **Hermes** `model_tools.py` `coerce_tool_args` / `_coerce_number` +
//!   `agent/message_sanitization.py` — coerce each model arg to its schema type,
//!   sanitize control chars, CLAMP length; a bad field is dropped, not fatal.
//! - **openclaw** `src/shared/balanced-json.ts` (`extractBalancedJsonPrefix`) — lift
//!   the JSON object out of a noisy reply, reused via
//!   [`crate::prime_intent::extract_json_object`].

use relux_core::{PrimeTaskChange, StateSummary, TaskStatus};

use crate::prime::{extract_assignee_phrase, extract_task_id, resolve_assignee, AssigneeResolution};
use crate::prime_intent::extract_json_object;

/// Minimum confidence before a brain's proposed update is honored. Below this the
/// deterministic outcome (a clarify) stands.
const CONFIDENCE_FLOOR: f32 = 0.6;

/// Max characters kept for a task title (matches the create-slot / deterministic cap).
const MAX_TITLE_CHARS: usize = 120;
/// Max characters kept for optional details folded into the task input.
const MAX_DETAILS_CHARS: usize = 600;
/// Max characters kept for a normalized assignee phrase before roster resolution.
const MAX_ASSIGNEE_CHARS: usize = 64;
/// Max characters kept from the brain's free-text rationale (audit/provenance only).
const MAX_RATIONALE_CHARS: usize = 240;
/// Inclusive priority range the kernel supports (1 low … 9 high; default 5).
const PRIORITY_MIN: u8 = 1;
const PRIORITY_MAX: u8 = 9;
/// Bounds on the grounding catalogs in the brain prompt.
const MAX_PROMPT_TASKS: usize = 24;
const MAX_PROMPT_AGENTS: usize = 24;

/// The only fields a brain update proposal may carry. Any other key fails the whole
/// proposal closed (openclaw's `UNSUPPORTED_*_PARAM_KEYS` rejection) — the brain may
/// not smuggle a run/tool/permission key in as authority.
const ALLOWED_KEYS: &[&str] = &[
    "task_id",
    "title",
    "details",
    "priority",
    "status",
    "assignee",
    "confidence",
    "rationale",
];

/// The operator-settable target statuses for a conversational by-id update, and the
/// phrases that name them. This is the status ALLOWLIST (openclaw `PLAN_STEP_STATUSES`):
/// an operator may CANCEL or BLOCK a task they own, but Prime never decrees a task
/// `running` / `completed` / `failed` from chat — those are driven by the run lifecycle.
pub const SETTABLE_STATUSES: &[(&str, TaskStatus)] =
    &[("blocked", TaskStatus::Blocked), ("cancelled", TaskStatus::Cancelled)];

/// The supported updatable fields, for docs and the brain prompt.
pub const SUPPORTED_FIELDS: &[&str] = &["title", "details", "priority", "status", "assignee"];

/// A validated patch the kernel will apply to an existing task. Each present field is
/// an already-sanitized, range/allowlist-checked value; absent fields are left
/// untouched. Serialized into [`relux_core::PrimeAction::UpdateTask::patch`] (kept as a
/// string so the action stays `Eq`) and parsed back at apply time.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskUpdatePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    /// An EXISTING agent id (resolved against the roster before it lands here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

impl TaskUpdatePatch {
    /// True when the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.details.is_none()
            && self.priority.is_none()
            && self.status.is_none()
            && self.assignee.is_none()
    }

    /// Encode for the `UpdateTask` action's `patch` string. Infallible (a small,
    /// well-formed struct); `{}` on the impossible serialization error.
    pub fn to_patch_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Decode a `patch` string produced by [`Self::to_patch_string`], or `None` when
    /// it is not a valid patch object.
    pub fn from_patch_str(patch: &str) -> Option<Self> {
        serde_json::from_str(patch).ok()
    }

    /// The display change rows for the chat card, in a stable field order. Built only
    /// from what the patch actually carries (already validated).
    pub fn change_rows(&self) -> Vec<PrimeTaskChange> {
        let mut rows = Vec::new();
        if let Some(title) = &self.title {
            rows.push(row("title", title.clone()));
        }
        if let Some(details) = &self.details {
            rows.push(row("details", details.clone()));
        }
        if let Some(priority) = self.priority {
            rows.push(row("priority", priority.to_string()));
        }
        if let Some(status) = &self.status {
            rows.push(row("status", status_label(status).to_string()));
        }
        if let Some(assignee) = &self.assignee {
            rows.push(row("assignee", assignee.clone()));
        }
        rows
    }
}

fn row(field: &str, value: String) -> PrimeTaskChange {
    PrimeTaskChange {
        field: field.to_string(),
        value,
    }
}

/// The fully-validated update the kernel will apply: an EXISTING task id + a non-empty
/// validated patch. Built by [`deterministic_update`] (the rail) or
/// [`reconcile_update_slots`] (the brain), never directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTaskUpdate {
    pub task_id: String,
    pub patch: TaskUpdatePatch,
}

/// The outcome of the deterministic by-id update rail, which `decide` maps to a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeterministicUpdate {
    /// An existing task + at least one validated change → an `UpdateTask` `Act`.
    Resolved(ResolvedTaskUpdate),
    /// A `task_…` id was named but it does not exist → an honest reply (fail closed).
    UnknownTask(String),
    /// A reassignment phrase matched more than one agent → ask which (a resolvable
    /// clarify the memory can continue).
    AmbiguousAssignee { phrase: String, matches: Vec<String> },
    /// A reassignment phrase matched no agent on the roster → an honest reply.
    UnknownAssignee(String),
    /// The user asked for a status the operator cannot set from chat (e.g. "mark it
    /// done"); the label is the requested status → an honest reply, never a fake.
    RejectedStatus(&'static str),
    /// Missing task id and/or no recognizable field → ask one concrete question.
    NeedsClarification,
}

/// Which single field the deterministic rail will read for a by-id update. Conservative
/// and ordered (priority → title → assignee → status → details) so a simple command
/// resolves to exactly one change; a multi-field edit is the brain's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateField {
    Priority,
    Title,
    Assignee,
    Status,
    Details,
}

/// The one field the deterministic rail will act on for `message`, if any. Mirrors
/// the keyword priority order of `prime::update_change_phrase` (priority → title →
/// assignee → status) and adds details.
fn primary_field(message: &str) -> Option<UpdateField> {
    let m = message.to_lowercase();
    if m.contains("priority") {
        Some(UpdateField::Priority)
    } else if m.contains("rename") || m.contains("retitle") || m.contains("title") {
        Some(UpdateField::Title)
    } else if m.contains("reassign") || m.contains("assignee") || m.contains("assign") {
        Some(UpdateField::Assignee)
    } else if has_status_word(&m) {
        Some(UpdateField::Status)
    } else if m.contains("detail") || m.contains("notes") || m.contains("description") {
        Some(UpdateField::Details)
    } else {
        None
    }
}

/// True when the message names any status change (settable or rejected), so the rail
/// routes to the status field even for a status it will then honestly refuse.
fn has_status_word(m: &str) -> bool {
    parse_status_change(m).is_some()
}

/// The outcome of reading a status change from free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusOutcome {
    /// An operator-settable status (blocked / cancelled).
    Set(TaskStatus),
    /// A status the operator cannot set from chat; the label names what they asked for.
    Rejected(&'static str),
}

/// Read a status change from a (any-case) message, or `None` when none is named.
///
/// Settable: cancel(led) → `Cancelled`, block(ed) → `Blocked`. Rejected (honestly,
/// never faked): done / complete(d) / finish(ed) / resolved → `completed`; fail(ed) →
/// `failed`; running / start → `running`. "unblock" is treated as a rejected
/// "running again" because re-opening is a lifecycle action, not a status decree.
pub fn parse_status_change(message: &str) -> Option<StatusOutcome> {
    let m = message.to_lowercase();
    // Settable first.
    if m.contains("cancel") {
        return Some(StatusOutcome::Set(TaskStatus::Cancelled));
    }
    if m.contains("unblock") {
        return Some(StatusOutcome::Rejected("running again"));
    }
    if m.contains("block") {
        return Some(StatusOutcome::Set(TaskStatus::Blocked));
    }
    // Rejected (handled with an honest reply, not a fake state change).
    if has_word(&m, "done")
        || m.contains("complete")
        || m.contains("finish")
        || m.contains("resolved")
    {
        return Some(StatusOutcome::Rejected("completed"));
    }
    if has_word(&m, "fail") || has_word(&m, "failed") {
        return Some(StatusOutcome::Rejected("failed"));
    }
    if has_word(&m, "running") {
        return Some(StatusOutcome::Rejected("running"));
    }
    None
}

/// Map a settable status phrase (or canonical id) from a brain proposal to a
/// [`TaskStatus`], honoring ONLY the operator-settable allowlist. Anything else
/// (running / completed / failed / leased / …) returns `None` and is dropped — the
/// brain can never set a task to a machine-driven status.
pub fn parse_settable_status(value: &str) -> Option<TaskStatus> {
    let v = value.trim().to_lowercase();
    // Exact allowlist id, then the same forgiving phrasing the deterministic rail uses.
    for (label, status) in SETTABLE_STATUSES {
        if v == *label {
            return Some(status.clone());
        }
    }
    match parse_status_change(&v) {
        Some(StatusOutcome::Set(s)) => Some(s),
        _ => None,
    }
}

/// The deterministic by-id update rail: parse a simple update command, validate every
/// piece against the live state, and return what `decide` should do. Pure (no kernel
/// access); the terminal-state guard is enforced by the kernel at apply time.
pub fn deterministic_update(message: &str, summary: &StateSummary) -> DeterministicUpdate {
    let Some(raw_id) = extract_task_id(message) else {
        return DeterministicUpdate::NeedsClarification;
    };
    // Honor the id only when it names an EXISTING task; take the canonical casing from
    // the roster (fail closed on an unknown id).
    let Some(task_id) = summary
        .all_task_ids
        .iter()
        .find(|id| id.eq_ignore_ascii_case(&raw_id))
        .cloned()
    else {
        return DeterministicUpdate::UnknownTask(raw_id);
    };

    let mut patch = TaskUpdatePatch::default();
    match primary_field(message) {
        Some(UpdateField::Priority) => {
            patch.priority = extract_priority(message);
        }
        Some(UpdateField::Title) => {
            patch.title = value_after_to(message).map(|s| sanitize_line(&s, MAX_TITLE_CHARS)).filter(|s| !s.is_empty());
        }
        Some(UpdateField::Details) => {
            patch.details = value_after_to(message).map(|s| sanitize_line(&s, MAX_DETAILS_CHARS)).filter(|s| !s.is_empty());
        }
        Some(UpdateField::Assignee) => {
            if let Some(phrase) = extract_assignee_phrase(message) {
                match resolve_assignee(&phrase, &summary.all_agent_ids) {
                    AssigneeResolution::Resolved(id) => patch.assignee = Some(id),
                    AssigneeResolution::Ambiguous(mut matches) => {
                        matches.sort();
                        return DeterministicUpdate::AmbiguousAssignee { phrase, matches };
                    }
                    AssigneeResolution::Unresolved => {
                        return DeterministicUpdate::UnknownAssignee(phrase);
                    }
                }
            }
        }
        Some(UpdateField::Status) => match parse_status_change(message) {
            Some(StatusOutcome::Set(s)) => patch.status = Some(s),
            Some(StatusOutcome::Rejected(label)) => {
                return DeterministicUpdate::RejectedStatus(label)
            }
            None => {}
        },
        None => {}
    }

    if patch.is_empty() {
        return DeterministicUpdate::NeedsClarification;
    }
    DeterministicUpdate::Resolved(ResolvedTaskUpdate { task_id, patch })
}

/// A brain's parsed, allowlist-checked update proposal. Built only by
/// [`parse_update_slots`]; every string is sanitized and clamped, the priority is
/// coerced, and the status is kept only when settable. `task_id` / `assignee` are raw
/// references that [`reconcile_update_slots`] validates against the live state.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainUpdateSlots {
    pub task_id: Option<String>,
    pub title: Option<String>,
    pub details: Option<String>,
    pub priority: Option<u8>,
    pub status: Option<TaskStatus>,
    /// A raw assignee reference (id or fuzzy phrase) the kernel resolves to an
    /// existing agent.
    pub assignee: Option<String>,
    pub confidence: f32,
    pub rationale: String,
}

/// Build the JSON-only update-extraction prompt, grounded in the live board + the
/// supported fields + the status allowlist, so the brain maps a natural reference
/// ("bump the readme task to high") onto a real id + a real field. The kernel still
/// validates everything, so the listing is grounding, not authority.
pub fn build_update_slots_prompt(message: &str, summary: &StateSummary) -> String {
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
    let agent_list = if summary.all_agent_ids.is_empty() {
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

    format!(
        "You update an EXISTING task on a local Relux control plane. From the user's \
message, identify which task and what to change, choosing the task ONLY from the list \
below. You perform no action; you only describe the change so the kernel can apply it.\n\n\
Respond with JSON ONLY (no prose, no code fences) in this shape:\n\
{{\"task_id\":\"<task id from the list>\",\"title\":\"<optional new title, or omit>\",\
\"details\":\"<optional new details, or omit>\",\"priority\":<optional integer 1-9, or omit>,\
\"status\":\"<optional: blocked | cancelled, or omit>\",\"assignee\":\"<optional existing \
agent id, or omit>\",\"confidence\":0.0-1.0}}\n\n\
Rules:\n\
- task_id: REQUIRED; use an exact id from the Tasks list. If you cannot identify the task, \
set confidence low.\n\
- Include ONLY the fields the user actually asked to change; omit the rest.\n\
- status: ONLY \"blocked\" or \"cancelled\". NEVER \"running\"/\"completed\"/\"failed\" — those \
happen through the run lifecycle, not a chat edit.\n\
- assignee: ONLY an agent id from the Agents list; NEVER invent one.\n\
- priority: integer 1 (low) to 9 (high).\n\
- Do NOT add any field other than these. Do NOT claim the task was changed.\n\n\
Tasks:\n{tasks}\n\nAgents:\n{agents}\n\nMessage: {msg}",
        tasks = task_lines.join("\n"),
        agents = agent_list,
        msg = message,
    )
}

/// Parse a brain reply into validated [`BrainUpdateSlots`], or `Err` on anything
/// malformed / unsupported. The schema/allowlist gate: lift the JSON, reject any field
/// outside [`ALLOWED_KEYS`] (fail closed), sanitize and clamp every string, coerce the
/// priority, and keep the status only when settable.
pub fn parse_update_slots(raw: &str) -> Result<BrainUpdateSlots, String> {
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

    let task_id = obj
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(sanitize_task_id)
        .filter(|s| !s.is_empty());
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_line(s, MAX_TITLE_CHARS))
        .filter(|s| !s.is_empty());
    let details = obj
        .get("details")
        .and_then(|v| v.as_str())
        .map(|s| sanitize_line(s, MAX_DETAILS_CHARS))
        .filter(|s| !s.is_empty());
    let priority = coerce_priority(obj.get("priority"));
    // A non-settable / unknown status value is DROPPED (coerce-or-drop), not fatal.
    let status = obj
        .get("status")
        .and_then(|v| v.as_str())
        .and_then(parse_settable_status);
    let assignee = obj
        .get("assignee")
        .and_then(|v| v.as_str())
        .map(|s| clamp_line(s, MAX_ASSIGNEE_CHARS))
        .filter(|s| !s.is_empty());
    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;
    let rationale = obj
        .get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| clamp_line(s, MAX_RATIONALE_CHARS))
        .unwrap_or_default();

    Ok(BrainUpdateSlots {
        task_id,
        title,
        details,
        priority,
        status,
        assignee,
        confidence,
        rationale,
    })
}

/// Reconcile a brain update proposal against the live state, returning the validated
/// update to apply, or `None` to keep the deterministic outcome (a clarify).
///
/// Policy — every rule fails toward the deterministic / safer choice:
/// 1. Low confidence (`< CONFIDENCE_FLOOR`) → `None`.
/// 2. `task_id` is the brain's when present, else the deterministic reference, and is
///    honored ONLY when it names an EXISTING task (`summary.all_task_ids`).
/// 3. Each field is taken as already validated by [`parse_update_slots`]; an `assignee`
///    is additionally resolved through [`resolve_assignee`] and kept only when it names
///    an existing agent.
/// 4. The patch must carry at least one change; otherwise `None`.
pub fn reconcile_update_slots(
    deterministic_task_id: Option<&str>,
    proposal: &BrainUpdateSlots,
    summary: &StateSummary,
) -> Option<ResolvedTaskUpdate> {
    if proposal.confidence < CONFIDENCE_FLOOR {
        return None;
    }
    let task_ref = proposal
        .task_id
        .as_deref()
        .or(deterministic_task_id)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let task_id = summary
        .all_task_ids
        .iter()
        .find(|id| id.eq_ignore_ascii_case(task_ref))
        .cloned()?;

    let assignee = proposal.assignee.as_ref().and_then(|phrase| {
        match resolve_assignee(phrase, &summary.all_agent_ids) {
            AssigneeResolution::Resolved(id) => Some(id),
            _ => None,
        }
    });

    let patch = TaskUpdatePatch {
        title: proposal.title.clone(),
        details: proposal.details.clone(),
        priority: proposal.priority,
        status: proposal.status.clone(),
        assignee,
    };
    if patch.is_empty() {
        return None;
    }
    Some(ResolvedTaskUpdate { task_id, patch })
}

/// True for a terminal task status — a finished task is never edited by a by-id update.
pub fn is_terminal_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Expired
    )
}

/// True when a status is one Prime may set from a conversational by-id update.
pub fn is_settable_status(status: &TaskStatus) -> bool {
    SETTABLE_STATUSES.iter().any(|(_, s)| s == status)
}

/// The snake_case label for a status (matches the wire `rename_all`), for display.
pub fn status_label(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Created => "created",
        TaskStatus::Queued => "queued",
        TaskStatus::Leased => "leased",
        TaskStatus::Running => "running",
        TaskStatus::WaitingForTool => "waiting_for_tool",
        TaskStatus::WaitingForApproval => "waiting_for_approval",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Expired => "expired",
    }
}

// --- text helpers --------------------------------------------------------------

/// The value after the first case-insensitive `" to "`, with any `task_…` token
/// removed, trimmed. Used for "rename … to X" / "set … details to X". `None` when no
/// `" to "` separator is present.
fn value_after_to(message: &str) -> Option<String> {
    let lower = message.to_lowercase();
    let idx = lower.find(" to ")?;
    let after = &message[idx + " to ".len()..];
    let kept = after
        .split_whitespace()
        .filter(|w| !w.to_lowercase().starts_with("task_"))
        .collect::<Vec<_>>()
        .join(" ");
    let kept = kept.trim().to_string();
    if kept.is_empty() {
        None
    } else {
        Some(kept)
    }
}

/// Extract a priority integer from a message that names "priority", clamped to `[1,9]`.
/// Scans the tokens AFTER "priority" and takes the first standalone integer, SKIPPING a
/// `task_…` id token (so "change task priority task_0001 to 8" reads 8, not the `0001`
/// inside the id). `None` when no usable number follows.
fn extract_priority(message: &str) -> Option<u8> {
    let lower = message.to_lowercase();
    let start = lower.find("priority").map(|i| i + "priority".len()).unwrap_or(0);
    let tail = &message[start..];
    for tok in tail.split_whitespace() {
        let low = tok.to_lowercase();
        if low.starts_with("task_") || low.starts_with("task") {
            continue;
        }
        // Keep only the digits in the token (strips trailing punctuation like "8.").
        let digits: String = tok.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        if let Ok(n) = digits.parse::<u32>() {
            return Some(n.clamp(PRIORITY_MIN as u32, PRIORITY_MAX as u32) as u8);
        }
    }
    None
}

/// Whole-word containment (so "fail" doesn't match "failsafe"). ASCII word boundaries.
fn has_word(haystack: &str, word: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t == word)
}

/// Coerce a JSON priority (number or numeric string) to a clamped `u8`, or `None`.
fn coerce_priority(value: Option<&serde_json::Value>) -> Option<u8> {
    let raw = match value? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    if !raw.is_finite() {
        return None;
    }
    let clamped = raw.round().clamp(PRIORITY_MIN as f64, PRIORITY_MAX as f64);
    Some(clamped as u8)
}

/// Sanitize a single-line string: control chars → space, collapse whitespace, trim,
/// clamp to `max`.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Like [`sanitize_line`] but for an opaque reference (assignee phrase) — same
/// cleaning, just a different name at call sites.
fn clamp_line(s: &str, max: usize) -> String {
    sanitize_line(s, max)
}

/// Sanitize a proposed task id: lowercase, keep only `[a-z0-9_-]`, clamp.
fn sanitize_task_id(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(MAX_ASSIGNEE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::{TaskBrief, TaskId};

    fn summary(tasks: &[&str], agents: &[&str]) -> StateSummary {
        StateSummary {
            plugins: 0,
            agents: agents.len(),
            tasks_total: tasks.len(),
            tasks_open: 0,
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: agents.iter().map(|s| s.to_string()).collect(),
            all_task_ids: tasks.iter().map(|s| s.to_string()).collect(),
            queued: tasks
                .iter()
                .map(|id| TaskBrief {
                    id: TaskId::new(*id),
                    title: format!("title for {id}"),
                    status: TaskStatus::Queued,
                    assigned_agent: None,
                })
                .collect(),
            recent: vec![],
        }
    }

    // --- deterministic rail --------------------------------------------------

    #[test]
    fn deterministic_renames_a_task() {
        let s = summary(&["task_0001"], &[]);
        match deterministic_update("rename task_0001 to Fix login blank page", &s) {
            DeterministicUpdate::Resolved(r) => {
                assert_eq!(r.task_id, "task_0001");
                assert_eq!(r.patch.title.as_deref(), Some("Fix login blank page"));
                assert!(r.patch.priority.is_none());
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn deterministic_sets_priority() {
        let s = summary(&["task_0001"], &[]);
        match deterministic_update("set task_0001 priority to 8", &s) {
            DeterministicUpdate::Resolved(r) => assert_eq!(r.patch.priority, Some(8)),
            other => panic!("expected Resolved, got {other:?}"),
        }
        // Out of range clamps.
        match deterministic_update("set task_0001 priority to 99", &s) {
            DeterministicUpdate::Resolved(r) => assert_eq!(r.patch.priority, Some(9)),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn deterministic_cancels_and_blocks_but_refuses_completion() {
        let s = summary(&["task_0001"], &[]);
        assert!(matches!(
            deterministic_update("cancel task_0001", &s),
            DeterministicUpdate::Resolved(ref r) if r.patch.status == Some(TaskStatus::Cancelled)
        ));
        assert!(matches!(
            deterministic_update("mark task_0001 as blocked", &s),
            DeterministicUpdate::Resolved(ref r) if r.patch.status == Some(TaskStatus::Blocked)
        ));
        // "mark done" is honestly refused, never faked into a completion.
        assert!(matches!(
            deterministic_update("mark task_0001 as done", &s),
            DeterministicUpdate::RejectedStatus("completed")
        ));
    }

    #[test]
    fn deterministic_reassigns_against_the_roster() {
        let s = summary(&["task_0001"], &["researcher"]);
        match deterministic_update("reassign task_0001 to the researcher", &s) {
            DeterministicUpdate::Resolved(r) => {
                assert_eq!(r.patch.assignee.as_deref(), Some("researcher"))
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
        // Unknown agent fails closed.
        assert!(matches!(
            deterministic_update("reassign task_0001 to nobody", &s),
            DeterministicUpdate::UnknownAssignee(_)
        ));
    }

    #[test]
    fn deterministic_reports_ambiguous_assignee() {
        let s = summary(&["task_0001"], &["research-agent", "research-bot"]);
        match deterministic_update("reassign task_0001 to research", &s) {
            DeterministicUpdate::AmbiguousAssignee { matches, .. } => {
                assert_eq!(matches.len(), 2)
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn deterministic_fails_closed_on_unknown_task() {
        let s = summary(&["task_0001"], &[]);
        assert!(matches!(
            deterministic_update("set task_9999 priority to 8", &s),
            DeterministicUpdate::UnknownTask(_)
        ));
    }

    #[test]
    fn deterministic_clarifies_when_underspecified() {
        let s = summary(&["task_0001"], &[]);
        // No task id.
        assert!(matches!(
            deterministic_update("update the priority", &s),
            DeterministicUpdate::NeedsClarification
        ));
        // Task id but no field.
        assert!(matches!(
            deterministic_update("update task_0001", &s),
            DeterministicUpdate::NeedsClarification
        ));
    }

    // --- brain parse / reconcile --------------------------------------------

    #[test]
    fn parses_a_clean_update_proposal() {
        let p = parse_update_slots(
            r#"{"task_id":"task_0001","title":"Fix the redirect","priority":7,"status":"blocked","confidence":0.9}"#,
        )
        .unwrap();
        assert_eq!(p.task_id.as_deref(), Some("task_0001"));
        assert_eq!(p.title.as_deref(), Some("Fix the redirect"));
        assert_eq!(p.priority, Some(7));
        assert_eq!(p.status, Some(TaskStatus::Blocked));
    }

    #[test]
    fn lifts_json_from_a_noisy_reply() {
        let p = parse_update_slots("ok:\n```\n{\"task_id\":\"task_0002\",\"priority\":3,\"confidence\":0.8}\n```")
            .unwrap();
        assert_eq!(p.task_id.as_deref(), Some("task_0002"));
        assert_eq!(p.priority, Some(3));
    }

    #[test]
    fn rejects_unsupported_field_fail_closed() {
        assert!(parse_update_slots(
            r#"{"task_id":"task_0001","run_now":true,"confidence":0.9}"#
        )
        .is_err());
    }

    #[test]
    fn drops_a_non_settable_status_value() {
        // The brain proposes "completed" — dropped (coerce-or-drop), not fatal.
        let p = parse_update_slots(
            r#"{"task_id":"task_0001","status":"completed","priority":4,"confidence":0.9}"#,
        )
        .unwrap();
        assert!(p.status.is_none());
        assert_eq!(p.priority, Some(4));
    }

    #[test]
    fn reconcile_validates_task_and_assignee() {
        let s = summary(&["task_0001"], &["researcher"]);
        let p = BrainUpdateSlots {
            task_id: Some("task_0001".to_string()),
            title: None,
            details: None,
            priority: None,
            status: None,
            assignee: Some("the researcher".to_string()),
            confidence: 0.9,
            rationale: String::new(),
        };
        let r = reconcile_update_slots(None, &p, &s).unwrap();
        assert_eq!(r.task_id, "task_0001");
        assert_eq!(r.patch.assignee.as_deref(), Some("researcher"));
    }

    #[test]
    fn reconcile_fails_closed_on_unknown_task_low_confidence_or_empty() {
        let s = summary(&["task_0001"], &["research-agent"]);
        let base = BrainUpdateSlots {
            task_id: Some("task_0001".to_string()),
            title: None,
            details: None,
            priority: Some(8),
            status: None,
            assignee: None,
            confidence: 0.9,
            rationale: String::new(),
        };
        // Unknown task.
        let mut unknown = base.clone();
        unknown.task_id = Some("task_9999".to_string());
        assert!(reconcile_update_slots(None, &unknown, &s).is_none());
        // Low confidence.
        let mut low = base.clone();
        low.confidence = 0.2;
        assert!(reconcile_update_slots(None, &low, &s).is_none());
        // Empty patch (no changes).
        let mut empty = base.clone();
        empty.priority = None;
        assert!(reconcile_update_slots(None, &empty, &s).is_none());
        // The clean base resolves.
        assert!(reconcile_update_slots(None, &base, &s).is_some());
    }

    #[test]
    fn reconcile_falls_back_to_the_deterministic_task_id() {
        let s = summary(&["task_0001"], &[]);
        let p = BrainUpdateSlots {
            task_id: None,
            title: Some("New title".to_string()),
            details: None,
            priority: None,
            status: None,
            assignee: None,
            confidence: 0.9,
            rationale: String::new(),
        };
        let r = reconcile_update_slots(Some("task_0001"), &p, &s).unwrap();
        assert_eq!(r.task_id, "task_0001");
        assert_eq!(r.patch.title.as_deref(), Some("New title"));
    }

    #[test]
    fn patch_round_trips_through_the_action_string() {
        let patch = TaskUpdatePatch {
            title: Some("T".to_string()),
            priority: Some(6),
            status: Some(TaskStatus::Blocked),
            ..Default::default()
        };
        let s = patch.to_patch_string();
        assert_eq!(TaskUpdatePatch::from_patch_str(&s), Some(patch.clone()));
        let rows = patch.change_rows();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|r| r.field == "status" && r.value == "blocked"));
    }

    #[test]
    fn settable_status_allowlist_rejects_machine_states() {
        assert_eq!(parse_settable_status("blocked"), Some(TaskStatus::Blocked));
        assert_eq!(parse_settable_status("cancelled"), Some(TaskStatus::Cancelled));
        assert!(parse_settable_status("running").is_none());
        assert!(parse_settable_status("completed").is_none());
    }

    #[test]
    fn prompt_carries_board_fields_and_status_allowlist() {
        let s = summary(&["task_0001"], &["research-agent"]);
        let prompt = build_update_slots_prompt("bump the readme task to high", &s);
        assert!(prompt.contains("task_0001"));
        assert!(prompt.contains("research-agent"));
        assert!(prompt.contains("blocked | cancelled"));
        assert!(prompt.contains("JSON ONLY"));
    }
}
