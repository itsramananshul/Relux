//! Safe, read-only **MCP registration proposal** from an imported plugin's source.
//!
//! When [`crate::introspect::detect_hints`] flags an imported source as a likely
//! MCP server (an `@modelcontextprotocol/sdk` dependency, an `mcp` Python dep, or a
//! standalone MCP config file), the operator still has to wire it up by hand. This
//! module turns that hint into a **reviewable, pre-filled draft** for the EXISTING
//! loopback MCP registry (`POST /v1/relux/mcp/servers`) — never a parallel registry,
//! and never an auto-action.
//!
//! ## What it proposes (and what it deliberately will not)
//!
//! - A **sanitized server id** (from the npm package name, else the plugin id),
//!   always reduced to a valid [`relux_core::is_valid_mcp_id`] id (fail-closed) or
//!   left for the operator when nothing usable remains.
//! - A **description** (from `package.json` `description`, else an honest default).
//! - A **loopback HTTP endpoint** ONLY when an MCP config file names a `url` that
//!   passes the same [`relux_core::validate_loopback_url`] rule the registry
//!   enforces. A non-loopback / `https` / missing `url` is NEVER pre-filled — the
//!   operator must enter the address (fail-closed manual entry).
//! - A detected **stdio `{command, args}`** server is surfaced **informationally
//!   only**, bounded, so the operator knows what to run themselves. Relux **never**
//!   runs a command or downloaded code, and the command/args are never the endpoint
//!   and never stored — they are display text. (`docs/mcp.md` "No stdio (command)
//!   MCP servers".)
//!
//! Crucially: building a proposal **executes nothing**. It only reads the same
//! bounded metadata files the hint scan already reads. Registration itself still
//! flows through the unchanged registry route/store/validation; the proposal merely
//! pre-fills the review form.
//!
//! Reference grounding (`docs/reference-driven-development.md`, BINDING):
//! - `reference/hermes-agent-main/hermes_cli/mcp_config.py` — a server config is
//!   either `{"url": <endpoint>}` (HTTP) or `{"command", "args", "env"}` (stdio),
//!   keyed by name in an `mcp_servers` map (`cmd_mcp_add`, `_get_mcp_servers`,
//!   `_apply_mcp_preset` L135-162). We read exactly that shape: a loopback `url`
//!   becomes the pre-filled endpoint; a `{command,args}` is surfaced as advisory
//!   text only (Relux goes stricter than Hermes — it dials loopback HTTP and never
//!   spawns a command).
//! - `reference/openclaw-main/src/plugins/install-security-scan.ts`
//!   (`scanBundleInstallSource`) — static, filesystem-only inspection that surfaces
//!   findings without ever running the source; we keep the same posture.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::introspect::PluginHint;
use relux_core::{
    is_valid_mcp_id, sanitize_mcp_server_id, validate_loopback_url, MAX_MCP_DESCRIPTION_CHARS,
};

/// Largest metadata file read while proposing (bytes) — matches the hint scan bound.
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Bound on a detected stdio command string (display only).
const MAX_CMD_CHARS: usize = 256;
/// Bound on each detected stdio arg (display only).
const MAX_ARG_CHARS: usize = 256;
/// Most detected stdio args surfaced (display only).
const MAX_ARGS: usize = 32;
/// Most advisory notes returned.
const MAX_NOTES: usize = 8;

/// A safe, pre-filled draft for registering an imported MCP source on the EXISTING
/// loopback MCP registry. Informational + advisory only — nothing here is executed,
/// and registration still goes through the unchanged `POST /v1/relux/mcp/servers`
/// route + validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpRegistrationProposal {
    /// A sanitized, valid MCP server id derived from the source (always safe to
    /// submit). The operator may still edit it before registering.
    pub suggested_id: String,
    /// A description derived from the source, or an honest default.
    pub suggested_description: String,
    /// A loopback HTTP endpoint pre-filled ONLY when an MCP config named a `url`
    /// that passes the loopback rule. Absent ⇒ the operator must enter it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_endpoint: Option<String>,
    /// True when no endpoint could be safely inferred, so the review form must
    /// require manual entry (fail-closed).
    pub endpoint_required: bool,
    /// A stdio command detected in an MCP config — INFORMATIONAL ONLY. Relux never
    /// runs it; it is shown so the operator knows what to start themselves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_command: Option<String>,
    /// The detected stdio command's args — informational only, bounded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected_args: Vec<String>,
    /// Honest, human-readable notes about what was detected and why manual entry may
    /// be needed. Never claims anything is runnable.
    pub notes: Vec<String>,
}

/// Build a pre-filled MCP registration proposal from an imported source, or `None`
/// when the source carries no MCP signal (fail-closed: no hint ⇒ no action).
///
/// `dir` is the installed plugin directory (already scanned for `hints`), `plugin_id`
/// is the installed plugin's id (the id fallback), and `hints` is the result of
/// [`crate::introspect::detect_hints`] for the same directory. Reads only the same
/// bounded metadata files the hint scan reads; executes nothing.
pub fn propose_mcp_registration(
    dir: &Path,
    plugin_id: &str,
    hints: &[PluginHint],
) -> Option<McpRegistrationProposal> {
    if !dir.is_dir() {
        return None;
    }
    // Only propose when an MCP signal was actually detected. No signal ⇒ no action.
    let is_mcp = hints
        .iter()
        .any(|h| h.kind == "mcp-server" || h.kind == "mcp-config");
    if !is_mcp {
        return None;
    }

    let mut notes: Vec<String> = Vec::new();

    // Derive a safe id: prefer the npm package name, else the plugin id. Always
    // sanitized to a valid mcp id; the raw string is never trusted.
    let raw_id = read_package_name(dir).unwrap_or_else(|| plugin_id.to_string());
    let mut suggested_id = sanitize_mcp_server_id(&raw_id);
    if suggested_id.is_empty() {
        suggested_id = sanitize_mcp_server_id(plugin_id);
    }
    if suggested_id.is_empty() {
        suggested_id = "imported-mcp".to_string();
    }
    debug_assert!(is_valid_mcp_id(&suggested_id));

    // Derive a description from the package metadata, else an honest default.
    let suggested_description = read_package_description(dir)
        .map(|d| clamp(&d, MAX_MCP_DESCRIPTION_CHARS))
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| format!("Imported MCP server (from plugin {plugin_id})"));

    // Try to infer wiring from an MCP config file. Only a loopback HTTP `url` is
    // safe to pre-fill as the endpoint; a `{command,args}` stdio server is surfaced
    // as informational text only — Relux never runs a command.
    let mut suggested_endpoint: Option<String> = None;
    let mut detected_command: Option<String> = None;
    let mut detected_args: Vec<String> = Vec::new();

    match read_mcp_config_server(dir) {
        Some(ServerWire::Url(url)) => {
            let trimmed = url.trim().to_string();
            if validate_loopback_url(&trimmed).is_ok() {
                suggested_endpoint = Some(trimmed);
                notes.push(
                    "Pre-filled the loopback endpoint from the MCP config file. Confirm it \
                     matches the address your server actually listens on."
                        .to_string(),
                );
            } else {
                notes.push(format!(
                    "The MCP config names a non-loopback URL ({}). Relux dials only loopback \
                     HTTP, so enter your local server's loopback address below.",
                    clamp(&trimmed, 80)
                ));
            }
        }
        Some(ServerWire::Command(cmd, args)) => {
            detected_command = Some(clamp(&cmd, MAX_CMD_CHARS));
            detected_args = args
                .into_iter()
                .take(MAX_ARGS)
                .map(|a| clamp(&a, MAX_ARG_CHARS))
                .filter(|a| !a.is_empty())
                .collect();
            notes.push(
                "This server runs as a command (stdio). Relux never runs commands or \
                 downloaded code — start it yourself as a loopback HTTP server, then enter \
                 that address below."
                    .to_string(),
            );
        }
        None => {}
    }

    if suggested_endpoint.is_none() && detected_command.is_none() {
        // npm/python MCP servers are typically stdio; a loopback address can't be
        // inferred from metadata. Force manual entry (fail-closed).
        notes.push(
            "Relux could not infer a loopback address from the source. Run the server \
             yourself as a loopback HTTP endpoint and enter it below."
                .to_string(),
        );
    }

    notes.truncate(MAX_NOTES);

    Some(McpRegistrationProposal {
        endpoint_required: suggested_endpoint.is_none(),
        suggested_id,
        suggested_description,
        suggested_endpoint,
        detected_command,
        detected_args,
        notes,
    })
}

/// Trim, clamp to `max` chars, and re-trim a display string.
fn clamp(s: &str, max: usize) -> String {
    s.trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Read a bounded UTF-8 metadata file, or `None` if missing / too large / not UTF-8.
fn read_bounded(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// The `name` field of a top-level `package.json`, when present.
fn read_package_name(dir: &Path) -> Option<String> {
    let text = read_bounded(&dir.join("package.json"))?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// The `description` field of a top-level `package.json`, when present.
fn read_package_description(dir: &Path) -> Option<String> {
    let text = read_bounded(&dir.join("package.json"))?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// The two MCP server wire shapes we read from a config file (Hermes
/// `mcp_config.py`): an HTTP `url`, or a stdio `{command, args}`.
enum ServerWire {
    Url(String),
    Command(String, Vec<String>),
}

/// Read the first server entry from a standalone MCP config file, if any. Looks for
/// the standard `{"mcpServers": {name: cfg}}` / `{"servers": {…}}` map and falls
/// back to treating the whole document as a single server config.
fn read_mcp_config_server(dir: &Path) -> Option<ServerWire> {
    for file in ["mcp.json", ".mcp.json", "mcp-config.json", "mcp_config.json"] {
        if let Some(text) = read_bounded(&dir.join(file)) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(wire) = first_server_wire(&json) {
                    return Some(wire);
                }
            }
        }
    }
    None
}

/// Resolve the first server config from a parsed MCP config document.
fn first_server_wire(json: &serde_json::Value) -> Option<ServerWire> {
    let map = json.get("mcpServers").or_else(|| json.get("servers"));
    let cfg = match map {
        Some(serde_json::Value::Object(m)) => m.values().next()?,
        _ => json,
    };
    server_wire_from(cfg)
}

/// Pull a `url` (HTTP) or `{command, args}` (stdio) out of one server config object.
fn server_wire_from(cfg: &serde_json::Value) -> Option<ServerWire> {
    if let Some(url) = cfg.get("url").and_then(|v| v.as_str()) {
        if !url.trim().is_empty() {
            return Some(ServerWire::Url(url.to_string()));
        }
    }
    if let Some(cmd) = cfg.get("command").and_then(|v| v.as_str()) {
        if !cmd.trim().is_empty() {
            let args = cfg
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return Some(ServerWire::Command(cmd.to_string(), args));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspect::detect_hints;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    fn propose(dir: &Path, id: &str) -> Option<McpRegistrationProposal> {
        let hints = detect_hints(dir);
        propose_mcp_registration(dir, id, &hints)
    }

    #[test]
    fn no_mcp_signal_yields_no_proposal() {
        let d = tmp();
        write(
            d.path(),
            "package.json",
            r#"{"name":"plain","dependencies":{"left-pad":"1.0.0"}}"#,
        );
        assert!(propose(d.path(), "plain").is_none(), "no MCP hint ⇒ no proposal");
    }

    #[test]
    fn npm_mcp_sdk_proposes_a_safe_id_and_requires_manual_endpoint() {
        let d = tmp();
        write(
            d.path(),
            "package.json",
            r#"{"name":"@acme/Cool-MCP","description":"A cool MCP server.",
               "bin":{"cool":"./bin.js"},
               "dependencies":{"@modelcontextprotocol/sdk":"^1.0.0"}}"#,
        );
        let p = propose(d.path(), "cool-mcp-plugin").expect("an MCP source proposes a draft");
        // The id is sanitized from the package name, valid, never raw.
        assert_eq!(p.suggested_id, "acme-cool-mcp");
        assert!(is_valid_mcp_id(&p.suggested_id));
        // The description is carried from the package metadata.
        assert_eq!(p.suggested_description, "A cool MCP server.");
        // No config file ⇒ no inferable endpoint ⇒ manual entry is required.
        assert!(p.suggested_endpoint.is_none());
        assert!(p.endpoint_required);
        assert!(p.detected_command.is_none());
        assert!(
            p.notes.iter().any(|n| n.contains("loopback")),
            "a manual-entry note must explain the loopback requirement: {:?}",
            p.notes
        );
    }

    #[test]
    fn mcp_config_with_loopback_url_prefills_the_endpoint() {
        let d = tmp();
        write(
            d.path(),
            "mcp.json",
            r#"{"mcpServers":{"fs":{"url":"http://127.0.0.1:8000/mcp"}}}"#,
        );
        let p = propose(d.path(), "fs-plugin").expect("an mcp.json proposes a draft");
        assert_eq!(
            p.suggested_endpoint.as_deref(),
            Some("http://127.0.0.1:8000/mcp"),
            "a loopback url is safely pre-filled"
        );
        assert!(!p.endpoint_required, "a pre-filled endpoint is not required");
    }

    #[test]
    fn mcp_config_with_remote_url_does_not_prefill_and_requires_manual() {
        let d = tmp();
        write(
            d.path(),
            "mcp.json",
            r#"{"mcpServers":{"x":{"url":"https://mcp.example.com/mcp"}}}"#,
        );
        let p = propose(d.path(), "x-plugin").expect("draft");
        // A non-loopback URL is NEVER pre-filled (fail-closed); manual entry forced.
        assert!(p.suggested_endpoint.is_none());
        assert!(p.endpoint_required);
        assert!(
            p.notes.iter().any(|n| n.contains("non-loopback")),
            "must explain the rejected remote URL: {:?}",
            p.notes
        );
    }

    #[test]
    fn stdio_command_config_is_advisory_only_never_an_endpoint() {
        let d = tmp();
        write(
            d.path(),
            "mcp.json",
            r#"{"mcpServers":{"gh":{"command":"npx","args":["@modelcontextprotocol/server-github"]}}}"#,
        );
        let p = propose(d.path(), "gh-plugin").expect("draft");
        // The command is surfaced informationally; it is NEVER the endpoint.
        assert_eq!(p.detected_command.as_deref(), Some("npx"));
        assert_eq!(p.detected_args, vec!["@modelcontextprotocol/server-github".to_string()]);
        assert!(p.suggested_endpoint.is_none(), "a command is never an endpoint");
        assert!(p.endpoint_required);
        assert!(
            p.notes.iter().any(|n| n.contains("never runs commands")),
            "must be honest that Relux will not run the command: {:?}",
            p.notes
        );
    }

    #[test]
    fn id_falls_back_to_plugin_id_then_default() {
        let d = tmp();
        // An MCP config with no package.json: the id comes from the plugin id.
        write(d.path(), "mcp.json", r#"{"mcpServers":{}}"#);
        let p = propose(d.path(), "My Imported Repo").expect("draft");
        assert_eq!(p.suggested_id, "my-imported-repo");
        assert!(is_valid_mcp_id(&p.suggested_id));
    }

    #[test]
    fn no_proposal_for_a_missing_directory() {
        let d = tmp();
        let missing = d.path().join("nope");
        assert!(propose_mcp_registration(&missing, "x", &[]).is_none());
    }
}
