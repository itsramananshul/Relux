//! Honest, tolerant parsing of a coding-agent CLI's captured output.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run: `usage`/`cost`),
//! section 9.7 (Run Event: `structured_payload`), and the "Adapter run depth"
//! slice (parse a structured result envelope when one is present, otherwise
//! surface the plain text honestly - never fabricate a tool call or success).
//!
//! Some CLIs can emit a single JSON **result envelope** describing the run (the
//! Claude CLI's `--output-format json` produces `{ "type": "result", "result":
//! "...", "is_error": false, "total_cost_usd": 0.01, "num_turns": 3, "usage":
//! {...}, "duration_ms": 1234 }`). When the captured stdout is exactly such an
//! object, [`parse_adapter_result`] lifts the human text out of `result` and
//! records the structured metrics. When it is plain prose (Codex `exec`, a
//! generic command, or Claude in text mode), the parser degrades to the plain
//! text with `structured = false`. It never invents fields that were not present.

use serde::{Deserialize, Serialize};

use crate::artifact::{capture_run_artifacts, RunArtifact};
use crate::AdapterKind;

/// The outcome of interpreting an adapter's captured stdout.
///
/// `text` is always the best human-readable summary we have (the envelope's
/// `result`, or the raw stdout). The remaining fields are only `Some`/`true` when
/// they were genuinely present in a parsed envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterResultSummary {
    /// True only when stdout was a recognized JSON result envelope.
    pub structured: bool,
    /// The human-facing summary text (envelope `result`, else the raw stdout).
    pub text: String,
    /// The envelope's `is_error` flag, when present. An envelope can report an
    /// error even on a process exit of 0; callers may surface this honestly.
    pub is_error: Option<bool>,
    /// Reported cost in USD (`total_cost_usd`/`cost_usd`), when present.
    pub cost_usd: Option<f64>,
    /// Number of model turns the CLI took, when present.
    pub num_turns: Option<u64>,
    /// The raw `usage` object, when present.
    pub usage: Option<serde_json::Value>,
    /// The envelope's self-reported duration in milliseconds, when present.
    pub duration_ms: Option<u64>,
    /// Read-only artifact references the envelope declared (`artifacts: [...]`).
    /// Bounded, redacted, path-sanitized references — never the file contents, a
    /// diff, or an apply plan (master plan section 9.6 / section 15). Empty when
    /// the envelope declared none.
    pub artifacts: Vec<RunArtifact>,
}

impl AdapterResultSummary {
    /// A plain-text summary with no structured metrics.
    fn plain(text: impl Into<String>) -> Self {
        Self {
            structured: false,
            text: text.into(),
            is_error: None,
            cost_usd: None,
            num_turns: None,
            usage: None,
            duration_ms: None,
            artifacts: Vec::new(),
        }
    }
}

/// A short, stable adapter-source label for an [`AdapterKind`], used to tag
/// captured artifact references ("from claude-cli").
fn adapter_source_label(kind: AdapterKind) -> &'static str {
    match kind {
        AdapterKind::LocalPrime => "local-prime",
        AdapterKind::ClaudeCli => "claude-cli",
        AdapterKind::CodexCli => "codex-cli",
        AdapterKind::Command => "command",
    }
}

/// Parse an adapter's (already secret-redacted, capped) stdout into a structured
/// summary when it is a recognized JSON result envelope, otherwise fall back to
/// the plain text. `kind` is advisory only - the parser detects the envelope by
/// shape, so it stays correct even if a CLI changes its flags.
pub fn parse_adapter_result(stdout: &str, kind: AdapterKind) -> AdapterResultSummary {
    let trimmed = stdout.trim();
    // Only attempt JSON parsing when the text actually looks like a single JSON
    // object. This avoids mis-parsing prose that merely contains braces.
    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return AdapterResultSummary::plain(stdout.to_string());
    }
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return AdapterResultSummary::plain(stdout.to_string()),
    };
    let obj = match value.as_object() {
        Some(o) => o,
        None => return AdapterResultSummary::plain(stdout.to_string()),
    };

    // Recognize the Claude-style result envelope: a `result` string, optionally
    // tagged `"type":"result"`. We require a `result` field so an arbitrary JSON
    // blob the agent happened to print is not mistaken for an envelope.
    let result_text = obj.get("result").and_then(|v| v.as_str());
    let is_result_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "result")
        .unwrap_or(false);
    if result_text.is_none() && !is_result_type {
        // Not an envelope we understand - keep the raw JSON as honest plain text.
        return AdapterResultSummary::plain(stdout.to_string());
    }

    let text = result_text
        .map(|s| s.to_string())
        .unwrap_or_else(|| stdout.to_string());
    let is_error = obj.get("is_error").and_then(|v| v.as_bool());
    let cost_usd = obj
        .get("total_cost_usd")
        .or_else(|| obj.get("cost_usd"))
        .and_then(|v| v.as_f64());
    let num_turns = obj.get("num_turns").and_then(|v| v.as_u64());
    let usage = obj.get("usage").cloned();
    let duration_ms = obj.get("duration_ms").and_then(|v| v.as_u64());
    // Capture any declared artifact references read-only (bounded, redacted,
    // path-sanitized). Only from a recognized envelope - an arbitrary JSON blob
    // never reaches here. Never reads the filesystem.
    let artifacts = capture_run_artifacts(obj.get("artifacts"), adapter_source_label(kind));

    AdapterResultSummary {
        structured: true,
        text,
        is_error,
        cost_usd,
        num_turns,
        usage,
        duration_ms,
        artifacts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_result_envelope() {
        let stdout = r#"{
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "I summarized the repo: 3 crates, all green.",
            "total_cost_usd": 0.0123,
            "num_turns": 4,
            "duration_ms": 8123,
            "usage": { "input_tokens": 1200, "output_tokens": 340 }
        }"#;
        let s = parse_adapter_result(stdout, AdapterKind::ClaudeCli);
        assert!(s.structured);
        assert_eq!(s.text, "I summarized the repo: 3 crates, all green.");
        assert_eq!(s.is_error, Some(false));
        assert_eq!(s.cost_usd, Some(0.0123));
        assert_eq!(s.num_turns, Some(4));
        assert_eq!(s.duration_ms, Some(8123));
        assert_eq!(
            s.usage.as_ref().and_then(|u| u.get("output_tokens")).and_then(|v| v.as_u64()),
            Some(340)
        );
    }

    #[test]
    fn envelope_can_report_error_on_clean_exit() {
        let stdout = r#"{"type":"result","is_error":true,"result":"hit a rate limit"}"#;
        let s = parse_adapter_result(stdout, AdapterKind::ClaudeCli);
        assert!(s.structured);
        assert_eq!(s.is_error, Some(true));
        assert_eq!(s.text, "hit a rate limit");
    }

    #[test]
    fn plain_text_is_passed_through_unstructured() {
        let stdout = "Done. I edited src/main.rs and ran the tests.";
        let s = parse_adapter_result(stdout, AdapterKind::CodexCli);
        assert!(!s.structured);
        assert_eq!(s.text, stdout);
        assert_eq!(s.cost_usd, None);
        assert_eq!(s.num_turns, None);
    }

    #[test]
    fn malformed_json_degrades_to_plain_text() {
        let stdout = "{ this is not valid json";
        let s = parse_adapter_result(stdout, AdapterKind::ClaudeCli);
        assert!(!s.structured);
        assert_eq!(s.text, stdout);
    }

    #[test]
    fn json_object_without_result_is_not_treated_as_envelope() {
        // An agent that prints some unrelated JSON must not be mistaken for a
        // structured success/result.
        let stdout = r#"{"files":["a.rs","b.rs"],"note":"changed two files"}"#;
        let s = parse_adapter_result(stdout, AdapterKind::Command);
        assert!(!s.structured);
        assert_eq!(s.text, stdout);
    }

    #[test]
    fn envelope_captures_artifact_references() {
        let stdout = r#"{
            "type": "result",
            "result": "Edited two files.",
            "artifacts": [
                { "name": "main.rs", "type": "file", "path": "src/main.rs", "summary": "added a fn" },
                { "type": "diff", "path": "/abs/should/drop" }
            ]
        }"#;
        let s = parse_adapter_result(stdout, AdapterKind::ClaudeCli);
        assert!(s.structured);
        assert_eq!(s.artifacts.len(), 2);
        assert_eq!(s.artifacts[0].name, "main.rs");
        assert_eq!(s.artifacts[0].source, "claude-cli");
        assert_eq!(s.artifacts[0].path.as_deref(), Some("src/main.rs"));
        // The absolute path is dropped, but the reference is still captured.
        assert_eq!(s.artifacts[1].path, None);
    }

    #[test]
    fn plain_text_has_no_artifacts() {
        let s = parse_adapter_result("just prose", AdapterKind::CodexCli);
        assert!(s.artifacts.is_empty());
    }

    #[test]
    fn envelope_without_artifacts_is_empty_not_fabricated() {
        let stdout = r#"{"type":"result","result":"ok"}"#;
        let s = parse_adapter_result(stdout, AdapterKind::ClaudeCli);
        assert!(s.structured);
        assert!(s.artifacts.is_empty());
    }

    #[test]
    fn prose_with_braces_is_not_parsed_as_json() {
        let stdout = "I changed the struct to { id, name } as requested.";
        let s = parse_adapter_result(stdout, AdapterKind::ClaudeCli);
        assert!(!s.structured);
        assert_eq!(s.text, stdout);
    }
}
