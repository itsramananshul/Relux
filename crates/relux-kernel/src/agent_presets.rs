//! Curated **role-preset bundles** for the manual Crew (agent) create workflow.
//!
//! Section: `docs/relix-dashboard-design.md` §9.1 (the create/edit Configuration tab —
//! "the role-preset bundles … remain future work"). A preset is an OPERATOR
//! CONVENIENCE: it pre-fills the role/persona/skills of a new crew member so the
//! common crew types (researcher, builder, reviewer, planner, operator) don't have to
//! be typed from scratch each time.
//!
//! ## The safety shape (NON-NEGOTIABLE)
//!
//! A preset **SUGGESTS configuration only — it never grants a permission or a
//! capability, and never picks a runtime/adapter.** Structurally, [`AgentPreset`]
//! carries only the same three fields an operator could type by hand (role, persona,
//! skills); it has no permission/adapter field, so a preset literally *cannot* widen
//! an agent's power. Preset-expanded fields flow through the existing
//! [`crate::agent_config::validate_new_agent`] validators (no duplicate validation),
//! and `create_agent` still grants only the minimal echo tool regardless of preset.
//! Elevated grants stay on the deliberate, audited Governance path.
//!
//! Reference-driven (`docs/reference-driven-development.md`):
//! - openclaw `src/agents/tools/sessions-spawn-tool.ts` — when a spawn request names a
//!   role it becomes a pure context label (`roleContext = requestedAgentId ? { role:
//!   requestedAgentId } : {}`, L323); the role NEVER expands the worker's tools — the
//!   inherited tool allow/deny list governs capability *separately*. We mirror that
//!   split exactly: a preset shapes the agent's *description*, it never touches the
//!   permission grant. The same file's default-the-enum pattern (`params.cleanup ===
//!   "keep" | "delete" ? … : "keep"`, L301) → an unknown preset id is rejected, a known
//!   one expands to a fixed bundle.
//! - openclaw `src/acp/approval-classifier.ts` `normalizeToolName` (lowercase + strict
//!   id shape) → [`find_agent_preset`] resolves the id case-insensitively against a
//!   fixed allowlist; an off-list id resolves to nothing (the caller 400s).
//! - Hermes `agent/system_prompt.py` — a persona/role steers the *system prompt* only;
//!   the toolset (capability) is configured on a separate axis. The preset persona is
//!   the same kind of advisory operating-style text (bounded + secret-redacted by the
//!   shared `agent_config` sanitizers when it is applied).

/// One curated role preset. Deliberately holds ONLY the advisory fields an operator
/// could type manually — there is no permission or adapter field, so a preset can
/// never grant a capability or pick a runtime.
#[derive(Debug, Clone, Copy)]
pub struct AgentPreset {
    /// Strict-slug allowlist key (also the value sent over the wire).
    pub id: &'static str,
    /// Operator-facing label for the selector.
    pub label: &'static str,
    /// One line describing what this crew type is for (shown under the selector).
    pub summary: &'static str,
    /// Default Role / Description.
    pub role: &'static str,
    /// Default persona (operating style). Bounded + secret-redacted by the
    /// `agent_config` sanitizers when applied — kept well under the persona clamp here.
    pub persona: &'static str,
    /// Suggested specialty slugs (already valid `[a-z0-9-]` slugs, well under the
    /// per-skill and per-list bounds; re-validated by `validate_skills` on apply).
    pub skills: &'static [&'static str],
}

/// The curated preset list. Small and opinionated on purpose: these are the common
/// crew types an operator reaches for, each a safe suggestion bundle. None grants any
/// permission. Keep the ids unique, lowercase, strict slugs.
pub const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: "researcher",
        label: "Researcher",
        summary: "Investigates questions and gathers cited sources.",
        role: "Investigates questions and gathers sources.",
        persona: "Methodical and thorough; gathers several sources, cites them, and \
                  flags uncertainty instead of guessing. Asks one clarifying question \
                  when the goal is ambiguous rather than assuming.",
        skills: &["research", "analysis", "writing"],
    },
    AgentPreset {
        id: "builder",
        label: "Builder / Coder",
        summary: "Implements features and writes code to a requirement.",
        role: "Implements features and writes code.",
        persona: "Pragmatic and precise; makes the smallest change that satisfies the \
                  requirement, matches the surrounding code, and verifies with tests \
                  before claiming the work is done.",
        skills: &["coding", "testing", "debugging"],
    },
    AgentPreset {
        id: "reviewer",
        label: "Reviewer",
        summary: "Reviews work for correctness and quality.",
        role: "Reviews work for correctness and quality.",
        persona: "Skeptical and constructive; looks for correctness bugs and \
                  simplifications, verifies each claim against the code, and explains \
                  the reasoning behind every finding.",
        skills: &["review", "testing", "quality"],
    },
    AgentPreset {
        id: "planner",
        label: "Planner",
        summary: "Breaks a goal into a sequenced, verifiable plan.",
        role: "Breaks goals into a sequenced plan.",
        persona: "Structured and outcome-focused; decomposes a goal into ordered, \
                  verifiable steps, surfaces risks and dependencies, and proposes the \
                  plan for review before acting.",
        skills: &["planning", "strategy", "analysis"],
    },
    AgentPreset {
        id: "operator",
        label: "Operator / Support",
        summary: "Handles routine operations and support tasks.",
        role: "Handles routine operations and support tasks.",
        persona: "Calm and dependable; follows the runbook, communicates status \
                  clearly, and escalates anything risky or ambiguous instead of \
                  improvising.",
        skills: &["support", "operations", "communication"],
    },
];

/// Resolve a preset id (case-insensitively, trimmed) against the fixed allowlist.
/// Returns `None` for an unknown id — the caller surfaces an honest 400, never an
/// invented bundle. Mirrors openclaw's strict-id resolution discipline.
pub fn find_agent_preset(id: &str) -> Option<&'static AgentPreset> {
    let key = id.trim().to_lowercase();
    AGENT_PRESETS.iter().find(|p| p.id == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_config::{validate_new_agent, CreateAgentInput, MAX_AGENT_PERSONA_CHARS};

    fn adapters() -> Vec<String> {
        vec!["relux-adapter-local-prime".to_string()]
    }

    #[test]
    fn preset_ids_are_unique_strict_slugs() {
        let mut seen = std::collections::HashSet::new();
        for p in AGENT_PRESETS {
            assert!(seen.insert(p.id), "duplicate preset id '{}'", p.id);
            // Strict slug: lowercase [a-z0-9-], no leading/trailing hyphen.
            assert_eq!(p.id, p.id.to_lowercase(), "id not lowercase: {}", p.id);
            assert!(
                p.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "id not a strict slug: {}",
                p.id
            );
            assert!(!p.id.starts_with('-') && !p.id.ends_with('-'), "id edge hyphen: {}", p.id);
            assert!(!p.label.trim().is_empty(), "empty label for {}", p.id);
            assert!(!p.summary.trim().is_empty(), "empty summary for {}", p.id);
        }
    }

    #[test]
    fn find_is_case_insensitive_and_fails_closed() {
        assert!(find_agent_preset("researcher").is_some());
        assert!(find_agent_preset("  Researcher  ").is_some());
        assert!(find_agent_preset("RESEARCHER").is_some());
        // An unknown id resolves to nothing (the caller 400s).
        assert!(find_agent_preset("evil-overlord").is_none());
        assert!(find_agent_preset("").is_none());
    }

    #[test]
    fn every_preset_expands_through_the_existing_validators() {
        // The core guarantee: each curated preset, expanded into a create request,
        // passes the SAME agent_config validators the manual path uses — no preset
        // can produce an invalid role/persona/skills, and no duplicate validation.
        for (i, p) in AGENT_PRESETS.iter().enumerate() {
            let skills: Vec<String> = p.skills.iter().map(|s| s.to_string()).collect();
            let name = format!("Preset {i}");
            let resolved = validate_new_agent(
                CreateAgentInput {
                    id: None,
                    name: &name,
                    role: Some(p.role),
                    persona: Some(p.persona),
                    adapter_plugin: None,
                    skills: Some(&skills),
                },
                &adapters(),
                &[],
                &[],
            )
            .unwrap_or_else(|e| panic!("preset '{}' failed validation: {e}", p.id));

            // Skills are already valid slugs → they survive validation unchanged.
            assert_eq!(resolved.skills, skills, "preset '{}' skills changed", p.id);
            // The persona is kept (non-empty) and within the shared bound.
            let persona = resolved.persona.expect("preset persona kept");
            assert!(!persona.is_empty(), "preset '{}' persona emptied", p.id);
            assert!(
                persona.chars().count() <= MAX_AGENT_PERSONA_CHARS,
                "preset '{}' persona over bound",
                p.id
            );
            // The preset defaults the SAFE local-Prime adapter (it picks no runtime).
            assert_eq!(resolved.adapter_plugin, "relux-adapter-local-prime");
        }
    }

    #[test]
    fn a_preset_carries_no_capability_field() {
        // Structural safety: the type exposes only advisory fields. This test documents
        // the invariant — if a permission/adapter field is ever added to AgentPreset,
        // this comment (and the no-auto-grant contract) must be revisited deliberately.
        // (There is nothing to grant from here; create_agent's fixed echo-only grant is
        // asserted by the server-side `agent_create_with_preset_*` tests.)
        let p = find_agent_preset("builder").unwrap();
        assert!(!p.role.is_empty());
        assert!(!p.persona.is_empty());
        assert!(!p.skills.is_empty());
    }
}
