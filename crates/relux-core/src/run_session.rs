//! Durable, redacted **session identity / handoff / resume** metadata for a run.
//!
//! Spec ref: `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3 (Agent / subagent / session
//! model) and `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run). Reference mapping
//! (`docs/reference-driven-development.md`, BINDING):
//!
//! - OpenClaw `src/agents/acp-spawn.ts` (`resumeSessionId`,
//!   `validateAcpResumeSessionOwnership`) and `src/agents/command/attempt-execution.ts`
//!   (`getCliSessionBinding(sessionEntry, "claude-cli").sessionId`,
//!   `runCliWithSession(nextCliSessionId, binding)`,
//!   `claudeCliSessionTranscriptHasContent` → reset-on-missing,
//!   `FailoverReason::session_expired`): a per-provider CLI **session binding**
//!   (the `sessionId`) is captured, then optionally **resumed** through the same
//!   spawn gate; an expired/empty session is reset to a fresh run rather than
//!   faked.
//!
//! Relux mapping — this module is the bounded, honest **metadata** layer:
//!
//! - The Claude CLI's `--output-format json` result envelope carries a top-level
//!   `session_id`. [`crate::parse_adapter_result`] lifts it; [`RunSession::from_envelope`]
//!   sanitizes it (argv-safe charset, leading-dash rejected, length-bounded) and
//!   records it on the [`crate::Run`] with the adapter source label and a per-kind
//!   `resume_supported` capability flag ([`crate::AdapterKind::resume_supported`]).
//! - We store ONLY the bounded, sanitized session id + source + capability — never
//!   raw provider envelopes, tokens, or full logs.
//! - Whether a stored session can actually be resumed is a pure decision
//!   ([`plan_resume`]); the kernel wires the *supported* case through the existing
//!   governed adapter gate and honestly refuses the rest.

use serde::{Deserialize, Serialize};

use crate::AdapterKind;

/// The maximum length of a stored adapter session id. A provider session id is a
/// short token (typically a UUID); anything longer is truncated defensively so a
/// runaway value can never bloat the run record or an argv element.
pub const MAX_SESSION_ID_LEN: usize = 128;

/// Sanitize a raw provider session id into a bounded, argv-safe value, or `None`
/// when nothing safe remains.
///
/// A session id is threaded into a CLI argv (`--resume <id>`) on the resume path,
/// so it must be safe even though the spawn is argv-only (no shell). We keep only
/// an allowlisted charset (ASCII alphanumerics plus `-` `_` `.`), reject a value
/// that would look like a flag (a leading `-`), and bound the length. An empty or
/// fully-stripped value yields `None` (no session captured) rather than an empty
/// string — we never fabricate a session id.
pub fn sanitize_session_id(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(MAX_SESSION_ID_LEN)
        .collect();
    // A value that starts with '-' could be misread as a flag after `--resume`;
    // strip leading dashes/dots so it is unambiguously a positional value.
    let cleaned = cleaned.trim_start_matches(['-', '.']).to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Bounded, redacted **session identity** captured for a run from its adapter's
/// structured result envelope. This is the durable handoff record: who the
/// provider session was, where it came from, and whether Relux can safely resume
/// it. It carries no secrets — just a sanitized session id, the adapter source
/// label, and a capability flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSession {
    /// The provider's session id, sanitized + bounded (e.g. the Claude CLI
    /// envelope's `session_id`). Safe to display and to thread into a governed
    /// `--resume` argv element.
    pub adapter_session_id: String,
    /// The adapter source label this session came from (e.g. `claude-cli`).
    pub source: String,
    /// Whether Relux can perform a safe, non-interactive resume of THIS session
    /// through the governed adapter gate ([`AdapterKind::resume_supported`]). When
    /// `false`, the session id is still captured for handoff/audit/manual
    /// continuation, but the kernel honestly refuses a `run.resume` (the operator
    /// re-runs fresh — a distinct action).
    pub resume_supported: bool,
}

impl RunSession {
    /// Build a [`RunSession`] from an adapter envelope's `session_id` (if any) and
    /// the adapter kind. Returns `None` when no session id was present or nothing
    /// safe remained after sanitization (we never fabricate one).
    pub fn from_envelope(session_id: Option<&str>, kind: &AdapterKind) -> Option<Self> {
        let adapter_session_id = sanitize_session_id(session_id?)?;
        Some(Self {
            adapter_session_id,
            source: kind.source_label().to_string(),
            resume_supported: kind.resume_supported(),
        })
    }
}

/// The honest outcome of deciding whether a run can be resumed (pure; the single
/// source of truth shared by the kernel action and the dashboard label).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeDisposition {
    /// The run carries a resumable provider session; the kernel may thread this
    /// session id through the governed adapter gate to continue it.
    Supported {
        adapter_session_id: String,
        source: String,
    },
    /// Resume is not available; `reason` is an operator-facing, secret-free
    /// explanation (no captured session, an adapter without safe non-interactive
    /// resume, or a run still in flight). The honest alternative is a fresh re-run.
    NotSupported { reason: String },
}

impl ResumeDisposition {
    /// Whether this disposition permits a resume.
    pub fn is_supported(&self) -> bool {
        matches!(self, ResumeDisposition::Supported { .. })
    }
}

/// Decide whether a run can be resumed, from its captured session and whether it
/// has reached a terminal state. Pure and exhaustive so the kernel never fakes a
/// resume and the UI label matches the action exactly.
///
/// A resume continues a prior provider session, so the run must be terminal
/// (`terminal == true`, i.e. completed or failed) and must carry a session whose
/// adapter `resume_supported` is set. Everything else is an honest `NotSupported`
/// with the specific reason.
pub fn plan_resume(session: Option<&RunSession>, terminal: bool) -> ResumeDisposition {
    if !terminal {
        return ResumeDisposition::NotSupported {
            reason: "the run is still in flight; wait for it to finish before resuming its session"
                .to_string(),
        };
    }
    match session {
        None => ResumeDisposition::NotSupported {
            reason: "no provider session id was captured for this run, so there is nothing to \
                     resume; start a fresh run instead"
                .to_string(),
        },
        Some(s) if !s.resume_supported => ResumeDisposition::NotSupported {
            reason: format!(
                "the {} adapter does not support safe non-interactive session resume; start a \
                 fresh run instead (a fresh re-run is a distinct action)",
                s.source
            ),
        },
        Some(s) => ResumeDisposition::Supported {
            adapter_session_id: s.adapter_session_id.clone(),
            source: s.source.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_a_uuid_session_id() {
        let id = sanitize_session_id("  9f3c1e2a-7b6d-4c8e-9a01-abc123DEF456  ").unwrap();
        assert_eq!(id, "9f3c1e2a-7b6d-4c8e-9a01-abc123DEF456");
    }

    #[test]
    fn strips_unsafe_chars_and_leading_dash() {
        // Spaces and shell-ish chars are dropped; a leading dash can't survive to
        // be misread as a flag after `--resume`.
        let id = sanitize_session_id("--resume; rm -rf /  sess_01").unwrap();
        assert!(!id.starts_with('-'));
        assert!(!id.contains(' '));
        assert!(!id.contains(';'));
        assert!(!id.contains('/'));
    }

    #[test]
    fn empty_or_all_unsafe_yields_none() {
        assert_eq!(sanitize_session_id("   "), None);
        assert_eq!(sanitize_session_id("///"), None);
        assert_eq!(sanitize_session_id("---"), None);
    }

    #[test]
    fn long_session_id_is_bounded() {
        let raw = "a".repeat(500);
        let id = sanitize_session_id(&raw).unwrap();
        assert_eq!(id.len(), MAX_SESSION_ID_LEN);
    }

    #[test]
    fn from_envelope_sets_capability_per_kind() {
        let claude = RunSession::from_envelope(Some("sess-123"), &AdapterKind::ClaudeCli).unwrap();
        assert_eq!(claude.adapter_session_id, "sess-123");
        assert_eq!(claude.source, "claude-cli");
        assert!(claude.resume_supported);

        // Codex/Command capture nothing here (no session id in their path); but
        // even if one were present, the capability flag is honest (false).
        let codex = RunSession::from_envelope(Some("sess-9"), &AdapterKind::CodexCli).unwrap();
        assert!(!codex.resume_supported);
        assert_eq!(codex.source, "codex-cli");
    }

    #[test]
    fn from_envelope_without_session_id_is_none() {
        assert!(RunSession::from_envelope(None, &AdapterKind::ClaudeCli).is_none());
        assert!(RunSession::from_envelope(Some("   "), &AdapterKind::ClaudeCli).is_none());
    }

    #[test]
    fn plan_resume_supported_for_terminal_claude_session() {
        let s = RunSession {
            adapter_session_id: "sess-123".to_string(),
            source: "claude-cli".to_string(),
            resume_supported: true,
        };
        let d = plan_resume(Some(&s), true);
        assert!(d.is_supported());
        match d {
            ResumeDisposition::Supported { adapter_session_id, source } => {
                assert_eq!(adapter_session_id, "sess-123");
                assert_eq!(source, "claude-cli");
            }
            _ => panic!("expected Supported"),
        }
    }

    #[test]
    fn plan_resume_refuses_in_flight_run() {
        let s = RunSession {
            adapter_session_id: "sess-123".to_string(),
            source: "claude-cli".to_string(),
            resume_supported: true,
        };
        assert!(!plan_resume(Some(&s), false).is_supported());
    }

    #[test]
    fn plan_resume_refuses_without_session() {
        assert!(!plan_resume(None, true).is_supported());
    }

    #[test]
    fn plan_resume_refuses_unsupported_adapter() {
        let s = RunSession {
            adapter_session_id: "sess-9".to_string(),
            source: "codex-cli".to_string(),
            resume_supported: false,
        };
        let d = plan_resume(Some(&s), true);
        assert!(!d.is_supported());
        match d {
            ResumeDisposition::NotSupported { reason } => {
                assert!(reason.contains("codex-cli"));
                assert!(reason.contains("fresh"));
            }
            _ => panic!("expected NotSupported"),
        }
    }
}
