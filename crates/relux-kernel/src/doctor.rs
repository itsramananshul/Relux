//! Read-only operator **Doctor** report (`docs/relix-dashboard-design.md` §15).
//!
//! The Home/Health readiness guide already turns the live `/v1/relux` reads into
//! an honest pass/warn/fail checklist *in the frontend*. The Doctor is the
//! kernel-side counterpart: a single, cheap, **read-only** diagnostic the kernel
//! computes from the SAME inexpensive reads the `/v1/relux/health` endpoint
//! already makes (store open/load, dashboard bundle, AI status, adapter + tool
//! readiness, agent + approval counts) and returns as structured rows with a
//! severity, a human message, and — where there is a concrete fix — a remediation
//! line and a dashboard action link.
//!
//! It performs NO heavy work: no cargo build/test, no network beyond what health
//! already does (none), no mutation. It is bounded by the existing in-memory
//! kernel snapshot.
//!
//! ## Reference grounding (`docs/reference-driven-development.md`)
//!
//! - Hermes `hermes_cli/doctor.py` — `check_ok`/`check_warn`/`check_fail`/
//!   `check_info` emit severity rows with a detail string, and `_fail_and_issue`
//!   pairs a failure with a concrete `fix` remediation. We mirror that shape:
//!   every [`DoctorCheck`] carries a [`DoctorSeverity`], a `message`, and an
//!   optional `remediation`/`action_link` (our equivalent of the `fix`).
//! - openclaw `src/gateway/server/health-state.ts` — `buildGatewaySnapshot`
//!   surfaces resolved filesystem paths (`configPath`, `stateDir`) ONLY to admin
//!   callers via `includeSensitive`; the default snapshot omits them. We adopt the
//!   stricter default unconditionally: the Doctor takes NO path inputs at all
//!   (`DoctorInputs` carries booleans/counts/states, never a db path or a resolved
//!   binary path), so a path can never leak into a check message.
//!
//! The derivation deliberately matches the frontend `readiness.ts` semantics so
//! the two surfaces never disagree on what "ready" means: a SELECTED-but-broken
//! brain is the failure; a local deterministic brain WORKS (info, not failure); an
//! installed-but-unconfigured adapter/tool is attention, not a hard fail.

use serde::Serialize;

use crate::ai::AiStatus;
use relux_core::{
    AdapterRuntimeState, AdapterRuntimeStatus, ToolDescriptor, ToolExecutability,
    CLAUDE_CLI_ADAPTER_ID, CODEX_CLI_ADAPTER_ID,
};

/// One check's severity, lowest → highest. Mirrors Hermes' four `check_*` levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorSeverity {
    /// Healthy / nothing to do.
    Ok,
    /// Neutral context or an optional, non-blocking recommendation.
    Info,
    /// Installed/selected but needs setup before it does anything; not fatal.
    Warn,
    /// A selected capability is broken and blocks its normal use.
    Fail,
}

/// One row in the Doctor report. Carries no secrets and no filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    /// Stable machine id, e.g. `prime.brain` (safe for the UI to key on).
    pub id: String,
    /// Short human label.
    pub label: String,
    pub severity: DoctorSeverity,
    /// Secret-free explanation of the current state.
    pub message: String,
    /// What to do about it, when there is a concrete fix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    /// An in-app route that fixes/inspects this (e.g. `/health`, `/crew`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_link: Option<String>,
}

/// At-a-glance tallies across every check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DoctorSummary {
    pub ok: usize,
    pub info: usize,
    pub warn: usize,
    pub fail: usize,
}

/// The full Doctor report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    /// Unix seconds the report was produced (stamped by the caller).
    pub generated_at: i64,
    /// The worst severity across all checks (`fail` > `warn` > `ok`/`info`).
    pub overall: DoctorSeverity,
    pub summary: DoctorSummary,
    pub checks: Vec<DoctorCheck>,
}

/// The cheap, already-available inputs the report is derived from. Deliberately
/// holds NO filesystem paths or secrets (structural redaction): the caller
/// distills these from the loaded kernel without passing any path through.
pub struct DoctorInputs<'a> {
    /// Whether the kernel state store opened and loaded.
    pub db_ok: bool,
    /// Whether a dashboard bundle is being served.
    pub dashboard_bundle_present: bool,
    /// The key-free AI status (model name is safe; the key never appears here).
    pub ai: &'a AiStatus,
    /// Installed adapter runtimes (state/availability only is read).
    pub adapters: &'a [AdapterRuntimeStatus],
    /// Discovered tools with their executable status.
    pub tools: &'a [ToolDescriptor],
    /// Number of configured agents (including Prime).
    pub agent_count: usize,
    /// Number of approvals awaiting an operator decision.
    pub pending_approvals: usize,
    /// Number of tasks whose newest run failed with a class that needs an operator
    /// to act (auth / adapter / permission / invalid request / output validation /
    /// unknown). `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §7.
    pub runs_needing_action: usize,
    /// Number of tasks whose newest run failed transiently and has a bounded retry
    /// scheduled (not yet exhausted) on the `[2m,10m,30m,2h]` backoff.
    pub runs_retry_pending: usize,
}

fn check(
    id: &str,
    label: &str,
    severity: DoctorSeverity,
    message: impl Into<String>,
    remediation: Option<&str>,
    action_link: Option<&str>,
) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        label: label.to_string(),
        severity,
        message: message.into(),
        remediation: remediation.map(|s| s.to_string()),
        action_link: action_link.map(|s| s.to_string()),
    }
}

fn adapter_for<'a>(
    adapters: &'a [AdapterRuntimeStatus],
    id: &str,
) -> Option<&'a AdapterRuntimeStatus> {
    adapters.iter().find(|a| a.plugin_id == id)
}

/// The `prime.brain` row — who answers Prime's chat. Mirrors `readiness.ts`
/// `primeBrainStep`: a selected-but-broken brain is the only failure; the local
/// deterministic brain works (info).
fn brain_check(ai: &AiStatus, adapters: &[AdapterRuntimeStatus]) -> DoctorCheck {
    const ID: &str = "prime.brain";
    const LABEL: &str = "Prime brain";

    if ai.disabled {
        return check(
            ID,
            LABEL,
            DoctorSeverity::Warn,
            "Prime's AI brain is disabled; replies use the built-in deterministic operator.",
            Some("Re-enable a brain in Health → AI settings."),
            Some("/health"),
        );
    }

    match ai.brain.as_str() {
        "local" => check(
            ID,
            LABEL,
            DoctorSeverity::Info,
            "Prime is using the built-in local deterministic brain. Optional: connect OpenRouter, the Claude CLI, or the Codex CLI for richer replies.",
            None,
            Some("/health"),
        ),
        "openrouter" => {
            if ai.configured {
                let msg = if ai.model.trim().is_empty() {
                    "OpenRouter brain is configured.".to_string()
                } else {
                    format!("OpenRouter brain is configured (model {}).", ai.model)
                };
                check(ID, LABEL, DoctorSeverity::Ok, msg, None, Some("/health"))
            } else {
                check(
                    ID,
                    LABEL,
                    DoctorSeverity::Fail,
                    "OpenRouter brain is selected but no API key is configured.",
                    Some("Add the OpenRouter key in Health → AI settings, or switch to the local brain."),
                    Some("/health"),
                )
            }
        }
        "claude_cli" => cli_brain_check(ID, LABEL, "Claude CLI", adapters, CLAUDE_CLI_ADAPTER_ID),
        "codex_cli" => cli_brain_check(ID, LABEL, "Codex CLI", adapters, CODEX_CLI_ADAPTER_ID),
        other => check(
            ID,
            LABEL,
            DoctorSeverity::Info,
            format!("Prime brain '{other}' is selected."),
            None,
            Some("/health"),
        ),
    }
}

fn cli_brain_check(
    id: &str,
    label: &str,
    name: &str,
    adapters: &[AdapterRuntimeStatus],
    adapter_id: &str,
) -> DoctorCheck {
    match adapter_for(adapters, adapter_id).map(|a| &a.state) {
        Some(AdapterRuntimeState::Available) | Some(AdapterRuntimeState::LocalDeterministic) => {
            check(
                id,
                label,
                DoctorSeverity::Ok,
                format!("{name} brain is ready (adapter enabled and on PATH)."),
                None,
                Some("/crew"),
            )
        }
        Some(AdapterRuntimeState::MissingBinary) => check(
            id,
            label,
            DoctorSeverity::Fail,
            format!("{name} brain is selected but its CLI is not on PATH."),
            Some("Install and sign in to the CLI so it is on PATH, then enable its adapter on Crew → Adapters, or switch to the local brain."),
            Some("/crew"),
        ),
        Some(AdapterRuntimeState::Disabled) => check(
            id,
            label,
            DoctorSeverity::Fail,
            format!("{name} brain is selected but its adapter runtime is disabled."),
            Some("Enable the adapter on Crew → Adapters, or switch to the local brain."),
            Some("/crew"),
        ),
        Some(AdapterRuntimeState::NeedsConfiguration) | None => check(
            id,
            label,
            DoctorSeverity::Fail,
            format!("{name} brain is selected but its adapter is not enabled."),
            Some("Enable the adapter on Crew → Adapters, or switch to the local brain."),
            Some("/crew"),
        ),
    }
}

/// The `adapters.real_work` row — whether a Claude/Codex CLI adapter can EXECUTE
/// assigned tasks (distinct from the brain). Optional, so an unavailable adapter
/// is info, never a failure: Prime tracks work without one.
fn real_work_check(adapters: &[AdapterRuntimeStatus]) -> DoctorCheck {
    const ID: &str = "adapters.real_work";
    const LABEL: &str = "Real-work adapter";

    let claude = adapter_for(adapters, CLAUDE_CLI_ADAPTER_ID);
    let codex = adapter_for(adapters, CODEX_CLI_ADAPTER_ID);
    let cli: Vec<&AdapterRuntimeStatus> = [claude, codex].into_iter().flatten().collect();

    let name_of = |a: &AdapterRuntimeStatus| {
        if a.plugin_id == CLAUDE_CLI_ADAPTER_ID {
            "Claude CLI"
        } else {
            "Codex CLI"
        }
    };

    if let Some(a) = cli.iter().find(|a| a.state == AdapterRuntimeState::Available) {
        return check(
            ID,
            LABEL,
            DoctorSeverity::Ok,
            format!(
                "{} adapter is enabled and on PATH — Prime can execute assigned tasks through it.",
                name_of(a)
            ),
            None,
            Some("/crew"),
        );
    }

    if let Some(a) = cli.iter().find(|a| a.available_on_path) {
        return check(
            ID,
            LABEL,
            DoctorSeverity::Info,
            format!(
                "{} is detected on PATH but its adapter is not enabled (optional).",
                name_of(a)
            ),
            Some("Enable it on Crew → Adapters to execute real work (it runs in a safe, non-bypass mode)."),
            Some("/crew"),
        );
    }

    check(
        ID,
        LABEL,
        DoctorSeverity::Info,
        "No real-work CLI adapter enabled (optional). Prime creates and tracks work without one.",
        Some("Install and sign in to the Claude CLI or Codex CLI, then enable its adapter on Crew → Adapters to execute tasks."),
        Some("/crew"),
    )
}

/// The `plugins.tools` row — the honest capability surface from tool readiness.
fn tools_check(tools: &[ToolDescriptor]) -> DoctorCheck {
    const ID: &str = "plugins.tools";
    const LABEL: &str = "Plugins & tools";

    let ready = tools
        .iter()
        .filter(|t| t.executable == ToolExecutability::Ready)
        .count();
    let needs_runtime = tools
        .iter()
        .filter(|t| {
            matches!(
                t.executable,
                ToolExecutability::RuntimeNotConfigured | ToolExecutability::RuntimeDisabled
            )
        })
        .count();
    let needs_approval = tools
        .iter()
        .filter(|t| t.executable == ToolExecutability::NeedsApproval)
        .count();

    if needs_runtime > 0 {
        let plural = if needs_runtime == 1 { "" } else { "s" };
        let it = if needs_runtime == 1 { "it" } else { "they" };
        return check(
            ID,
            LABEL,
            DoctorSeverity::Warn,
            format!("{needs_runtime} tool{plural} need a loopback runtime before {it} can run."),
            Some("Point Relux at the local HTTP server you run for the plugin, on Plugins."),
            Some("/plugins"),
        );
    }

    if ready > 0 {
        let plural = if ready == 1 { "" } else { "s" };
        let approval_note = if needs_approval > 0 {
            let is = if needs_approval == 1 { "is" } else { "are" };
            format!(" {needs_approval} more {is} gated behind per-call approval (by design).")
        } else {
            String::new()
        };
        return check(
            ID,
            LABEL,
            DoctorSeverity::Ok,
            format!("{ready} tool{plural} ready to invoke.{approval_note}"),
            None,
            Some("/plugins"),
        );
    }

    check(
        ID,
        LABEL,
        DoctorSeverity::Info,
        "No extra tools configured (optional). Prime's built-in capabilities are available.",
        Some("Install plugins on Plugins to add tools and adapters."),
        Some("/plugins"),
    )
}

fn crew_check(agent_count: usize) -> DoctorCheck {
    const ID: &str = "crew";
    const LABEL: &str = "Crew";
    if agent_count > 0 {
        let plural = if agent_count == 1 { "" } else { "s" };
        check(
            ID,
            LABEL,
            DoctorSeverity::Ok,
            format!("{agent_count} agent{plural} configured (including Prime)."),
            None,
            Some("/crew"),
        )
    } else {
        check(
            ID,
            LABEL,
            DoctorSeverity::Info,
            "No agents configured; Prime is the built-in operative and can do the work itself.",
            None,
            Some("/crew"),
        )
    }
}

fn approvals_check(pending: usize) -> DoctorCheck {
    const ID: &str = "approvals.pending";
    const LABEL: &str = "Pending approvals";
    if pending > 0 {
        let plural = if pending == 1 { "" } else { "s" };
        let is = if pending == 1 { "is" } else { "are" };
        check(
            ID,
            LABEL,
            DoctorSeverity::Warn,
            format!("{pending} approval{plural} {is} waiting on your decision."),
            Some("Review and approve or reject them on Approvals."),
            Some("/approvals"),
        )
    } else {
        check(
            ID,
            LABEL,
            DoctorSeverity::Ok,
            "No approvals pending.",
            None,
            Some("/approvals"),
        )
    }
}

/// The `runs.recovery` row — failed runs that need attention vs. transient
/// failures already scheduled to retry. Mirrors Paperclip's run-liveness surface
/// (`run-liveness.ts`): a failure that needs an operator (auth/adapter/permission/
/// invalid/validation/unknown) is the attention signal; a transient failure with a
/// bounded retry pending is informational, not a problem. A clean board is `Ok`.
fn runs_recovery_check(needs_action: usize, retry_pending: usize) -> DoctorCheck {
    const ID: &str = "runs.recovery";
    const LABEL: &str = "Run recovery";

    if needs_action > 0 {
        let plural = if needs_action == 1 { "" } else { "s" };
        let needs = if needs_action == 1 { "needs" } else { "need" };
        let retry_note = if retry_pending > 0 {
            let rp = if retry_pending == 1 { "" } else { "s" };
            format!(" {retry_pending} transient failure{rp} will retry automatically.")
        } else {
            String::new()
        };
        return check(
            ID,
            LABEL,
            DoctorSeverity::Warn,
            format!("{needs_action} failed run{plural} {needs} operator action (auth, adapter, permission, request, or review).{retry_note}"),
            Some("Review the failed runs on Work, fix the cause, and retry."),
            Some("/work"),
        );
    }

    if retry_pending > 0 {
        let plural = if retry_pending == 1 { "" } else { "s" };
        let is = if retry_pending == 1 { "is" } else { "are" };
        return check(
            ID,
            LABEL,
            DoctorSeverity::Info,
            format!("{retry_pending} transient run failure{plural} {is} scheduled to retry on a bounded backoff."),
            None,
            Some("/work"),
        );
    }

    check(
        ID,
        LABEL,
        DoctorSeverity::Ok,
        "No failed runs need attention.",
        None,
        Some("/work"),
    )
}

fn store_check(db_ok: bool) -> DoctorCheck {
    const ID: &str = "kernel.store";
    const LABEL: &str = "Kernel state store";
    if db_ok {
        check(
            ID,
            LABEL,
            DoctorSeverity::Ok,
            "Kernel state store opened and loaded successfully.",
            None,
            None,
        )
    } else {
        check(
            ID,
            LABEL,
            DoctorSeverity::Fail,
            "Could not open or load the kernel state store.",
            Some("Check the data directory exists and is writable, then restart `relux-kernel serve`."),
            Some("/health"),
        )
    }
}

fn bundle_check(present: bool) -> DoctorCheck {
    const ID: &str = "dashboard.bundle";
    const LABEL: &str = "Dashboard bundle";
    if present {
        check(
            ID,
            LABEL,
            DoctorSeverity::Ok,
            "Dashboard bundle is present and served at /dashboard.",
            None,
            None,
        )
    } else {
        check(
            ID,
            LABEL,
            DoctorSeverity::Warn,
            "Dashboard bundle not found; the /v1/relux API works but /dashboard shows a build notice.",
            Some("Run `npm run build` in apps/dashboard (or set RELUX_DASHBOARD_DIST)."),
            None,
        )
    }
}

/// Build the full Doctor report from the cheap inputs. Pure (no I/O, no clock):
/// the caller stamps `generated_at`.
pub fn build_doctor_report(inputs: &DoctorInputs, generated_at: i64) -> DoctorReport {
    let checks = vec![
        store_check(inputs.db_ok),
        bundle_check(inputs.dashboard_bundle_present),
        brain_check(inputs.ai, inputs.adapters),
        real_work_check(inputs.adapters),
        tools_check(inputs.tools),
        crew_check(inputs.agent_count),
        approvals_check(inputs.pending_approvals),
        runs_recovery_check(inputs.runs_needing_action, inputs.runs_retry_pending),
    ];

    let mut summary = DoctorSummary {
        ok: 0,
        info: 0,
        warn: 0,
        fail: 0,
    };
    for c in &checks {
        match c.severity {
            DoctorSeverity::Ok => summary.ok += 1,
            DoctorSeverity::Info => summary.info += 1,
            DoctorSeverity::Warn => summary.warn += 1,
            DoctorSeverity::Fail => summary.fail += 1,
        }
    }

    let overall = if summary.fail > 0 {
        DoctorSeverity::Fail
    } else if summary.warn > 0 {
        DoctorSeverity::Warn
    } else {
        DoctorSeverity::Ok
    };

    DoctorReport {
        generated_at,
        overall,
        summary,
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::AiMode;
    use relux_core::RiskLevel;

    fn ai(brain: &str, configured: bool, disabled: bool, model: &str) -> AiStatus {
        AiStatus {
            mode: AiMode::Deterministic,
            brain: brain.to_string(),
            configured,
            disabled,
            model: model.to_string(),
            timeout_ms: 0,
            api_key_secret: None,
            secret_missing: false,
            reason: String::new(),
            auto_detected: false,
        }
    }

    fn adapter(id: &str, state: AdapterRuntimeState, on_path: bool) -> AdapterRuntimeStatus {
        AdapterRuntimeStatus {
            plugin_id: id.to_string(),
            adapter_name: id.to_string(),
            kind: None,
            configured: state != AdapterRuntimeState::NeedsConfiguration,
            enabled: matches!(state, AdapterRuntimeState::Available),
            // Path-shaped values that MUST NOT leak into any check message.
            command: Some("/secret/home/.relix/bin/claude".to_string()),
            available_on_path: on_path,
            resolved_path: Some("/secret/home/.relix/bin/claude".to_string()),
            timeout_seconds: None,
            max_output_bytes: None,
            working_dir: Some("/secret/home/work".to_string()),
            state,
            detail: String::new(),
        }
    }

    fn tool(executable: ToolExecutability) -> ToolDescriptor {
        ToolDescriptor {
            plugin_id: "relux-plugin-x".to_string(),
            tool_name: "x.run".to_string(),
            description: String::new(),
            permission: "x:run".to_string(),
            risk: RiskLevel::Low,
            source_kind: "bundled".to_string(),
            installed: true,
            enabled: true,
            protected: false,
            executable,
        }
    }

    fn find<'a>(report: &'a DoctorReport, id: &str) -> &'a DoctorCheck {
        report.checks.iter().find(|c| c.id == id).expect("check present")
    }

    fn healthy_inputs<'a>(ai: &'a AiStatus) -> DoctorInputs<'a> {
        DoctorInputs {
            db_ok: true,
            dashboard_bundle_present: true,
            ai,
            adapters: &[],
            tools: &[],
            agent_count: 1,
            pending_approvals: 0,
            runs_needing_action: 0,
            runs_retry_pending: 0,
        }
    }

    #[test]
    fn local_brain_is_info_not_failure() {
        let a = ai("local", false, false, "");
        let report = build_doctor_report(&healthy_inputs(&a), 0);
        assert_eq!(find(&report, "prime.brain").severity, DoctorSeverity::Info);
        // A local-only, freshly-set-up instance is healthy overall (no warn/fail).
        assert_eq!(report.overall, DoctorSeverity::Ok);
    }

    #[test]
    fn openrouter_without_key_fails() {
        let a = ai("openrouter", false, false, "");
        let report = build_doctor_report(&healthy_inputs(&a), 0);
        let brain = find(&report, "prime.brain");
        assert_eq!(brain.severity, DoctorSeverity::Fail);
        assert!(brain.remediation.is_some());
        assert_eq!(brain.action_link.as_deref(), Some("/health"));
        assert_eq!(report.overall, DoctorSeverity::Fail);
    }

    #[test]
    fn openrouter_with_key_is_ok_and_names_model() {
        let a = ai("openrouter", true, false, "anthropic/claude");
        let report = build_doctor_report(&healthy_inputs(&a), 0);
        let brain = find(&report, "prime.brain");
        assert_eq!(brain.severity, DoctorSeverity::Ok);
        assert!(brain.message.contains("anthropic/claude"));
    }

    #[test]
    fn disabled_brain_is_warn() {
        let a = ai("openrouter", true, true, "m");
        let report = build_doctor_report(&healthy_inputs(&a), 0);
        assert_eq!(find(&report, "prime.brain").severity, DoctorSeverity::Warn);
        assert_eq!(report.overall, DoctorSeverity::Warn);
    }

    #[test]
    fn claude_cli_brain_available_is_ok_missing_is_fail() {
        let a = ai("claude_cli", false, false, "");

        let avail = [adapter(
            CLAUDE_CLI_ADAPTER_ID,
            AdapterRuntimeState::Available,
            true,
        )];
        let mut inp = healthy_inputs(&a);
        inp.adapters = &avail;
        let report = build_doctor_report(&inp, 0);
        assert_eq!(find(&report, "prime.brain").severity, DoctorSeverity::Ok);

        let missing = [adapter(
            CLAUDE_CLI_ADAPTER_ID,
            AdapterRuntimeState::MissingBinary,
            false,
        )];
        let mut inp2 = healthy_inputs(&a);
        inp2.adapters = &missing;
        let report2 = build_doctor_report(&inp2, 0);
        let brain = find(&report2, "prime.brain");
        assert_eq!(brain.severity, DoctorSeverity::Fail);
        assert_eq!(brain.action_link.as_deref(), Some("/crew"));
    }

    #[test]
    fn real_work_adapter_optional_is_info_available_is_ok() {
        let a = ai("local", false, false, "");
        let report = build_doctor_report(&healthy_inputs(&a), 0);
        // No adapters → optional info, not a failure.
        assert_eq!(
            find(&report, "adapters.real_work").severity,
            DoctorSeverity::Info
        );

        let avail = [adapter(CODEX_CLI_ADAPTER_ID, AdapterRuntimeState::Available, true)];
        let mut inp = healthy_inputs(&a);
        inp.adapters = &avail;
        let report2 = build_doctor_report(&inp, 0);
        let rw = find(&report2, "adapters.real_work");
        assert_eq!(rw.severity, DoctorSeverity::Ok);
        assert!(rw.message.contains("Codex CLI"));
    }

    #[test]
    fn tools_needing_runtime_warn_ready_ok() {
        let a = ai("local", false, false, "");

        let needs = [tool(ToolExecutability::RuntimeNotConfigured)];
        let mut inp = healthy_inputs(&a);
        inp.tools = &needs;
        let report = build_doctor_report(&inp, 0);
        assert_eq!(
            find(&report, "plugins.tools").severity,
            DoctorSeverity::Warn
        );

        let ready = [tool(ToolExecutability::Ready)];
        let mut inp2 = healthy_inputs(&a);
        inp2.tools = &ready;
        let report2 = build_doctor_report(&inp2, 0);
        assert_eq!(find(&report2, "plugins.tools").severity, DoctorSeverity::Ok);
    }

    #[test]
    fn store_failure_fails_and_pending_approvals_warn() {
        let a = ai("local", false, false, "");
        let mut inp = healthy_inputs(&a);
        inp.db_ok = false;
        inp.pending_approvals = 2;
        let report = build_doctor_report(&inp, 0);
        assert_eq!(find(&report, "kernel.store").severity, DoctorSeverity::Fail);
        let appr = find(&report, "approvals.pending");
        assert_eq!(appr.severity, DoctorSeverity::Warn);
        assert!(appr.message.contains('2'));
        assert_eq!(report.overall, DoctorSeverity::Fail);
    }

    #[test]
    fn missing_bundle_is_warn() {
        let a = ai("local", false, false, "");
        let mut inp = healthy_inputs(&a);
        inp.dashboard_bundle_present = false;
        let report = build_doctor_report(&inp, 0);
        assert_eq!(
            find(&report, "dashboard.bundle").severity,
            DoctorSeverity::Warn
        );
    }

    #[test]
    fn report_never_leaks_paths_or_secrets() {
        // The Doctor takes no path inputs, so even an adapter whose resolved
        // binary/working-dir LOOK like secret paths can never surface in any row.
        let a = ai("claude_cli", false, false, "");
        let adapters = [
            adapter(CLAUDE_CLI_ADAPTER_ID, AdapterRuntimeState::Available, true),
            adapter(CODEX_CLI_ADAPTER_ID, AdapterRuntimeState::Disabled, true),
        ];
        let mut inp = healthy_inputs(&a);
        inp.adapters = &adapters;
        let report = build_doctor_report(&inp, 12345);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("/secret/"), "path leaked into doctor report: {json}");
        assert!(!json.contains(".relix/bin"), "path leaked into doctor report");
        assert_eq!(report.generated_at, 12345);
    }

    #[test]
    fn fully_healthy_local_instance_is_ok_overall() {
        let a = ai("local", false, false, "");
        let ready = [tool(ToolExecutability::Ready)];
        let mut inp = healthy_inputs(&a);
        inp.tools = &ready;
        let report = build_doctor_report(&inp, 0);
        assert_eq!(report.overall, DoctorSeverity::Ok);
        assert_eq!(report.summary.fail, 0);
        assert_eq!(report.summary.warn, 0);
        assert_eq!(report.checks.len(), 8);
    }

    #[test]
    fn runs_recovery_warns_on_action_needed_and_infos_on_retry_pending() {
        let a = ai("local", false, false, "");

        // A failure that needs an operator → warn, with a /work action link.
        let mut inp = healthy_inputs(&a);
        inp.runs_needing_action = 2;
        inp.runs_retry_pending = 1;
        let report = build_doctor_report(&inp, 0);
        let row = find(&report, "runs.recovery");
        assert_eq!(row.severity, DoctorSeverity::Warn);
        assert!(row.message.contains('2'));
        assert!(row.message.contains("retry automatically"));
        assert_eq!(row.action_link.as_deref(), Some("/work"));

        // Only transient retries pending → info, never a failure.
        let mut inp2 = healthy_inputs(&a);
        inp2.runs_retry_pending = 3;
        let report2 = build_doctor_report(&inp2, 0);
        let row2 = find(&report2, "runs.recovery");
        assert_eq!(row2.severity, DoctorSeverity::Info);
        assert_eq!(report2.overall, DoctorSeverity::Ok);

        // Clean board → ok.
        let report3 = build_doctor_report(&healthy_inputs(&a), 0);
        assert_eq!(find(&report3, "runs.recovery").severity, DoctorSeverity::Ok);
    }
}
