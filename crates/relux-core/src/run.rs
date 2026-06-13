use serde::{Deserialize, Serialize};

use crate::agent::AgentId;
use crate::artifact::RunArtifact;
use crate::plugin::PluginId;
use crate::proposed_change::ProposedChange;
use crate::run_failure::{RunFailureClass, RunRetryState};
use crate::run_session::RunSession;
use crate::task::TaskId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

impl RunId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle states for one execution attempt of a task.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

/// One execution attempt for a task.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run).
///
/// The timing fields work in two layers. `started_at`/`ended_at` come from the
/// kernel's deterministic logical clock (ordering, reproducible), so they are NOT
/// wall-clock instants. `duration_ms` is the **real** measured wall time of an
/// adapter subprocess (captured in the adapter spawn, which is the one place a
/// real process is touched); it is only present for CLI adapter runs. `usage` and
/// `cost` are only populated when an adapter emits a structured result envelope we
/// could parse (master plan section 9.6) - never fabricated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task_id: TaskId,
    pub agent_id: AgentId,
    pub adapter_plugin: PluginId,
    pub status: RunStatus,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    /// Real measured wall-clock duration of the adapter subprocess, in
    /// milliseconds. Only set for CLI adapter runs; `None` for the deterministic
    /// local echo path (which never touches a real process).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Structured token/usage data, only when the adapter reported it in a
    /// machine-readable result envelope. Never synthesized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
    /// Reported cost in USD, only when the adapter result envelope carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// When this run was created by retrying an earlier run, the id of that run
    /// (attempt lineage). Retry is a fresh run on the same task, not a resume of a
    /// partial CLI run (master plan section 10.2 `prime.retry_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retried_from: Option<RunId>,
    /// When this run was created by **resuming** an earlier run's provider session
    /// (`run.resume`), the id of that run. Distinct from `retried_from`: a resume
    /// continues the prior adapter session (threading its `session.adapter_session_id`
    /// through the governed `--resume` gate), whereas a retry is a cold fresh run on
    /// the same task. `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3. The kernel also reads
    /// this as the "this is a resume" marker when composing the adapter argv. Never
    /// both `resumed_from` and `retried_from` are set on one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<RunId>,
    /// The durable, bounded, redacted **session identity / handoff** captured for
    /// this run from its adapter's structured result envelope (the Claude CLI
    /// `session_id`). Records who the provider session was, its source, and whether
    /// Relux can safely resume it (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3). `None`
    /// when the adapter emitted no session id (e.g. the local echo path, Codex
    /// plain-text, or a generic command). Never a token, raw envelope, or full log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<RunSession>,
    /// Read-only **artifact references** the adapter declared in its structured
    /// result envelope (master plan section 9.6 / section 15). Each is a bounded,
    /// redacted, path-sanitized reference (name/type/summary/source) — NOT a
    /// workspace diff or an apply plan. Empty when the adapter declared none (or
    /// emitted no structured envelope). Capturing these never enables apply: the
    /// Relux run model still has no diff/apply, so the dashboard lists them
    /// read-only and keeps apply unavailable. Never fabricated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<RunArtifact>,
    /// Reviewable, applyable **proposed file changes** the adapter declared in its
    /// structured result envelope (`proposed_changes: [...]`, master plan section
    /// 15 / section 9.6). Each is a bounded, path-sanitized, text-only
    /// **full-content replacement** of one file with the agent's baseline hash —
    /// the first real Relux diff/apply model. Unlike `artifacts` (read-only
    /// references), these carry content and can be reviewed (approve/reject) and,
    /// once approved, explicitly applied into the run's controlled workspace root
    /// with a baseline-conflict check. Empty when the adapter declared none. Never
    /// fabricated; apply never happens without an explicit operator action.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_changes: Vec<ProposedChange>,
    /// The structured classification of why this run failed (set only for failed
    /// runs). Drives the honest "retryable vs needs-operator-action" surface in run
    /// records, Prime replies, and the Doctor (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md`
    /// §7). `None` for runs that have not failed. Never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<RunFailureClass>,
    /// The bounded transient-retry state for a failed run whose class is
    /// auto-retryable ([`RunFailureClass::retryable`]): which attempt it is, the
    /// cap, and the earliest real instant a retry may run. `None` for a successful
    /// run, or a failure whose class is not auto-retryable (those wait for an
    /// operator). There is no background scheduler — the retry is consumed manually
    /// (`prime.retry_run`) or on the next autonomy tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RunRetryState>,
    /// Real wall-clock seconds (Unix epoch) of this run's most recent transcript
    /// activity — set when the run starts and bumped on every run event the kernel
    /// pushes. Distinct from `started_at`/`ended_at`, which are logical-clock
    /// strings for ordering, not wall time. This is the heartbeat the run watchdog
    /// reads to decide whether a `Running` run has stalled (no new activity for the
    /// configured window). Like `duration_ms` and `retry.next_attempt_at`, it is
    /// honest wall-clock, not a deterministic value. `None` only for older
    /// snapshots written before the field existed (they default to never-active,
    /// so the watchdog can recover a pre-existing orphan after one window).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{ArtifactKind, RunArtifact};

    fn sample_run() -> Run {
        Run {
            id: RunId::new("run_0001"),
            task_id: TaskId::new("task_0001"),
            agent_id: AgentId::new("agent_0001"),
            adapter_plugin: PluginId::new("relux-adapter-claude-cli"),
            status: RunStatus::Completed,
            started_at: Some("t0".into()),
            ended_at: Some("t1".into()),
            summary: Some("done".into()),
            error: None,
            duration_ms: Some(10),
            usage: None,
            cost: None,
            retried_from: None,
            resumed_from: None,
            session: None,
            artifacts: Vec::new(),
            proposed_changes: Vec::new(),
            failure_class: None,
            retry: None,
            last_activity_at: None,
        }
    }

    #[test]
    fn empty_artifacts_are_omitted_from_the_wire() {
        let json = serde_json::to_value(sample_run()).unwrap();
        assert!(json.get("artifacts").is_none(), "empty artifacts must be omitted");
        assert!(
            json.get("proposed_changes").is_none(),
            "empty proposed_changes must be omitted"
        );
        // Absent session/resume metadata stays off the wire (no fabricated nulls).
        assert!(json.get("session").is_none(), "absent session must be omitted");
        assert!(json.get("resumed_from").is_none(), "absent resumed_from must be omitted");
    }

    #[test]
    fn session_and_resume_lineage_round_trip_on_the_wire() {
        use crate::run_session::RunSession;
        let mut run = sample_run();
        run.session = Some(RunSession {
            adapter_session_id: "sess-abc-123".into(),
            source: "claude-cli".into(),
            resume_supported: true,
        });
        run.resumed_from = Some(RunId::new("run_0000"));
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(
            json.pointer("/session/adapter_session_id").and_then(|v| v.as_str()),
            Some("sess-abc-123")
        );
        assert_eq!(
            json.pointer("/session/resume_supported").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(json.get("resumed_from").and_then(|v| v.as_str()), Some("run_0000"));
        let back: Run = serde_json::from_value(json).unwrap();
        assert_eq!(back.session, run.session);
        assert_eq!(back.resumed_from, run.resumed_from);
    }

    #[test]
    fn proposed_changes_round_trip_with_status_for_the_api() {
        use crate::proposed_change::{ProposedChange, ProposedChangeStatus};
        let mut run = sample_run();
        run.proposed_changes = vec![ProposedChange {
            path: "src/main.rs".into(),
            action: crate::proposed_change::ProposedChangeAction::Replace,
            dest_path: None,
            new_content: "fn main() {}\n".into(),
            baseline_sha256: Some(crate::proposed_change::sha256_hex(b"old")),
            new_sha256: crate::proposed_change::sha256_hex(b"fn main() {}\n"),
            bytes: 13,
            source: "claude-cli".into(),
            status: ProposedChangeStatus::Approved,
            review_note: Some("looks good".into()),
            refused_reason: None,
            applied_at: None,
        }];
        let json = serde_json::to_value(&run).unwrap();
        let cs = json.get("proposed_changes").and_then(|v| v.as_array()).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].get("status").and_then(|v| v.as_str()), Some("approved"));
        assert_eq!(cs[0].get("path").and_then(|v| v.as_str()), Some("src/main.rs"));
        let back: Run = serde_json::from_value(json).unwrap();
        assert_eq!(back.proposed_changes, run.proposed_changes);
    }

    #[test]
    fn artifacts_round_trip_with_type_field_for_the_api() {
        let mut run = sample_run();
        run.artifacts = vec![RunArtifact {
            name: "main.rs".into(),
            kind: ArtifactKind::File,
            summary: Some("edited".into()),
            source: "claude-cli".into(),
            path: Some("src/main.rs".into()),
            bytes: Some(42),
            truncated: false,
        }];
        let json = serde_json::to_value(&run).unwrap();
        // The API flattens `Run`, so the wire carries `artifacts[].type`.
        let arts = json.get("artifacts").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].get("type").and_then(|v| v.as_str()), Some("file"));
        assert_eq!(arts[0].get("name").and_then(|v| v.as_str()), Some("main.rs"));
        // Round-trips back to the same value.
        let back: Run = serde_json::from_value(json).unwrap();
        assert_eq!(back.artifacts, run.artifacts);
    }
}
