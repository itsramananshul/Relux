//! Manual Crew (agent) create/edit configuration: validation, sanitization, and
//! clamping for the operator-facing agent-config workflow.
//!
//! Section: `docs/relix-dashboard-design.md` (Crew page) + `docs/relix-company-model.md`
//! (Operatives — operators must be able to configure crew directly).
//!
//! Reference-driven (`docs/reference-driven-development.md`). This mirrors the same
//! validation discipline the brain-assisted agent-slot path already adopted, applied
//! to the *manual* HTTP path so the two surfaces agree on what a valid agent config is:
//! - openclaw `sessions-spawn-tool.ts` (`UNSUPPORTED_SESSIONS_SPAWN_PARAM_KEYS` rejected
//!   before any param is read; `readStringParam(..., { required: true })`; default the
//!   rest) → require the mandatory `name`, default/clamp the optional fields.
//! - openclaw `approval-classifier.ts` `normalizeToolName` (lowercase + length-bounded +
//!   strict `^[a-z0-9._-]+$`) → [`normalize_agent_id`] reduces a display name to a strict
//!   id shape; the adapter must resolve to an EXISTING roster id (never invented).
//! - Hermes `message_sanitization.py` (`_escape_invalid_chars_in_json_strings`,
//!   `_sanitize_tool_error` length clamp) → control-char strip + length clamp on every
//!   operator-supplied string; the persona is additionally run through
//!   [`relux_core::redact::redact_secrets`] so a pasted credential is never stored verbatim.
//!
//! All functions here are pure (no kernel/state access) so they are unit-testable: the
//! HTTP handlers gather the live rosters (known adapters, existing ids/names) and hand
//! them in, then apply the result through the kernel under its lock.

use relux_core::agent::AgentStatus;
use relux_core::redact::redact_secrets;

/// Display name clamp (single line).
pub const MAX_AGENT_NAME_CHARS: usize = 64;
/// Agent id clamp (strict `[a-z0-9-]` shape).
pub const MAX_AGENT_ID_CHARS: usize = 64;
/// Role/description clamp (single line).
pub const MAX_AGENT_DESC_CHARS: usize = 240;
/// Persona clamp (multi-line operating style). Matches the brain-assisted
/// `prime_agent_slots::MAX_PERSONA_CHARS` so manual and seeded personas share one bound.
pub const MAX_AGENT_PERSONA_CHARS: usize = 600;

/// Per-skill slug length clamp. A specialty tag is a short word/slug, not prose.
pub const MAX_SKILL_CHARS: usize = 32;
/// Maximum number of skills/tags on one agent. Bounds the list so a pasted blob cannot
/// balloon the record or the assignment-matching candidate set.
pub const MAX_SKILLS: usize = 16;

/// The safe default adapter when an operator does not pick one: the local
/// deterministic Prime adapter (always usable, no CLI spawn).
pub const DEFAULT_ADAPTER_PLUGIN: &str = "relux-adapter-local-prime";

/// Honest, operator-facing validation failures for the manual agent-config path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentConfigError {
    /// `name` was missing or sanitized to empty.
    NameRequired,
    /// The id (provided or derived from the name) normalized to empty.
    IdInvalid,
    /// An agent with this id already exists.
    DuplicateId(String),
    /// An agent with this display name already exists (case-insensitive).
    DuplicateName(String),
    /// The chosen adapter is not one of the known/installed adapter plugins.
    UnknownAdapter(String),
    /// The requested status is not an operator-settable status.
    InvalidStatus(String),
    /// A provided skill/tag contained nothing that sanitizes to a valid slug.
    InvalidSkill(String),
    /// More than [`MAX_SKILLS`] distinct skills/tags were supplied.
    TooManySkills(usize),
}

impl AgentConfigError {
    /// A short, honest message safe to surface to the operator.
    pub fn message(&self) -> String {
        match self {
            AgentConfigError::NameRequired => "name is required".to_string(),
            AgentConfigError::IdInvalid => {
                "id must contain at least one letter or digit".to_string()
            }
            AgentConfigError::DuplicateId(id) => format!("an agent with id '{id}' already exists"),
            AgentConfigError::DuplicateName(name) => {
                format!("an agent named '{name}' already exists")
            }
            AgentConfigError::UnknownAdapter(a) => {
                format!("unknown adapter '{a}'; choose one of the installed adapters")
            }
            AgentConfigError::InvalidStatus(s) => {
                format!("invalid status '{s}'; use active, paused, or disabled")
            }
            AgentConfigError::InvalidSkill(s) => {
                format!("invalid skill '{s}'; use short words or slugs (letters, digits, hyphens)")
            }
            AgentConfigError::TooManySkills(n) => {
                format!("too many skills ({n}); at most {MAX_SKILLS} are allowed")
            }
        }
    }
}

impl std::fmt::Display for AgentConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

/// Raw operator input for creating an agent.
pub struct CreateAgentInput<'a> {
    pub id: Option<&'a str>,
    pub name: &'a str,
    pub role: Option<&'a str>,
    pub persona: Option<&'a str>,
    pub adapter_plugin: Option<&'a str>,
    /// Specialty tags/skills. Absent => no skills; present => validated to a bounded
    /// slug list (each invalid entry is rejected with a clear error).
    pub skills: Option<&'a [String]>,
}

/// A validated, ready-to-apply new agent config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNewAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub persona: Option<String>,
    pub adapter_plugin: String,
    pub skills: Vec<String>,
}

/// Raw operator input for editing an agent. A field left `None` means "leave
/// unchanged"; a present field is validated and applied.
pub struct UpdateAgentInput<'a> {
    pub name: Option<&'a str>,
    pub role: Option<&'a str>,
    /// Present => set the persona; an empty/whitespace value CLEARS it.
    pub persona: Option<&'a str>,
    pub adapter_plugin: Option<&'a str>,
    pub status: Option<&'a str>,
    /// Present => REPLACE the whole skill list (an empty list clears all skills);
    /// absent => leave the current skills unchanged.
    pub skills: Option<&'a [String]>,
}

/// A validated, ready-to-apply agent edit. Outer `None` = unchanged; for persona,
/// `Some(None)` = clear, `Some(Some(_))` = set. For skills, `Some(list)` replaces the
/// whole list (an empty list clears it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedAgentUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub persona: Option<Option<String>>,
    pub adapter_plugin: Option<String>,
    pub status: Option<AgentStatus>,
    pub skills: Option<Vec<String>>,
}

/// Normalize a display name (or an explicitly-typed id) into a strict agent id:
/// lowercase, keep only `[a-z0-9-]` (separators collapse to a single hyphen), trim
/// hyphens, clamp. Mirrors openclaw's `normalizeToolName` strict-id discipline and the
/// kernel's existing `name.to_lowercase().replace(' ', "-")` derivation.
pub fn normalize_agent_id(raw: &str) -> String {
    let lowered = raw.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_hyphen = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_hyphen = false;
        } else if c == '-' || c == '_' || c == '.' || c.is_whitespace() {
            if last_hyphen {
                continue;
            }
            last_hyphen = true;
            out.push('-');
        }
        // Drop anything else.
        if out.chars().count() >= MAX_AGENT_ID_CHARS {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Sanitize a single-line string: control chars → space, collapse whitespace, trim,
/// clamp. Shared shape with `prime_agent_slots::sanitize_line`.
fn sanitize_line(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect::<String>().trim().to_string()
}

/// Sanitize a multi-line block: drop control chars except `\n`, collapse intra-line
/// whitespace, drop blank lines, trim, clamp. Shared shape with
/// `prime_agent_slots::sanitize_block`.
fn sanitize_block(s: &str, max: usize) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|line| {
            let cleaned: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect();
    let joined = lines.join("\n");
    let truncated: String = joined.chars().take(max).collect();
    truncated.trim().to_string()
}

/// Sanitize a persona: block-sanitize, REDACT obvious secrets (so a pasted credential
/// is never persisted verbatim), then clamp again (redaction never grows the visible
/// content beyond the bound). Returns an empty string when nothing meaningful remains.
pub fn sanitize_persona(raw: &str) -> String {
    let block = sanitize_block(raw, MAX_AGENT_PERSONA_CHARS);
    let redacted = redact_secrets(&block);
    redacted.chars().take(MAX_AGENT_PERSONA_CHARS).collect::<String>().trim().to_string()
}

/// Reduce one raw skill/tag to a strict slug: lowercase, keep only `[a-z0-9-]`
/// (separators collapse to a single hyphen), trim hyphens, clamp to [`MAX_SKILL_CHARS`].
/// Returns `None` when nothing valid remains (e.g. an emoji-only or control-only input),
/// which the caller surfaces as an honest [`AgentConfigError::InvalidSkill`]. Mirrors
/// [`normalize_agent_id`]'s strict-id discipline (openclaw `normalizeToolName`).
pub fn sanitize_skill(raw: &str) -> Option<String> {
    let slug = normalize_agent_id(raw); // same `[a-z0-9-]` slug shape, already clamped/trimmed
    let clamped: String = slug.chars().take(MAX_SKILL_CHARS).collect();
    let trimmed = clamped.trim_matches('-').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Validate a raw skills list into a bounded, deduped slug list. Each entry must
/// sanitize to a non-empty slug (else [`AgentConfigError::InvalidSkill`]); duplicates are
/// dropped (first occurrence wins); more than [`MAX_SKILLS`] distinct skills is rejected
/// ([`AgentConfigError::TooManySkills`]). An empty input yields an empty list (= no skills).
pub fn validate_skills(raw: &[String]) -> Result<Vec<String>, AgentConfigError> {
    let mut out: Vec<String> = Vec::new();
    for entry in raw {
        // Skip blank-only entries silently (a trailing comma in the UI), but reject an
        // entry that had real content yet sanitized to nothing (e.g. "💥").
        if entry.trim().is_empty() {
            continue;
        }
        let slug = sanitize_skill(entry)
            .ok_or_else(|| AgentConfigError::InvalidSkill(entry.trim().to_string()))?;
        if !out.contains(&slug) {
            out.push(slug);
        }
    }
    if out.len() > MAX_SKILLS {
        return Err(AgentConfigError::TooManySkills(out.len()));
    }
    Ok(out)
}

/// Case-insensitive membership against a roster of ids/names.
fn contains_ci(haystack: &[String], needle: &str) -> bool {
    let lowered = needle.trim().to_lowercase();
    haystack.iter().any(|h| h.trim().to_lowercase() == lowered)
}

/// Resolve a requested adapter against the live roster, preserving the canonical
/// roster id (exact case). Returns `Err(UnknownAdapter)` when it is not installed.
fn resolve_adapter(requested: &str, known_adapters: &[String]) -> Result<String, AgentConfigError> {
    let lowered = requested.trim().to_lowercase();
    known_adapters
        .iter()
        .find(|a| a.trim().to_lowercase() == lowered)
        .cloned()
        .ok_or_else(|| AgentConfigError::UnknownAdapter(requested.trim().to_string()))
}

/// Map an operator-supplied status string onto the allowlist of statuses an operator
/// may set. Machine-driven statuses (`Error`) and unknown values are rejected.
fn resolve_status(raw: &str) -> Result<AgentStatus, AgentConfigError> {
    match raw.trim().to_lowercase().as_str() {
        "active" | "enabled" | "enable" => Ok(AgentStatus::Active),
        "paused" | "pause" => Ok(AgentStatus::Paused),
        "disabled" | "disable" => Ok(AgentStatus::Disabled),
        other => Err(AgentConfigError::InvalidStatus(other.to_string())),
    }
}

/// Validate and resolve a new-agent request against the live rosters.
///
/// `known_adapters` = installed adapter plugin ids; `existing_ids` / `existing_names`
/// = the current roster (for the uniqueness checks). An adapter is honored only when it
/// resolves to a roster id; an absent adapter defaults to the local Prime adapter.
pub fn validate_new_agent(
    input: CreateAgentInput<'_>,
    known_adapters: &[String],
    existing_ids: &[String],
    existing_names: &[String],
) -> Result<ResolvedNewAgent, AgentConfigError> {
    let name = sanitize_line(input.name, MAX_AGENT_NAME_CHARS);
    if name.is_empty() {
        return Err(AgentConfigError::NameRequired);
    }

    let id = match input.id {
        Some(raw) if !raw.trim().is_empty() => normalize_agent_id(raw),
        _ => normalize_agent_id(&name),
    };
    if id.is_empty() {
        return Err(AgentConfigError::IdInvalid);
    }
    if contains_ci(existing_ids, &id) {
        return Err(AgentConfigError::DuplicateId(id));
    }
    if contains_ci(existing_names, &name) {
        return Err(AgentConfigError::DuplicateName(name));
    }

    let description = input
        .role
        .map(|r| sanitize_line(r, MAX_AGENT_DESC_CHARS))
        .unwrap_or_default();

    let persona = input
        .persona
        .map(sanitize_persona)
        .filter(|p| !p.is_empty());

    let adapter_plugin = match input.adapter_plugin {
        Some(raw) if !raw.trim().is_empty() => resolve_adapter(raw, known_adapters)?,
        _ => DEFAULT_ADAPTER_PLUGIN.to_string(),
    };

    let skills = match input.skills {
        Some(raw) => validate_skills(raw)?,
        None => Vec::new(),
    };

    Ok(ResolvedNewAgent {
        id,
        name,
        description,
        persona,
        adapter_plugin,
        skills,
    })
}

/// Validate and resolve an agent-edit request. `existing_names_except_self` excludes
/// the agent being edited so renaming to its own name is not a false duplicate.
pub fn validate_agent_update(
    input: UpdateAgentInput<'_>,
    known_adapters: &[String],
    existing_names_except_self: &[String],
) -> Result<ResolvedAgentUpdate, AgentConfigError> {
    let mut resolved = ResolvedAgentUpdate::default();

    if let Some(raw) = input.name {
        let name = sanitize_line(raw, MAX_AGENT_NAME_CHARS);
        if name.is_empty() {
            return Err(AgentConfigError::NameRequired);
        }
        if contains_ci(existing_names_except_self, &name) {
            return Err(AgentConfigError::DuplicateName(name));
        }
        resolved.name = Some(name);
    }

    if let Some(raw) = input.role {
        // An empty role is a deliberate clear-to-blank, not an error.
        resolved.description = Some(sanitize_line(raw, MAX_AGENT_DESC_CHARS));
    }

    if let Some(raw) = input.persona {
        let persona = sanitize_persona(raw);
        resolved.persona = Some(if persona.is_empty() { None } else { Some(persona) });
    }

    if let Some(raw) = input.adapter_plugin {
        if !raw.trim().is_empty() {
            resolved.adapter_plugin = Some(resolve_adapter(raw, known_adapters)?);
        }
    }

    if let Some(raw) = input.status {
        if !raw.trim().is_empty() {
            resolved.status = Some(resolve_status(raw)?);
        }
    }

    // Present => replace the whole skill list (an empty list clears it).
    if let Some(raw) = input.skills {
        resolved.skills = Some(validate_skills(raw)?);
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapters() -> Vec<String> {
        vec![
            "relux-adapter-local-prime".to_string(),
            "relux-adapter-claude-cli".to_string(),
        ]
    }

    #[test]
    fn normalize_agent_id_strict_shape() {
        assert_eq!(normalize_agent_id("CI Watcher"), "ci-watcher");
        assert_eq!(normalize_agent_id("Code  Robot!!"), "code-robot");
        assert_eq!(normalize_agent_id("  --weird__id.. "), "weird-id");
        assert_eq!(normalize_agent_id("!!!"), "");
    }

    #[test]
    fn create_requires_name() {
        let err = validate_new_agent(
            CreateAgentInput { id: None, name: "   ", role: None, persona: None, adapter_plugin: None, skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err, AgentConfigError::NameRequired);
    }

    #[test]
    fn create_derives_id_and_defaults_adapter() {
        let ok = validate_new_agent(
            CreateAgentInput { id: None, name: "Research Bot", role: Some("does research"), persona: None, adapter_plugin: None, skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(ok.id, "research-bot");
        assert_eq!(ok.name, "Research Bot");
        assert_eq!(ok.description, "does research");
        assert_eq!(ok.adapter_plugin, DEFAULT_ADAPTER_PLUGIN);
        assert!(ok.persona.is_none());
    }

    #[test]
    fn create_rejects_duplicate_id_and_name() {
        let dup_id = validate_new_agent(
            CreateAgentInput { id: Some("research-bot"), name: "Other", role: None, persona: None, adapter_plugin: None, skills: None },
            &adapters(),
            &["research-bot".to_string()],
            &[],
        )
        .unwrap_err();
        assert_eq!(dup_id, AgentConfigError::DuplicateId("research-bot".to_string()));

        let dup_name = validate_new_agent(
            CreateAgentInput { id: None, name: "Research Bot", role: None, persona: None, adapter_plugin: None, skills: None },
            &adapters(),
            &[],
            &["research bot".to_string()],
        )
        .unwrap_err();
        assert_eq!(dup_name, AgentConfigError::DuplicateName("Research Bot".to_string()));
    }

    #[test]
    fn create_rejects_unknown_adapter() {
        let err = validate_new_agent(
            CreateAgentInput { id: None, name: "Bot", role: None, persona: None, adapter_plugin: Some("relux-adapter-evil"), skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err, AgentConfigError::UnknownAdapter("relux-adapter-evil".to_string()));
    }

    #[test]
    fn create_resolves_adapter_canonical_case() {
        let ok = validate_new_agent(
            CreateAgentInput { id: None, name: "Bot", role: None, persona: None, adapter_plugin: Some("RELUX-Adapter-Claude-CLI"), skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(ok.adapter_plugin, "relux-adapter-claude-cli");
    }

    #[test]
    fn persona_is_bounded_and_redacted() {
        // Overlong personas are clamped, not rejected.
        let long = "word ".repeat(400);
        let ok = validate_new_agent(
            CreateAgentInput { id: None, name: "Bot", role: None, persona: Some(&long), adapter_plugin: None, skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap();
        let persona = ok.persona.expect("persona kept");
        assert!(persona.chars().count() <= MAX_AGENT_PERSONA_CHARS, "persona clamped");

        // A pasted credential is redacted before storage.
        let secret = format!("{}{}", "sk-ant-", "0123456789abcdef0123456789");
        let with_secret = format!("Use my key {secret} when you run");
        let ok = validate_new_agent(
            CreateAgentInput { id: None, name: "Bot2", role: None, persona: Some(&with_secret), adapter_plugin: None, skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap();
        let persona = ok.persona.expect("persona kept");
        assert!(!persona.contains(&secret), "secret leaked into persona: {persona}");
        assert!(persona.contains("***REDACTED***"));
    }

    #[test]
    fn update_leaves_absent_fields_unchanged() {
        let resolved = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: None, adapter_plugin: None, status: None, skills: None },
            &adapters(),
            &[],
        )
        .unwrap();
        assert_eq!(resolved, ResolvedAgentUpdate::default());
    }

    #[test]
    fn update_clears_persona_on_empty() {
        let resolved = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: Some("   "), adapter_plugin: None, status: None, skills: None },
            &adapters(),
            &[],
        )
        .unwrap();
        assert_eq!(resolved.persona, Some(None));
    }

    #[test]
    fn update_validates_status_and_adapter() {
        let ok = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: None, adapter_plugin: Some("relux-adapter-claude-cli"), status: Some("Disabled"), skills: None },
            &adapters(),
            &[],
        )
        .unwrap();
        assert_eq!(ok.adapter_plugin.as_deref(), Some("relux-adapter-claude-cli"));
        assert_eq!(ok.status, Some(AgentStatus::Disabled));

        let bad_status = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: None, adapter_plugin: None, status: Some("error"), skills: None },
            &adapters(),
            &[],
        )
        .unwrap_err();
        assert_eq!(bad_status, AgentConfigError::InvalidStatus("error".to_string()));
    }

    #[test]
    fn update_rejects_duplicate_name_but_allows_self() {
        let dup = validate_agent_update(
            UpdateAgentInput { name: Some("Taken"), role: None, persona: None, adapter_plugin: None, status: None, skills: None },
            &adapters(),
            &["taken".to_string()],
        )
        .unwrap_err();
        assert_eq!(dup, AgentConfigError::DuplicateName("Taken".to_string()));

        // Renaming to a name not held by anyone else is fine.
        let ok = validate_agent_update(
            UpdateAgentInput { name: Some("Fresh"), role: None, persona: None, adapter_plugin: None, status: None, skills: None },
            &adapters(),
            &["taken".to_string()],
        )
        .unwrap();
        assert_eq!(ok.name.as_deref(), Some("Fresh"));
    }

    fn skills(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn validate_skills_slugifies_dedups_and_clamps() {
        // Slugified (lowercase, separators → hyphen), deduped (case-insensitive), order kept.
        let ok = validate_skills(&skills(&["Rust", "  back end ", "rust", "Data_Science"])).unwrap();
        assert_eq!(ok, vec!["rust", "back-end", "data-science"]);
        // Blank entries (a trailing comma in the UI) are skipped, not errors.
        let ok = validate_skills(&skills(&["rust", "   ", ""])).unwrap();
        assert_eq!(ok, vec!["rust"]);
        // An over-long skill is clamped to MAX_SKILL_CHARS, not rejected.
        let long = "a".repeat(100);
        let ok = validate_skills(&[long]).unwrap();
        assert_eq!(ok[0].chars().count(), MAX_SKILL_CHARS);
    }

    #[test]
    fn validate_skills_rejects_unsanitizable_and_overflow() {
        // An entry with real content that sanitizes to nothing is an honest error.
        let err = validate_skills(&skills(&["💥🔥"])).unwrap_err();
        assert_eq!(err, AgentConfigError::InvalidSkill("💥🔥".to_string()));
        // More than MAX_SKILLS distinct skills is rejected.
        let many: Vec<String> = (0..MAX_SKILLS + 1).map(|i| format!("skill-{i}")).collect();
        let err = validate_skills(&many).unwrap_err();
        assert_eq!(err, AgentConfigError::TooManySkills(MAX_SKILLS + 1));
    }

    #[test]
    fn create_accepts_and_validates_skills() {
        let provided = skills(&["Rust", "rust", "Backend"]);
        let ok = validate_new_agent(
            CreateAgentInput { id: None, name: "Bot", role: None, persona: None, adapter_plugin: None, skills: Some(&provided) },
            &adapters(),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(ok.skills, vec!["rust", "backend"]);
        // No skills field => empty list (backwards compatible).
        let ok = validate_new_agent(
            CreateAgentInput { id: None, name: "Bot", role: None, persona: None, adapter_plugin: None, skills: None },
            &adapters(),
            &[],
            &[],
        )
        .unwrap();
        assert!(ok.skills.is_empty());
    }

    #[test]
    fn update_replaces_or_clears_skills() {
        // A present list REPLACES the whole skill set.
        let provided = skills(&["design", "ux"]);
        let resolved = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: None, adapter_plugin: None, status: None, skills: Some(&provided) },
            &adapters(),
            &[],
        )
        .unwrap();
        assert_eq!(resolved.skills, Some(vec!["design".to_string(), "ux".to_string()]));
        // An empty list CLEARS all skills.
        let resolved = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: None, adapter_plugin: None, status: None, skills: Some(&[]) },
            &adapters(),
            &[],
        )
        .unwrap();
        assert_eq!(resolved.skills, Some(vec![]));
        // Absent => unchanged (None).
        let resolved = validate_agent_update(
            UpdateAgentInput { name: None, role: None, persona: None, adapter_plugin: None, status: None, skills: None },
            &adapters(),
            &[],
        )
        .unwrap();
        assert_eq!(resolved.skills, None);
    }
}
