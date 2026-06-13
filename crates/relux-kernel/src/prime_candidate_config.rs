//! Prime-guided activation of a detected **capability candidate** of an installed
//! plugin, through the EXISTING governed configuration paths.
//!
//! [`crate::capability_detect::detect_candidates`] turns an imported source into a
//! bounded list of [`CapabilityCandidate`]s, each with an honest `activation`:
//! `mcp_register` (one-click governed registration on the unchanged MCP registry) or
//! `command_tool` (a pre-filled, reviewable argv draft for the unchanged command-tool
//! path). Until now the operator had to leave chat and configure each candidate by
//! hand on the Plugins page. This module is the seam that lets Prime guide that step:
//!
//! - The pure text parser [`parse_candidate_config_request`] reads a chat message like
//!   "configure the first candidate" / "enable the MCP server from hermes-agent" /
//!   "turn that script into a tool" into a [`CandidateConfigSelector`] (a fuzzy plugin
//!   selector + a candidate selector). Prime stages a `ConfigurePluginCandidate`
//!   proposal from it — always behind a human approval.
//! - The pure resolver [`resolve_candidate`] maps a selector (a concrete id, or a
//!   keyword like `"mcp"` / `"command"` / `"first"`) onto one candidate of a freshly
//!   re-read list, so the backend never trusts a client-supplied command — it re-reads
//!   the candidates server-side and re-resolves the selection.
//! - The pure draft builder [`command_tool_body`] renders a candidate's pre-filled
//!   [`crate::capability_detect::CommandToolProposal`] into the exact JSON the existing
//!   [`crate::parse_command_tool_input`] validator already accepts, so activation reuses
//!   the unchanged command-tool validation + storage with no duplicated spawn logic.
//!
//! ## Hard invariants
//!
//! - **Executes nothing.** Resolving/drafting only reshapes already-detected,
//!   read-only metadata. The actual MCP registration / command-tool configuration
//!   flows through the EXISTING kernel paths (which re-validate the argv/loopback
//!   safety contract), and the resulting tool always stays gated until invoked.
//! - **Never trusts the client.** The backend re-reads candidates from the plugin's
//!   install directory and re-resolves the selector — a tampered command field in a
//!   request body can never reach a spawn, because the spawn recipe is rebuilt here
//!   from the server-side scan, not from the request.
//!
//! Reference grounding (`docs/reference-driven-development.md`, BINDING):
//! - `reference/hermes-agent-main/hermes_cli/mcp_config.py` (`cmd_mcp_add`,
//!   `_get_mcp_servers`) — registering an MCP server is keying a `{command,args,env}`
//!   (or `{url}`) entry by name; configuration is a separate step from running it. We
//!   register through the unchanged registry and never spawn on configuration.
//! - `reference/openclaw-main/extensions/acpx/src/config-schema.ts`
//!   (`McpServerConfig = {command, args, env}`) — the same server shape we rebuild from
//!   the candidate's proposal before handing it to the existing registry validation.

use serde_json::json;

use crate::capability_detect::CapabilityCandidate;

/// A parsed chat request to configure a detected capability candidate: a best-effort
/// plugin selector and a candidate selector, both resolved AUTHORITATIVELY server-side
/// against a fresh candidate scan. Neither is trusted as a concrete command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateConfigSelector {
    /// A fuzzy plugin reference parsed from the message ("from hermes-agent",
    /// "for acme-tools"), or empty when the user said only "the first candidate" — the
    /// backend then resolves it to the unique installed plugin that has candidates.
    pub plugin_selector: String,
    /// The candidate selector: a keyword (`"mcp"` / `"command"` / `"first"`) the
    /// resolver maps onto one candidate of the re-read list.
    pub candidate_selector: String,
}

/// Parse a chat message into a [`CandidateConfigSelector`], or `None` when the message
/// is not an explicit candidate-configuration request. Pure text — reads no state and
/// runs nothing. The caller has already routed the message to
/// [`relux_core::PrimeIntent::PluginConfiguration`]; this only extracts the selectors.
pub fn parse_candidate_config_request(message: &str) -> Option<CandidateConfigSelector> {
    let m = message.trim().to_lowercase();
    if m.is_empty() {
        return None;
    }

    // The candidate selector: an explicit MCP cue wins (MCP is the one-click path), then
    // a command/script/tool cue, then the positional "first" default. A bare
    // "configure the candidate" with no kind cue defaults to "first".
    let candidate_selector = if m.contains("mcp") || m.contains(" server") {
        "mcp"
    } else if m.contains("command tool")
        || m.contains("command-tool")
        || m.contains("script")
        || m.contains("binary")
        || m.contains("cli")
        || (m.contains("command") && !m.contains("recommend"))
    {
        "command"
    } else {
        "first"
    }
    .to_string();

    // The plugin selector: the token following a "from/for/on/of/in <plugin>" cue, when
    // present. Best-effort and fuzzy — the backend re-resolves it (an empty selector
    // means "the unique plugin with candidates"). Strip trailing punctuation and a
    // possessive so "hermes-agent's" / "acme-tools," both reduce cleanly.
    let plugin_selector = extract_plugin_token(&m).unwrap_or_default();

    Some(CandidateConfigSelector {
        plugin_selector,
        candidate_selector,
    })
}

/// Extract a plausible plugin token from a "from/for/on/of <plugin>" phrasing. Returns
/// the cleaned token, or `None` when no such reference is present (the common
/// "configure the first candidate" case). Pure.
fn extract_plugin_token(m: &str) -> Option<String> {
    const CUES: &[&str] = &[" from ", " for ", " on ", " of "];
    // A handful of words that follow a cue but are NOT a plugin reference, so
    // "configure the mcp server from chat" / "... from the candidate" don't mistake a
    // filler word for a plugin id.
    const STOPWORDS: &[&str] = &[
        "the", "that", "this", "it", "chat", "candidate", "plugin", "github", "a", "my", "our",
    ];
    for cue in CUES {
        if let Some(idx) = m.find(cue) {
            let rest = &m[idx + cue.len()..];
            let raw = rest.split_whitespace().next().unwrap_or("");
            let token: String = raw
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
                .trim_end_matches("'s")
                .to_string();
            if token.is_empty() || STOPWORDS.contains(&token.as_str()) {
                continue;
            }
            return Some(token);
        }
    }
    None
}

/// Resolve a candidate selector against a freshly re-read candidate list. Tries, in
/// order: an exact `id` match (the dashboard button path), then a keyword selector
/// (`"mcp"` → the first `mcp_register` candidate, `"command"` → the first
/// `command_tool` candidate, `"first"`/empty → the first ACTIVATABLE candidate — one
/// whose activation is `mcp_register` or `command_tool`, skipping honest `manual`
/// pending records). Returns `None` when nothing matches (an honest "no such
/// candidate"). Pure — selects from the already-read list, runs nothing.
pub fn resolve_candidate<'a>(
    candidates: &'a [CapabilityCandidate],
    selector: &str,
) -> Option<&'a CapabilityCandidate> {
    let sel = selector.trim();
    // 1) Exact id (the unambiguous button path).
    if let Some(c) = candidates.iter().find(|c| c.id == sel) {
        return Some(c);
    }
    let key = sel.to_lowercase();
    // 2) Keyword selectors.
    if key == "mcp" || key == "mcp_register" || key.contains("server") {
        return candidates.iter().find(|c| c.activation == "mcp_register");
    }
    if key == "command" || key == "command_tool" || key.contains("script") || key.contains("cli") {
        return candidates.iter().find(|c| c.activation == "command_tool");
    }
    if key.is_empty() || key == "first" || key == "1" || key.contains("first") {
        return candidates
            .iter()
            .find(|c| is_activatable(c));
    }
    None
}

/// Whether a candidate has a one-click/governed activation path (so a "first" selector
/// skips honest `manual` pending records that Prime cannot wire up).
pub fn is_activatable(c: &CapabilityCandidate) -> bool {
    c.activation == "mcp_register" || c.activation == "command_tool"
}

/// Build the exact JSON body the existing [`crate::parse_command_tool_input`] validator
/// accepts, from a `command_tool` candidate's pre-filled proposal. Returns `None` when
/// the candidate is not a `command_tool` activation (no proposal to draft from). Pure —
/// the produced body still flows through the unchanged command-tool validation +
/// storage; nothing is run here.
pub fn command_tool_body(candidate: &CapabilityCandidate) -> Option<serde_json::Value> {
    if candidate.activation != "command_tool" {
        return None;
    }
    let p = candidate.command_tool.as_ref()?;
    let mut body = json!({
        "name": p.tool_name,
        "description": p.description,
        "program": p.program,
        "args": p.args,
    });
    if let Some(cwd) = p.cwd.as_ref() {
        body["cwd"] = json!(cwd);
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_detect::{CapabilityCandidate, CommandToolProposal};
    use crate::mcp_proposal::McpRegistrationProposal;

    fn mcp_candidate() -> CapabilityCandidate {
        CapabilityCandidate {
            id: "mcp-server".to_string(),
            kind: "mcp_stdio".to_string(),
            title: "MCP server (stdio)".to_string(),
            confidence: "high".to_string(),
            risk: "medium".to_string(),
            rationale: "an MCP server signal".to_string(),
            command_preview: Some("node ./dist/server.js".to_string()),
            env_placeholders: Vec::new(),
            activation: "mcp_register".to_string(),
            mcp_registration: Some(McpRegistrationProposal {
                suggested_id: "cool-mcp".to_string(),
                suggested_description: "Imported MCP server".to_string(),
                suggested_endpoint: None,
                endpoint_required: false,
                suggested_transport: "managed_stdio".to_string(),
                detected_command: Some("node".to_string()),
                detected_args: vec!["./dist/server.js".to_string()],
                notes: Vec::new(),
            }),
            command_tool: None,
            next_steps: Vec::new(),
        }
    }

    fn command_candidate() -> CapabilityCandidate {
        CapabilityCandidate {
            id: "cli-bin-cool".to_string(),
            kind: "cli_command".to_string(),
            title: "Command-line tool (npm bin)".to_string(),
            confidence: "medium".to_string(),
            risk: "medium".to_string(),
            rationale: "a declared bin".to_string(),
            command_preview: Some("node ./cli.js".to_string()),
            env_placeholders: Vec::new(),
            activation: "command_tool".to_string(),
            mcp_registration: None,
            command_tool: Some(CommandToolProposal {
                tool_name: "cool.run".to_string(),
                program: "node".to_string(),
                args: vec!["./cli.js".to_string()],
                cwd: None,
                description: "Command-line tool (configured from a detected entrypoint)".to_string(),
            }),
            next_steps: Vec::new(),
        }
    }

    fn manual_candidate() -> CapabilityCandidate {
        let mut c = command_candidate();
        c.id = "py-script-serve".to_string();
        c.activation = "manual".to_string();
        c.command_tool = None;
        c
    }

    #[test]
    fn parses_mcp_request_with_plugin_name() {
        let sel = parse_candidate_config_request("enable the MCP server from hermes-agent").unwrap();
        assert_eq!(sel.candidate_selector, "mcp");
        assert_eq!(sel.plugin_selector, "hermes-agent");
    }

    #[test]
    fn parses_first_candidate_with_no_plugin() {
        let sel = parse_candidate_config_request("configure the first candidate").unwrap();
        assert_eq!(sel.candidate_selector, "first");
        assert_eq!(sel.plugin_selector, "", "no plugin named ⇒ backend resolves the unique one");
    }

    #[test]
    fn parses_script_into_a_tool() {
        let sel = parse_candidate_config_request("turn that script into a tool").unwrap();
        assert_eq!(sel.candidate_selector, "command");
        assert_eq!(sel.plugin_selector, "");
    }

    #[test]
    fn filler_words_after_cue_are_not_plugin_tokens() {
        // "from chat" / "from the candidate" must not be read as a plugin id.
        let sel = parse_candidate_config_request("configure the mcp server from chat").unwrap();
        assert_eq!(sel.plugin_selector, "");
    }

    #[test]
    fn resolves_exact_id_first() {
        let cs = vec![mcp_candidate(), command_candidate()];
        let c = resolve_candidate(&cs, "cli-bin-cool").unwrap();
        assert_eq!(c.id, "cli-bin-cool");
    }

    #[test]
    fn resolves_mcp_keyword() {
        let cs = vec![command_candidate(), mcp_candidate()];
        let c = resolve_candidate(&cs, "mcp").unwrap();
        assert_eq!(c.activation, "mcp_register");
    }

    #[test]
    fn resolves_command_keyword() {
        let cs = vec![mcp_candidate(), command_candidate()];
        let c = resolve_candidate(&cs, "command").unwrap();
        assert_eq!(c.activation, "command_tool");
    }

    #[test]
    fn first_skips_manual_pending() {
        // A manual candidate leads the list but "first" must pick the activatable one.
        let cs = vec![manual_candidate(), command_candidate()];
        let c = resolve_candidate(&cs, "first").unwrap();
        assert_eq!(c.id, "cli-bin-cool", "first ⇒ first ACTIVATABLE, not the manual record");
    }

    #[test]
    fn unknown_selector_resolves_to_none() {
        let cs = vec![mcp_candidate()];
        assert!(resolve_candidate(&cs, "nope-not-here").is_none());
    }

    #[test]
    fn command_tool_body_matches_validator_shape() {
        let c = command_candidate();
        let body = command_tool_body(&c).unwrap();
        assert_eq!(body["name"], "cool.run");
        assert_eq!(body["program"], "node");
        assert_eq!(body["args"][0], "./cli.js");
        // The body parses through the EXISTING command-tool validator unchanged.
        let draft = crate::parse_command_tool_input(&body).expect("draft validates");
        assert_eq!(draft.program, "node");
        assert_eq!(draft.args, vec!["./cli.js".to_string()]);
    }

    #[test]
    fn command_tool_body_none_for_mcp_candidate() {
        assert!(command_tool_body(&mcp_candidate()).is_none());
    }
}
