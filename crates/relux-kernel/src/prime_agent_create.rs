//! Deterministic enrichment of an `AgentCreation` turn: the agent's **adapter/brain
//! preference** and the **capabilities the user asked for**, parsed from the request
//! and reconciled against live state.
//!
//! ## Why this exists
//!
//! The canonical Prime exchange in `docs/RELUX_MASTER_PLAN.md` §6 is:
//!
//! > User: build a coding agent that can open PRs
//! > Prime: I can do that. I need an adapter plugin, GitHub tool plugin, and scoped
//! >        GitHub permissions. I can create the agent with read/create-PR permissions
//! >        but not merge permission. Proceed?
//!
//! Two things fall out of that and out of the Agent Layer (`§7.3`: an agent has an
//! *adapter plugin* and *permissions*) and the adapter catalog (`§8.1`:
//! `relux-adapter-claude-cli`, `relux-adapter-codex-cli`, …):
//!
//! 1. **Adapter preference.** "create an agent that uses Claude" should put the operative
//!    on the Claude adapter — *when that adapter is actually installed*. The brain-slot
//!    layer ([`crate::prime_agent_slots`]) already honors a brain-proposed adapter against
//!    the live roster, but the deterministic rail hard-codes `relux-adapter-local-prime`
//!    and drops the preference on the floor when no brain is configured.
//! 2. **Capability honesty.** "that can read GitHub" must NOT be silently dropped, and
//!    Prime must NOT fabricate access. Per §6 and §7.5 ("granting broad permissions" is a
//!    reviewed action), Prime names the scoped permission it would need and routes the
//!    grant through the EXISTING approval gate — it never auto-grants on create.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! - **openclaw** `src/agents/tools/common.ts` (`normalizeToolModelOverride`, L130-139):
//!   a backend/model preference is an OVERRIDE — trimmed, with empty/`"default"` meaning
//!   "no override". `src/agents/tools/sessions-spawn-tool.ts` (L295-298) reads `runtime`
//!   from a constrained set (`"acp"` vs `"subagent"`, defaulting safely) and `model` as
//!   that normalized optional override. We mirror exactly: a named preference is honored
//!   ONLY when it resolves to a backend that actually exists; an unknown/empty preference
//!   leaves the deterministic default standing — we never invent an adapter.
//! - **openclaw** `resolveSubagentTargetFromRuns` shape (exact match against the KNOWN
//!   set, never resolving to something absent) — already adapted in
//!   [`crate::prime::resolve_assignee`]; here the same fail-closed discipline gates the
//!   adapter id against the live installed-adapter roster.
//!
//! ## The contract (binding)
//!
//! Pure functions over the message + live state. They DECIDE nothing risky: a resolved
//! adapter is only ever one already installed; a requested capability is only ever
//! reported (never granted). The caller ([`crate::prime::decide`]) builds an honest
//! reply from the result and stages any grant through the unchanged, approval-gated
//! permission path.

use relux_core::{CLAUDE_CLI_ADAPTER_ID, CODEX_CLI_ADAPTER_ID, LOCAL_PRIME_ADAPTER_ID};

/// The outcome of resolving an explicitly-named adapter/brain preference against the
/// live installed-adapter roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterPreference {
    /// No adapter/brain was named — keep the deterministic default (local-prime).
    None,
    /// A preference was named AND resolves to an installed adapter plugin. `label` is the
    /// human brand ("Claude", "Codex", "the local adapter") for the reply.
    Resolved { adapter_id: String, label: String },
    /// A preference was named (e.g. "Claude") but its adapter plugin is NOT installed.
    /// The caller must keep the default and say setup is needed — never fabricate it.
    NamedButUnavailable { adapter_id: String, label: String },
}

/// A capability the user explicitly asked the new agent to have. Detected so Prime is
/// HONEST (`docs/RELUX_MASTER_PLAN.md` §6): it names the scoped permission it would need
/// and routes the grant through the approval gate. NEVER auto-granted on create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedCapability {
    /// The human brand for the reply/button ("GitHub", "the terminal").
    pub label: String,
    /// The least-privilege permission string the grant follow-up would request. Matches
    /// [`crate::prime::derive_permission_label`] so the staged grant classifies + resolves
    /// through the unchanged `PermissionChange` path.
    pub permission: String,
    /// The tool plugin id that must be installed/configured before the permission is
    /// usable — surfaced in the honest reply (§6 "I need … GitHub tool plugin").
    pub tool_plugin: String,
}

/// Canonical id + human label for a recognized brand word.
fn brand_to_adapter(word: &str) -> Option<(&'static str, &'static str)> {
    match word {
        "claude" => Some((CLAUDE_CLI_ADAPTER_ID, "Claude")),
        "codex" => Some((CODEX_CLI_ADAPTER_ID, "Codex")),
        _ => None,
    }
}

/// Resolve an explicitly-named adapter/brain preference in an agent-creation `message`
/// against the live `available_adapter_ids` (the installed adapter plugin ids).
///
/// Recognizes a brand word (`claude` / `codex`), the explicit "local" adapter phrasing,
/// and a verbatim adapter plugin id. A recognized preference resolves only when its
/// adapter plugin is installed; otherwise it is reported as
/// [`AdapterPreference::NamedButUnavailable`] so the caller can be honest and fall back
/// to the default — it never invents/enables an adapter.
pub fn resolve_adapter_preference(
    message: &str,
    available_adapter_ids: &[String],
) -> AdapterPreference {
    let m = message.to_lowercase();
    let installed = |id: &str| available_adapter_ids.iter().any(|a| a.eq_ignore_ascii_case(id));

    // 1. A verbatim adapter plugin id wins (the user named the exact backend).
    for id in available_adapter_ids {
        if m.contains(&id.to_lowercase()) {
            let label = adapter_label(id);
            return AdapterPreference::Resolved {
                adapter_id: id.clone(),
                label,
            };
        }
    }

    // 2. The explicit "local" adapter phrasing → local-prime (only on a clear phrase, so
    //    the bare word "local" in prose can never flip the adapter).
    if m.contains("local prime")
        || m.contains("local-prime")
        || m.contains("local adapter")
        || m.contains("the local")
    {
        let id = LOCAL_PRIME_ADAPTER_ID.to_string();
        return if installed(LOCAL_PRIME_ADAPTER_ID) {
            AdapterPreference::Resolved {
                adapter_id: id,
                label: "the local adapter".to_string(),
            }
        } else {
            AdapterPreference::NamedButUnavailable {
                adapter_id: id,
                label: "the local adapter".to_string(),
            }
        };
    }

    // 3. A brand word matched as a WHOLE token (so "codex" matches but "coding" does not).
    for word in m.split(|c: char| !c.is_ascii_alphanumeric()) {
        if let Some((id, label)) = brand_to_adapter(word) {
            return if installed(id) {
                AdapterPreference::Resolved {
                    adapter_id: id.to_string(),
                    label: label.to_string(),
                }
            } else {
                AdapterPreference::NamedButUnavailable {
                    adapter_id: id.to_string(),
                    label: label.to_string(),
                }
            };
        }
    }

    AdapterPreference::None
}

/// A friendly label for a known adapter plugin id, else the id itself.
fn adapter_label(id: &str) -> String {
    match id {
        CLAUDE_CLI_ADAPTER_ID => "Claude".to_string(),
        CODEX_CLI_ADAPTER_ID => "Codex".to_string(),
        LOCAL_PRIME_ADAPTER_ID => "the local adapter".to_string(),
        other => other.to_string(),
    }
}

/// Detect the capabilities the user asked a freshly-created agent to have. Conservative
/// and deterministic — it only recognizes the two governed tool families Relux ships a
/// scoped permission label for ([`crate::prime::derive_permission_label`]); anything else
/// is left to the conversational reply. Returns at most one entry per family, in a
/// stable order, and never duplicates.
pub fn requested_capabilities(message: &str) -> Vec<RequestedCapability> {
    let m = message.to_lowercase();
    let mut out: Vec<RequestedCapability> = Vec::new();

    let github = m.contains("github")
        || m.contains("open pr")
        || m.contains("open a pr")
        || m.contains("pull request")
        || m.contains("repo access");
    if github {
        out.push(RequestedCapability {
            label: "GitHub".to_string(),
            permission: "tool:relux-tools-github:access".to_string(),
            tool_plugin: "relux-tools-github".to_string(),
        });
    }

    let terminal = m.contains("terminal")
        || m.contains("shell")
        || m.contains("command line")
        || m.contains("run commands")
        || m.contains("run shell");
    if terminal {
        out.push(RequestedCapability {
            label: "the terminal".to_string(),
            permission: "tool:relux-tools-terminal:access".to_string(),
            tool_plugin: "relux-tools-terminal".to_string(),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_preference_when_no_brand_named() {
        assert_eq!(
            resolve_adapter_preference("hire a research agent", &ids(&[LOCAL_PRIME_ADAPTER_ID])),
            AdapterPreference::None
        );
        // "coding" must not be read as "codex".
        assert_eq!(
            resolve_adapter_preference("make a coding agent for this repo", &ids(&[])),
            AdapterPreference::None
        );
    }

    #[test]
    fn resolves_a_named_brand_when_its_adapter_is_installed() {
        let avail = ids(&[LOCAL_PRIME_ADAPTER_ID, CLAUDE_CLI_ADAPTER_ID]);
        assert_eq!(
            resolve_adapter_preference("create an agent named researcher that uses Claude", &avail),
            AdapterPreference::Resolved {
                adapter_id: CLAUDE_CLI_ADAPTER_ID.to_string(),
                label: "Claude".to_string(),
            }
        );
    }

    #[test]
    fn reports_named_but_unavailable_when_the_adapter_is_not_installed() {
        // Claude was named, but only the local adapter is installed: never fabricate it.
        let avail = ids(&[LOCAL_PRIME_ADAPTER_ID]);
        assert_eq!(
            resolve_adapter_preference("an agent that uses claude", &avail),
            AdapterPreference::NamedButUnavailable {
                adapter_id: CLAUDE_CLI_ADAPTER_ID.to_string(),
                label: "Claude".to_string(),
            }
        );
    }

    #[test]
    fn resolves_codex_as_a_whole_word() {
        let avail = ids(&[CODEX_CLI_ADAPTER_ID]);
        assert_eq!(
            resolve_adapter_preference("run codex on this one", &avail),
            AdapterPreference::Resolved {
                adapter_id: CODEX_CLI_ADAPTER_ID.to_string(),
                label: "Codex".to_string(),
            }
        );
    }

    #[test]
    fn a_verbatim_adapter_id_is_honored() {
        let avail = ids(&[CLAUDE_CLI_ADAPTER_ID]);
        assert_eq!(
            resolve_adapter_preference(
                "create an agent on relux-adapter-claude-cli",
                &avail
            ),
            AdapterPreference::Resolved {
                adapter_id: CLAUDE_CLI_ADAPTER_ID.to_string(),
                label: "Claude".to_string(),
            }
        );
    }

    #[test]
    fn detects_github_capability_request() {
        let caps = requested_capabilities("create an agent that can read GitHub");
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].label, "GitHub");
        assert_eq!(caps[0].permission, "tool:relux-tools-github:access");
        assert_eq!(caps[0].tool_plugin, "relux-tools-github");
    }

    #[test]
    fn detects_terminal_and_github_together_in_stable_order() {
        let caps =
            requested_capabilities("a coding agent that can open PRs and run shell commands");
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].label, "GitHub");
        assert_eq!(caps[1].label, "the terminal");
    }

    #[test]
    fn no_capabilities_when_none_requested() {
        assert!(requested_capabilities("hire a research agent").is_empty());
    }

    #[test]
    fn permission_label_matches_the_grant_path_label() {
        // The detected permission MUST equal what `derive_permission_label` produces for
        // the same word, so the staged "grant <permission> to <agent>" follow-up
        // classifies + resolves through the unchanged PermissionChange path.
        let caps = requested_capabilities("can read github");
        assert_eq!(
            caps[0].permission,
            crate::prime::derive_permission_label("grant github access")
        );
    }
}
