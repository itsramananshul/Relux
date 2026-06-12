//! Diagnostic narrative pass for a failed run (the cheap, read-only "explain the
//! failure" half of the §3.3b recovery flow).
//!
//! Spec ref: `docs/relix-execution-and-issue-design.md` §3.3b ("Spawn a **cheap
//! diagnostic pass** … *forbidden* from doing deliverable work — diagnosis only.
//! It reads the run logs + issue context and produces (1) a plain-language root
//! cause and (2) a recommendation") and `docs/relix-dashboard-design.md` §6.10
//! ("the cheap **diagnostic LLM pass** that writes a richer narrative root cause").
//!
//! The deterministic recovery card (`apps/dashboard/src/recovery.ts`) already
//! classifies a failure from the kernel's structured `failure_class`. This module
//! is the *narrative* layer on top: an explicit, operator-triggered, READ-ONLY
//! pass that hands a bounded + redacted slice of the run's context to the
//! configured brain and asks for a concise written diagnosis. It is diagnosis
//! only — it has no tools, creates no tasks, starts no runs, mutates nothing.
//!
//! ## Reference grounding (`docs/reference-driven-development.md`, BINDING)
//!
//! - **Hermes** `reference/hermes-agent-main/agent/auxiliary_client.py` — the
//!   shared "side task" client: a separate, bounded model call (context
//!   compression, vision, extraction) that is distinct from the main agent loop
//!   and never does deliverable work. Our diagnostic pass mirrors that shape: a
//!   one-shot side call over a bounded context, with a graceful "no provider"
//!   fall-through (Hermes returns `None` and the caller degrades).
//! - **Hermes** `reference/hermes-agent-main/agent/error_classifier.py`
//!   (`_extract_message` clamps to 500 chars; `_sanitize_tool_error` to 2000) —
//!   the "clamp the provider envelope before it travels" rule we already mirror in
//!   `relux_core::run_failure::safe_public_message`; here we re-apply the same
//!   bound on every field of the context we build, and on the model's output.
//!
//! ## Safety
//!
//! Everything in this module is pure (no clock, network, or I/O) so the context
//! bounding/redaction, the prompt framing, and the model-output assembly are all
//! unit-tested. The impure half (the actual provider call) lives in
//! [`crate::ai::diagnose_via_openrouter`] and is isolated behind [`assemble`],
//! which takes the model's *already-returned* text (or `None`) — so the whole
//! narrative shape is testable with a provider test-double.

use serde::Serialize;

use relux_core::redact_secrets;

/// Max characters kept for the run's failure text (error/summary) in the context.
/// Matches Hermes' `_extract_message` 500-char clamp.
pub const MAX_DIAG_FAILURE_CHARS: usize = 800;
/// Max characters kept for the bounded log tail handed to the model (the MOST
/// RECENT chars — the failure is at the end).
pub const MAX_DIAG_LOG_TAIL_CHARS: usize = 2000;
/// Max number of (already-bounded) log lines folded into the context.
pub const MAX_DIAG_LOG_LINES: usize = 40;
/// Max characters kept for the task title in the context.
pub const MAX_DIAG_TASK_TITLE_CHARS: usize = 160;
/// Hard cap on the narrative we accept back from the model, so a runaway reply
/// can never bloat the response regardless of what the provider returns.
pub const MAX_DIAG_NARRATIVE_CHARS: usize = 1500;

/// The bounded, redacted context the diagnostic prompt is built from. Each field
/// is already clamped + secret-redacted by [`DiagnosticContext::build`], so the
/// prompt builder can splice them in verbatim and nothing unbounded or secret
/// ever reaches the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticContext {
    pub run_id: String,
    pub task_id: String,
    pub task_title: Option<String>,
    /// The run's lifecycle status (e.g. "failed", "cancelled").
    pub status: String,
    /// The structured failure class wire string, when one was recorded.
    pub failure_class: Option<String>,
    /// The adapter/runtime that ran (the plugin id).
    pub adapter: Option<String>,
    /// The redacted + clamped failure text (the run's error, else its summary).
    pub failure_text: Option<String>,
    /// The redacted + clamped MOST-RECENT log tail, when any log was captured.
    pub log_tail: Option<String>,
}

impl DiagnosticContext {
    /// Build the bounded, redacted context from the run's already-extracted
    /// fields + its captured log lines. Pure: redaction + clamping only.
    ///
    /// `failure_text` is the run's error (preferred) or summary; `log_lines` are
    /// the run-log tail lines (themselves already server-redacted — we re-redact
    /// belt-and-braces). Both are clamped so a pathological blob can't flood the
    /// prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        run_id: impl Into<String>,
        task_id: impl Into<String>,
        task_title: Option<&str>,
        status: impl Into<String>,
        failure_class: Option<&str>,
        adapter: Option<&str>,
        failure_text: Option<&str>,
        log_lines: &[String],
    ) -> Self {
        let task_title = task_title
            .map(|t| clamp_head(t, MAX_DIAG_TASK_TITLE_CHARS))
            .filter(|t| !t.is_empty());
        let failure_text = failure_text
            .map(|t| redact_secrets(&clamp_head(t, MAX_DIAG_FAILURE_CHARS)))
            .filter(|t| !t.trim().is_empty());
        // Keep only the most recent lines, redact each, then clamp the joined tail
        // to its most-recent chars (the failure is at the end).
        let log_tail = if log_lines.is_empty() {
            None
        } else {
            let start = log_lines.len().saturating_sub(MAX_DIAG_LOG_LINES);
            let joined = log_lines[start..]
                .iter()
                .map(|l| redact_secrets(l))
                .collect::<Vec<_>>()
                .join("\n");
            let clamped = clamp_tail(&joined, MAX_DIAG_LOG_TAIL_CHARS);
            if clamped.trim().is_empty() {
                None
            } else {
                Some(clamped)
            }
        };
        Self {
            run_id: run_id.into(),
            task_id: task_id.into(),
            task_title,
            status: status.into(),
            failure_class: failure_class.map(|s| s.to_string()),
            adapter: adapter.map(|s| s.to_string()),
            failure_text,
            log_tail,
        }
    }
}

/// Provenance of the returned narrative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticMode {
    /// The narrative was written by the configured brain.
    Model,
    /// No usable narrative — either no provider is configured, or the provider
    /// didn't return one this time. The `narrative` is an honest fallback that
    /// points at the deterministic recovery card.
    Unavailable,
}

impl DiagnosticMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The wire response for `POST /v1/relux/runs/:id/diagnose`.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticNarrative {
    pub run_id: String,
    pub mode: DiagnosticMode,
    /// The model id that wrote the narrative, when `mode == Model`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The narrative (a model diagnosis, or the honest fallback message).
    pub narrative: String,
    /// Whether a brain provider is configured at all (so the UI can offer
    /// "configure a provider" guidance when it is not).
    pub provider_configured: bool,
}

/// The diagnostic system prompt — pins the read-only, no-authority framing so the
/// model never claims it acted. Kept here (not in `ai.rs`) so the framing lives
/// next to the prompt body and is covered by this module's tests.
pub const DIAGNOSTIC_SYSTEM: &str = "You are a read-only diagnostic assistant for the Relux \
control plane. You analyze a SINGLE failed run from the bounded context you are given and \
explain it to an operator. You have NO tools and NO authority this turn: never claim you created \
a task, started or retried a run, changed any status, granted a permission, or fixed anything. \
Do not invent log lines, ids, error text, or any fact not present in the context. Be concise and \
concrete. Use plain ASCII.";

/// Build the diagnostic user prompt from the bounded context. Pure. Asks for the
/// four §3.3b parts: likely cause, evidence, recommended next action, and the
/// uncertainty / what-to-inspect-next.
pub fn build_diagnostic_prompt(ctx: &DiagnosticContext) -> String {
    let mut s = String::new();
    s.push_str(
        "Diagnose why this Relux run failed, using ONLY the context below. Do not perform any \
action — this is read-only analysis.\n\nContext:\n",
    );
    s.push_str(&format!("- Run: id {}, status {}", ctx.run_id, ctx.status));
    if let Some(cls) = &ctx.failure_class {
        s.push_str(&format!(", failure class {cls}"));
    }
    if let Some(adapter) = &ctx.adapter {
        s.push_str(&format!(", adapter {adapter}"));
    }
    s.push('\n');
    s.push_str(&format!("- Task: id {}", ctx.task_id));
    if let Some(title) = &ctx.task_title {
        s.push_str(&format!(", title \"{title}\""));
    }
    s.push('\n');
    if let Some(text) = &ctx.failure_text {
        s.push_str(&format!("- Failure text: {text}\n"));
    }
    if let Some(tail) = &ctx.log_tail {
        s.push_str("- Recent log tail (most recent lines, bounded + redacted):\n```\n");
        s.push_str(tail);
        s.push_str("\n```\n");
    }
    s.push_str(
        "\nProvide a concise diagnosis with these four labelled parts:\n\
1. Likely cause: the most plausible reason this run failed.\n\
2. Evidence: cite the specific signals above (status, failure class, failure text, or log lines) \
that support it.\n\
3. Recommended next action: the single best thing the operator should do next.\n\
4. Uncertainty / what to inspect next: what you are unsure about and where to look to confirm.\n\
Keep it under about 200 words. If the context is too thin to be sure, say so plainly.",
    );
    s
}

/// Assemble the final narrative from the model's (already-returned) output, or
/// fall back cleanly. Pure — the impure provider call happens in the caller and
/// its result (or `None`) is passed in, so this is fully testable with a
/// test-double.
///
/// - `model_output`: the brain's raw reply, or `None` on no-provider / failure.
/// - `model_name`: the model id, folded into a `Model` result.
/// - `configured`: whether a brain provider is configured at all (drives the
///   fallback wording and `provider_configured`).
pub fn assemble(
    ctx: &DiagnosticContext,
    model_output: Option<String>,
    model_name: Option<String>,
    configured: bool,
) -> DiagnosticNarrative {
    match model_output {
        Some(raw) if !raw.trim().is_empty() => DiagnosticNarrative {
            run_id: ctx.run_id.clone(),
            mode: DiagnosticMode::Model,
            model: model_name,
            // Re-redact + clamp: the context was redacted, but never trust a
            // model echo, and bound the length regardless of the provider.
            narrative: clamp_head(&redact_secrets(raw.trim()), MAX_DIAG_NARRATIVE_CHARS),
            provider_configured: true,
        },
        _ => DiagnosticNarrative {
            run_id: ctx.run_id.clone(),
            mode: DiagnosticMode::Unavailable,
            model: None,
            narrative: unavailable_message(configured),
            provider_configured: configured,
        },
    }
}

/// A clean, honest fallback narrative when no model diagnosis is available. It
/// points the operator at the deterministic recovery card (already shown) and, if
/// no provider is configured, offers the configure path (§3.3b: degrade, never
/// fabricate).
pub fn unavailable_message(configured: bool) -> String {
    if configured {
        "The diagnostic model is configured but did not return a usable narrative this time \
(often a transient provider hiccup or timeout). The recovery card above still shows the \
deterministic classified cause and the recommended next step — try Analyze failure again, or use \
Investigate with Prime to debug it conversationally."
            .to_string()
    } else {
        "No diagnostic model is configured, so a written narrative is not available. The recovery \
card above already classifies this failure and recommends a next step. To get a richer narrative, \
configure an OpenRouter brain under Settings, then run Analyze failure again."
            .to_string()
    }
}

// --- bounding helpers ------------------------------------------------------

/// Clamp to a head-bounded length with an ellipsis when truncated (char-safe).
fn clamp_head(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let kept: String = t.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Clamp to the MOST RECENT `max` chars (the failure is at the end), prefixing an
/// honest "earlier lines omitted" marker when truncated (char-safe).
fn clamp_tail(text: &str, max: usize) -> String {
    let t = text.trim_end();
    let count = t.chars().count();
    if count <= max {
        return t.to_string();
    }
    let skip = count - max;
    let kept: String = t.chars().skip(skip).collect();
    format!("… (earlier lines omitted)\n{kept}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DiagnosticContext {
        DiagnosticContext::build(
            "run-1",
            "task-1",
            Some("Ship the thing"),
            "failed",
            Some("auth_required"),
            Some("relux-adapter-claude-cli"),
            Some("401 Unauthorized: invalid api key"),
            &["line one".to_string(), "line two".to_string()],
        )
    }

    #[test]
    fn build_redacts_and_bounds_the_context() {
        let secret = "boom failed Authorization: Bearer sk-ant-api03-SECRETKEYVALUE1234567890 leaked";
        let huge_line = "x".repeat(5000);
        let c = DiagnosticContext::build(
            "run-9",
            "task-9",
            Some(&"T".repeat(500)),
            "failed",
            Some("unknown"),
            None,
            Some(secret),
            &[huge_line.clone()],
        );
        // Failure text: secret redacted, length clamped.
        let ft = c.failure_text.expect("failure text");
        assert!(!ft.contains("SECRETKEYVALUE"), "secret leaked: {ft}");
        assert!(ft.contains("REDACTED"));
        assert!(ft.chars().count() <= MAX_DIAG_FAILURE_CHARS);
        // Title clamped.
        assert!(c.task_title.unwrap().chars().count() <= MAX_DIAG_TASK_TITLE_CHARS);
        // Log tail clamped to its most-recent chars.
        let tail = c.log_tail.expect("log tail");
        assert!(tail.chars().count() <= MAX_DIAG_LOG_TAIL_CHARS + 40);
        assert!(tail.contains("earlier lines omitted"));
    }

    #[test]
    fn build_keeps_only_the_most_recent_lines() {
        let lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let c = DiagnosticContext::build(
            "r", "t", None, "failed", None, None, None, &lines,
        );
        let tail = c.log_tail.expect("tail");
        assert!(tail.contains("line 199"), "must keep the newest line");
        assert!(!tail.contains("\nline 0\n"), "must drop the oldest line");
    }

    #[test]
    fn build_handles_empty_optionals() {
        let c = DiagnosticContext::build("r", "t", None, "failed", None, None, None, &[]);
        assert!(c.task_title.is_none());
        assert!(c.failure_text.is_none());
        assert!(c.log_tail.is_none());
        assert!(c.failure_class.is_none());
    }

    #[test]
    fn prompt_carries_the_context_and_the_four_asks() {
        let p = build_diagnostic_prompt(&ctx());
        // Read-only framing.
        assert!(p.contains("read-only"));
        assert!(p.to_lowercase().contains("do not perform any action"));
        // Context is spliced in.
        assert!(p.contains("run-1"));
        assert!(p.contains("auth_required"));
        assert!(p.contains("relux-adapter-claude-cli"));
        assert!(p.contains("Ship the thing"));
        assert!(p.contains("401 Unauthorized"));
        assert!(p.contains("line two"));
        // The four labelled parts.
        assert!(p.contains("Likely cause:"));
        assert!(p.contains("Evidence:"));
        assert!(p.contains("Recommended next action:"));
        assert!(p.contains("Uncertainty"));
    }

    #[test]
    fn system_prompt_forbids_claiming_action() {
        assert!(DIAGNOSTIC_SYSTEM.contains("NO tools"));
        assert!(DIAGNOSTIC_SYSTEM.to_lowercase().contains("never claim"));
    }

    #[test]
    fn assemble_with_model_output_is_model_mode_and_bounded() {
        let long = "word ".repeat(2000);
        let out = assemble(&ctx(), Some(long), Some("openai/gpt-4o-mini".into()), true);
        assert_eq!(out.mode, DiagnosticMode::Model);
        assert_eq!(out.model.as_deref(), Some("openai/gpt-4o-mini"));
        assert!(out.provider_configured);
        assert!(out.narrative.chars().count() <= MAX_DIAG_NARRATIVE_CHARS);
        assert_eq!(out.run_id, "run-1");
    }

    #[test]
    fn assemble_redacts_a_model_echo() {
        let leak = "Likely cause: the key sk-ant-api03-LEAKED9999999 was rejected.";
        let out = assemble(&ctx(), Some(leak.into()), Some("m".into()), true);
        assert!(!out.narrative.contains("LEAKED9999999"), "model echo leaked a secret");
        assert!(out.narrative.contains("REDACTED"));
    }

    #[test]
    fn assemble_no_provider_is_a_clean_configure_message() {
        let out = assemble(&ctx(), None, None, false);
        assert_eq!(out.mode, DiagnosticMode::Unavailable);
        assert!(out.model.is_none());
        assert!(!out.provider_configured);
        assert!(out.narrative.contains("No diagnostic model is configured"));
        assert!(out.narrative.contains("Settings"));
        // Never fabricates a diagnosis.
        assert!(!out.narrative.to_lowercase().contains("likely cause"));
    }

    #[test]
    fn assemble_configured_but_no_output_is_an_honest_retry_message() {
        let out = assemble(&ctx(), None, Some("m".into()), true);
        assert_eq!(out.mode, DiagnosticMode::Unavailable);
        // model id is dropped when there was no usable output.
        assert!(out.model.is_none());
        assert!(out.provider_configured);
        assert!(out.narrative.contains("did not return a usable narrative"));
    }

    #[test]
    fn assemble_treats_blank_output_as_unavailable() {
        let out = assemble(&ctx(), Some("   \n  ".into()), Some("m".into()), true);
        assert_eq!(out.mode, DiagnosticMode::Unavailable);
    }
}
