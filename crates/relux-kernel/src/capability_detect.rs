//! Structured, read-only **capability candidates** for an imported plugin source.
//!
//! [`crate::introspect::detect_hints`] answers "what *is* this source?" with a flat
//! list of advisory hints, and [`crate::mcp_proposal::propose_mcp_registration`]
//! turns a single MCP signal into one pre-filled registration draft. This module
//! sits on top of both and answers the next question an operator actually has after
//! importing an ordinary repo with no `relux-plugin.json`:
//!
//!   > "You detected things — now give me a concrete, per-capability path to make
//!   >  *each* of them usable, and tell me honestly which ones Relux can wire up in
//!   >  one click versus which still need me."
//!
//! It produces a bounded list of [`CapabilityCandidate`]s, each carrying a
//! candidate `kind`, a `confidence`, a `risk`, a one-line `rationale` (the exact
//! signal Relux matched), a non-secret `command_preview`, any required
//! `env_placeholders` (names only — never values), and an honest `activation`:
//!
//! - `mcp_register` — Relux can turn this into a real, usable capability **now**
//!   through the EXISTING loopback MCP registry. The candidate carries a pre-filled
//!   [`McpRegistrationProposal`] for the unchanged `POST /v1/relux/mcp/servers`
//!   review form. This is the one path the architecture supports for one-click
//!   activation, so MCP candidates are surfaced first.
//! - `manual` — Relux has no governed runtime that can auto-activate this shape yet
//!   (a CLI binary, a Python/Cargo script). It is an **honest pending capability**:
//!   no fake "ready" state, just the concrete next steps through the existing
//!   governed paths (run it as a loopback server + add a tool definition, or author
//!   a `relux-plugin.json`). Relux never runs the source on its behalf.
//!
//! ## Hard invariants (same posture as the hint scan)
//!
//! - **Executes nothing.** Detection only reads the same bounded allow-list of
//!   top-level metadata files the hint scan reads. It never spawns a process,
//!   follows a command/entrypoint, recurses, or builds anything. A `command_preview`
//!   is display text the operator reviews — Relux runs it only after an explicit,
//!   gated registration (managed-stdio, argv-only, never a shell), never on import.
//! - **Never promotes a candidate into a runnable tool.** A candidate is a
//!   suggestion for a governed path; tools come only from a real manifest, operator
//!   tool definitions, or a registered+classified MCP server.
//! - **Bounded.** The candidate list is capped at [`MAX_CANDIDATES`]; every file
//!   read is capped at [`MAX_FILE_BYTES`].
//!
//! Reference grounding (`docs/reference-driven-development.md`, BINDING):
//! - `reference/hermes-agent-main/hermes_cli/mcp_config.py` (`_MCP_PRESETS`,
//!   `_get_mcp_servers`) and `reference/openclaw-main/extensions/acpx/src/config-schema.ts`
//!   (`McpServerConfig = {command, args, env}`) — an MCP/stdio capability is a
//!   `{command, args, env}` shape keyed in a server map; that is the shape a
//!   `mcp_stdio` candidate previews and pre-fills for managed-stdio registration.
//! - `reference/openclaw-main/src/plugins/install-security-scan.ts`
//!   (`scanBundleInstallSource`) — static, filesystem-only inspection that surfaces
//!   structured findings without ever running the source; we keep that posture and
//!   add honest activation guidance on top.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::introspect::PluginHint;
use crate::mcp_proposal::{propose_mcp_registration, McpRegistrationProposal};

/// Largest metadata file read while detecting candidates (bytes) — matches the
/// hint scan / proposal bound.
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// The most candidates returned for any one source — a safety cap so a pathological
/// tree can never produce an unbounded list.
const MAX_CANDIDATES: usize = 16;
/// Bound on a previewed command / arg / placeholder string (display only).
const MAX_PREVIEW_CHARS: usize = 256;
/// Most env placeholders surfaced per candidate (names only).
const MAX_ENV_PLACEHOLDERS: usize = 16;

/// One structured, read-only capability candidate detected in an imported source.
/// Advisory only: nothing here is executed, and only `mcp_register` candidates have
/// a one-click governed path to a usable capability (the rest are honest pending
/// records with concrete manual next steps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityCandidate {
    /// A stable, per-source slug so the dashboard can key/configure a candidate,
    /// e.g. `"mcp-server"`, `"cli-bin-cool"`, `"py-script-serve"`. Charset is
    /// `[a-z0-9-]`; never used to execute anything.
    pub id: String,
    /// The candidate kind the UI badges/branches on:
    /// `"mcp_stdio"` | `"mcp_http"` | `"cli_command"`.
    pub kind: String,
    /// A short human label, e.g. "MCP server (stdio)".
    pub title: String,
    /// How strongly the source signals this capability: `"high"` | `"medium"` |
    /// `"low"`. Honest, never inflated — a build-required or inferred shape is lower.
    pub confidence: String,
    /// The candidate's risk band (`"low"` | `"medium"` | `"high"`) — advisory; the
    /// authoritative gate is still the per-tool classification after registration.
    pub risk: String,
    /// Why Relux thinks this is a candidate: the exact signal matched (a dependency,
    /// a declared `bin`, a `[project.scripts]` entry, a `[[bin]]` target). Non-secret.
    pub rationale: String,
    /// A non-secret preview of the command an operator would run / register, or
    /// `None` when nothing concrete could be inferred. Display text — never run by
    /// detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_preview: Option<String>,
    /// Required environment variable NAMES the source appears to expect (e.g. from an
    /// MCP config's `env` map). Names only — never values, never secrets. The
    /// operator maps each to a stored secret at registration time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_placeholders: Vec<String>,
    /// How this candidate becomes usable: `"mcp_register"` (one-click governed
    /// registration via the EXISTING MCP registry) or `"manual"` (an honest pending
    /// capability — follow `next_steps`, Relux runs nothing for you).
    pub activation: String,
    /// Present ONLY for an `mcp_register` candidate: a safe, pre-filled draft for the
    /// unchanged `POST /v1/relux/mcp/servers` review form. Executes nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_registration: Option<McpRegistrationProposal>,
    /// Present ONLY for a `command_tool` candidate: a safe, pre-filled argv draft for
    /// the `POST /v1/relux/plugins/:id/command-tools` review form. Display text the
    /// operator reviews/edits — detection runs nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_tool: Option<CommandToolProposal>,
    /// Honest, concrete next steps to make this capability usable through the
    /// existing governed paths. Never claims anything is already runnable.
    pub next_steps: Vec<String>,
}

/// A safe, pre-filled draft for configuring a detected CLI/script/binary into a
/// governed command tool. Carries only display text the operator reviews and edits in
/// the `POST /v1/relux/plugins/:id/command-tools` form; detection never runs it. The
/// program is a best-guess launcher split from the candidate's `command_preview` — the
/// operator confirms/corrects it before anything is stored, and the resulting tool
/// always requires approval to invoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandToolProposal {
    /// Suggested manifest tool name (sanitized server-side on submit), e.g. `cool.run`.
    pub tool_name: String,
    /// Suggested program (`argv[0]`) — a launcher token split from the preview.
    pub program: String,
    /// Suggested fixed args (the remaining preview tokens).
    pub args: Vec<String>,
    /// Suggested working directory, relative to the install dir. `None` ⇒ the install
    /// dir root (the default for a repo-relative entrypoint).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// A short human description carried into the form.
    pub description: String,
}

/// Detect structured capability candidates for an imported plugin source.
///
/// `dir` is the installed plugin directory (already scanned for `hints`),
/// `plugin_id` is the installed plugin's id (an id fallback), and `hints` is the
/// result of [`crate::introspect::detect_hints`] for the same directory. Reads only
/// the same bounded metadata files the hint scan reads; executes nothing. Returns an
/// empty vec for a source with no recognizable runnable capability (an honest
/// "nothing detected"), so the UI can show exact "what to add" guidance.
pub fn detect_candidates(
    dir: &Path,
    plugin_id: &str,
    hints: &[PluginHint],
) -> Vec<CapabilityCandidate> {
    let mut out: Vec<CapabilityCandidate> = Vec::new();
    if !dir.is_dir() {
        return out;
    }

    // 1) MCP candidate first — it is the only shape Relux can activate in one click
    //    through the existing governed registry, so it leads.
    let mcp = mcp_candidate(dir, plugin_id, hints);
    let has_mcp = mcp.is_some();
    if let Some(c) = mcp {
        push(&mut out, c);
    }

    // 2) CLI commands — declared npm `bin`, Python `[project.scripts]`, Cargo `[[bin]]`.
    //    When an MCP candidate already covers this source, a declared npm `bin` is the
    //    MCP entrypoint (folded into the MCP candidate's command preview), so we do not
    //    also emit a duplicate CLI candidate for the npm bins.
    if !has_mcp {
        for c in npm_bin_candidates(dir) {
            push(&mut out, c);
        }
    }
    for c in python_script_candidates(dir) {
        push(&mut out, c);
    }
    for c in cargo_bin_candidates(dir) {
        push(&mut out, c);
    }

    out.truncate(MAX_CANDIDATES);
    out
}

/// Push a candidate unless the list is at the safety cap or an identical id is
/// already present (keeps the per-source list deduped + bounded).
fn push(out: &mut Vec<CapabilityCandidate>, c: CapabilityCandidate) {
    if out.len() >= MAX_CANDIDATES {
        return;
    }
    if out.iter().any(|e| e.id == c.id) {
        return;
    }
    out.push(c);
}

/// Read a bounded UTF-8 metadata file, or `None` if missing / too large / not UTF-8.
fn read_bounded(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Trim, clamp to `max` chars, and re-trim a display string.
fn clamp(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect::<String>().trim().to_string()
}

/// A slug-safe lowercase id fragment (`[a-z0-9-]`, collapsed dashes, bounded).
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in s.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Build the MCP candidate from the existing proposal, enriching an npm MCP-SDK
/// source (no config) into a one-click managed-stdio draft when a `bin` entrypoint is
/// declared. Returns `None` when the source carries no MCP signal.
fn mcp_candidate(
    dir: &Path,
    plugin_id: &str,
    hints: &[PluginHint],
) -> Option<CapabilityCandidate> {
    let mut proposal = propose_mcp_registration(dir, plugin_id, hints)?;

    // Enrich: an MCP source whose wiring could not be inferred from a config file
    // (npm `@modelcontextprotocol/sdk` with a declared `bin`, but no `mcp.json`) can
    // still be registered one-click as a managed-stdio server that runs the declared
    // entrypoint. The bin is the source's OWN declared entrypoint — surfaced for
    // review, never run by detection; managed-stdio re-validates + spawns it argv-only
    // only after the operator registers and Discovers.
    if proposal.detected_command.is_none() && proposal.suggested_endpoint.is_none() {
        if let Some((bin_name, bin_path)) = first_npm_bin(dir) {
            proposal.suggested_transport = "managed_stdio".to_string();
            proposal.detected_command = Some("node".to_string());
            proposal.detected_args = vec![clamp(&bin_path, MAX_PREVIEW_CHARS)];
            proposal.notes.insert(
                0,
                format!(
                    "Pre-filled a managed-stdio draft from the declared bin '{}' ({}). Relux runs \
                     it only after you register + Discover — argv-only, never a shell, never on \
                     import. Confirm node is the right launcher and the path is correct.",
                    clamp(&bin_name, 80),
                    clamp(&bin_path, 80)
                ),
            );
        }
    }

    let env_placeholders = mcp_config_env_names(dir);

    let (kind, title, confidence) = if proposal.suggested_transport == "managed_stdio" {
        let conf = if proposal.detected_command.is_some() { "high" } else { "medium" };
        ("mcp_stdio", "MCP server (stdio)", conf)
    } else if proposal.suggested_endpoint.is_some() {
        ("mcp_http", "MCP server (loopback HTTP)", "high")
    } else {
        // SDK/dependency signal, but no command or endpoint could be inferred.
        ("mcp_http", "MCP server (loopback HTTP)", "medium")
    };

    let command_preview = proposal.detected_command.as_ref().map(|cmd| {
        let mut s = cmd.clone();
        for a in &proposal.detected_args {
            s.push(' ');
            s.push_str(a);
        }
        clamp(&s, MAX_PREVIEW_CHARS)
    });

    let rationale = hints
        .iter()
        .find(|h| h.kind == "mcp-server" || h.kind == "mcp-config")
        .map(|h| h.detail.clone())
        .unwrap_or_else(|| "an MCP server signal was detected in the source".to_string());

    let mut next_steps = vec![
        "Open the pre-filled \"Register MCP server…\" review form, confirm/edit the fields, and \
         submit to the existing loopback registry."
            .to_string(),
        "After registering, click Discover to run a live tools/list through the gate."
            .to_string(),
        "Each discovered tool stays gated (needs approval) until you classify it — Relux never \
         auto-enables a tool."
            .to_string(),
    ];
    if !env_placeholders.is_empty() {
        next_steps.insert(
            1,
            format!(
                "Store a secret for each expected env var ({}) and map ENV_VAR=secret_name in the \
                 review form — names only, never plaintext values.",
                env_placeholders.join(", ")
            ),
        );
    }

    Some(CapabilityCandidate {
        id: "mcp-server".to_string(),
        kind: kind.to_string(),
        title: title.to_string(),
        confidence: confidence.to_string(),
        risk: "medium".to_string(),
        rationale,
        command_preview,
        env_placeholders,
        activation: "mcp_register".to_string(),
        mcp_registration: Some(proposal),
        command_tool: None,
        next_steps,
    })
}

/// The first declared `bin` of a top-level `package.json` as `(name, path)`, when
/// present. A string `bin` is keyed by the package name.
fn first_npm_bin(dir: &Path) -> Option<(String, String)> {
    let json: serde_json::Value = serde_json::from_str(&read_bounded(&dir.join("package.json"))?).ok()?;
    let pkg_name = json.get("name").and_then(|v| v.as_str()).unwrap_or("bin");
    match json.get("bin") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            Some((pkg_name.to_string(), s.to_string()))
        }
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .find_map(|(k, v)| v.as_str().map(|p| (k.to_string(), p.to_string())))
            .filter(|(_, p)| !p.trim().is_empty()),
        _ => None,
    }
}

/// All declared npm `bin` entries → one `cli_command` candidate each (honest pending:
/// Relux has no generic CLI runtime, so activation is `manual`).
fn npm_bin_candidates(dir: &Path) -> Vec<CapabilityCandidate> {
    let Some(json) =
        read_bounded(&dir.join("package.json")).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return Vec::new();
    };
    let pkg_name = json.get("name").and_then(|v| v.as_str()).unwrap_or("bin");
    let mut entries: Vec<(String, String)> = Vec::new();
    match json.get("bin") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            entries.push((pkg_name.to_string(), s.to_string()));
        }
        Some(serde_json::Value::Object(map)) => {
            for (k, v) in map.iter().take(8) {
                if let Some(p) = v.as_str() {
                    if !p.trim().is_empty() {
                        entries.push((k.to_string(), p.to_string()));
                    }
                }
            }
        }
        _ => {}
    }
    entries
        .into_iter()
        .map(|(name, path)| cli_candidate(
            &format!("cli-bin-{}", slug(&name)),
            "Command-line tool (npm bin)",
            "medium",
            format!("package.json declares a bin entrypoint '{}' → {}", clamp(&name, 80), clamp(&path, 80)),
            Some(clamp(&format!("node {path}"), MAX_PREVIEW_CHARS)),
        ))
        .collect()
}

/// Python console scripts from `[project.scripts]` / `[tool.poetry.scripts]` → one
/// `cli_command` candidate each (manual activation).
fn python_script_candidates(dir: &Path) -> Vec<CapabilityCandidate> {
    let Some(text) = read_bounded(&dir.join("pyproject.toml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, target) in toml_table_entries(&text, &["project.scripts", "tool.poetry.scripts"]) {
        out.push(cli_candidate(
            &format!("py-script-{}", slug(&name)),
            "Command-line tool (Python script)",
            "medium",
            format!("pyproject.toml declares a console script '{}' → {}", clamp(&name, 80), clamp(&target, 80)),
            Some(clamp(&name, MAX_PREVIEW_CHARS)),
        ));
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Cargo binary targets (`[[bin]]` names, else the `[package]` name) → one
/// `cli_command` candidate each. Build-required, so confidence is `low`.
fn cargo_bin_candidates(dir: &Path) -> Vec<CapabilityCandidate> {
    let Some(text) = read_bounded(&dir.join("Cargo.toml")) else {
        return Vec::new();
    };
    let bins = cargo_bin_names(&text);
    bins.into_iter()
        .take(8)
        .map(|name| cli_candidate(
            &format!("cargo-bin-{}", slug(&name)),
            "Command-line tool (Rust binary)",
            "low",
            format!("Cargo.toml declares a binary target '{}' (build required before it can run)", clamp(&name, 80)),
            Some(clamp(&format!("cargo run --bin {name}"), MAX_PREVIEW_CHARS)),
        ))
        .collect()
}

/// Construct a `cli_command` candidate. When a concrete `command_preview` could be
/// inferred, the candidate is a **`command_tool`** activation: it carries a safe,
/// pre-filled argv [`CommandToolProposal`] for the
/// `POST /v1/relux/plugins/:id/command-tools` review form (detection runs nothing —
/// the operator confirms/edits, and the resulting tool always requires approval to
/// invoke). With no inferable command it falls back to an honest `manual` record.
fn cli_candidate(
    id: &str,
    title: &str,
    confidence: &str,
    rationale: String,
    command_preview: Option<String>,
) -> CapabilityCandidate {
    let proposal = command_preview
        .as_deref()
        .and_then(|p| command_tool_proposal(id, title, p));
    let activation = if proposal.is_some() { "command_tool" } else { "manual" };
    let next_steps = if proposal.is_some() {
        vec![
            "Click Configure to open a pre-filled, reviewable command-tool form — \
             nothing is stored or run until you confirm."
                .to_string(),
            "Confirm the program (argv[0]) and args; Relux runs them argv-only (never a \
             shell), confined to this plugin's install directory."
                .to_string(),
            "The configured tool always requires approval to invoke, with bounded, \
             redacted output and a hard timeout — it never runs silently."
                .to_string(),
            "Prefer an MCP/stdio interface (register it on the MCP page) or a real \
             relux-plugin.json when the source supports one."
                .to_string(),
        ]
    } else {
        vec![
            "No concrete command could be inferred. If it can expose an MCP/stdio \
             interface, register it on the MCP page (managed-stdio) so its tools flow \
             through the gate."
                .to_string(),
            "Otherwise run it yourself as a loopback HTTP server, then add a tool \
             definition on this plugin and point a loopback runtime at it."
                .to_string(),
            "Or author a relux-plugin.json from the manifest template and re-install \
             for a first-class plugin."
                .to_string(),
        ]
    };
    CapabilityCandidate {
        id: id.to_string(),
        kind: "cli_command".to_string(),
        title: title.to_string(),
        confidence: confidence.to_string(),
        risk: "medium".to_string(),
        rationale,
        command_preview,
        env_placeholders: Vec::new(),
        activation: activation.to_string(),
        mcp_registration: None,
        command_tool: proposal,
        next_steps,
    }
}

/// Split a non-secret `command_preview` (e.g. `node ./dist/server.js`,
/// `cargo run --bin x`, `serve`) into a reviewable `(program, args)` argv draft and a
/// suggested tool name derived from the candidate id. Returns `None` when no program
/// token can be inferred. This is display text — detection never runs it.
fn command_tool_proposal(id: &str, title: &str, preview: &str) -> Option<CommandToolProposal> {
    let mut tokens = preview.split_whitespace();
    let program = tokens.next()?.to_string();
    if program.is_empty() {
        return None;
    }
    let args: Vec<String> = tokens
        .map(|t| clamp(t, MAX_PREVIEW_CHARS))
        .filter(|t| !t.is_empty())
        .collect();
    // Suggest a dotted tool name from the candidate id's descriptive tail (the part
    // after the `cli-bin-`/`py-script-`/`cargo-bin-` prefix), giving a `.run` verb so
    // it derives a clean permission. The submit endpoint re-sanitizes it.
    let tail = id
        .strip_prefix("cli-bin-")
        .or_else(|| id.strip_prefix("py-script-"))
        .or_else(|| id.strip_prefix("cargo-bin-"))
        .unwrap_or(id);
    let stem = slug(tail);
    let tool_name = if stem.is_empty() {
        "command.run".to_string()
    } else {
        format!("{stem}.run")
    };
    Some(CommandToolProposal {
        tool_name,
        program: clamp(&program, MAX_PREVIEW_CHARS),
        args,
        cwd: None,
        description: format!("{title} (configured from a detected entrypoint)"),
    })
}

/// Required env var NAMES from a standalone MCP config file's first server `env` map
/// (names only — never values). Bounded; empty when none.
fn mcp_config_env_names(dir: &Path) -> Vec<String> {
    for file in ["mcp.json", ".mcp.json", "mcp-config.json", "mcp_config.json"] {
        let Some(text) = read_bounded(&dir.join(file)) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let map = json.get("mcpServers").or_else(|| json.get("servers"));
        let cfg = match map {
            Some(serde_json::Value::Object(m)) => match m.values().next() {
                Some(v) => v,
                None => continue,
            },
            _ => &json,
        };
        if let Some(serde_json::Value::Object(env)) = cfg.get("env") {
            let mut names: Vec<String> = env
                .keys()
                .filter(|k| is_env_var_name(k))
                .take(MAX_ENV_PLACEHOLDERS)
                .map(|k| k.to_string())
                .collect();
            names.dedup();
            if !names.is_empty() {
                return names;
            }
        }
    }
    Vec::new()
}

/// POSIX-style env var name (mirrors `relux_core::is_valid_env_var_name`).
fn is_env_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Collect `key = "value"` entries under any of the given dotted TOML table headers,
/// without a TOML dependency. A lightweight line scan: find a `[header]` line, then
/// read `key = "value"` lines until the next `[` header. Bounded by the caller.
fn toml_table_entries(text: &str, headers: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            // A new table header: are we entering one of the wanted sections?
            let name = line.trim_start_matches('[').trim_end_matches(']').trim();
            in_section = headers.iter().any(|h| name.eq_ignore_ascii_case(h));
            continue;
        }
        if !in_section || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().trim_matches('"').trim_matches('\'').trim();
            let val = v.trim().trim_matches('"').trim_matches('\'').trim();
            if !key.is_empty() && !val.is_empty() {
                out.push((key.to_string(), val.to_string()));
            }
        }
    }
    out
}

/// Binary target names from a `Cargo.toml`: every `[[bin]]` block's `name`, else the
/// `[package]` `name` when an `src/main.rs`-style single binary is the convention.
/// Lightweight line scan (no TOML dependency); bounded by the caller.
fn cargo_bin_names(text: &str) -> Vec<String> {
    let mut bins: Vec<String> = Vec::new();
    let mut package_name: Option<String> = None;
    let mut section = "";
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            section = if line.starts_with("[[bin]]") {
                "bin"
            } else if line.starts_with("[package]") {
                "package"
            } else {
                "other"
            };
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() != "name" {
                continue;
            }
            let name = v.trim().trim_matches('"').trim_matches('\'').trim().to_string();
            if name.is_empty() {
                continue;
            }
            match section {
                "bin" if !bins.contains(&name) => bins.push(name),
                "bin" => {}
                "package" => package_name = Some(name),
                _ => {}
            }
        }
    }
    if bins.is_empty() {
        if let Some(name) = package_name {
            bins.push(name);
        }
    }
    bins
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

    fn detect(dir: &Path, id: &str) -> Vec<CapabilityCandidate> {
        let hints = detect_hints(dir);
        detect_candidates(dir, id, &hints)
    }

    fn find<'a>(cs: &'a [CapabilityCandidate], kind: &str) -> Option<&'a CapabilityCandidate> {
        cs.iter().find(|c| c.kind == kind)
    }

    #[test]
    fn missing_dir_yields_no_candidates() {
        let d = tmp();
        assert!(detect_candidates(&d.path().join("nope"), "x", &[]).is_empty());
    }

    #[test]
    fn readme_only_repo_has_no_candidates() {
        let d = tmp();
        write(d.path(), "README.md", "# A tool\nDoes things by hand.\n");
        let cs = detect(d.path(), "readme-only");
        assert!(cs.is_empty(), "a README-only repo yields no runnable candidate: {cs:?}");
    }

    #[test]
    fn npm_mcp_sdk_with_bin_is_a_one_click_managed_stdio_candidate() {
        let d = tmp();
        write(
            d.path(),
            "package.json",
            r#"{"name":"cool-mcp","bin":{"cool":"./dist/server.js"},
               "dependencies":{"@modelcontextprotocol/sdk":"^1.0.0"}}"#,
        );
        let cs = detect(d.path(), "cool-mcp");
        let mcp = find(&cs, "mcp_stdio").expect("an npm MCP SDK + bin is a stdio candidate");
        assert_eq!(mcp.activation, "mcp_register", "MCP is the one-click governed path");
        assert_eq!(mcp.confidence, "high", "a declared bin gives a concrete command");
        let reg = mcp.mcp_registration.as_ref().expect("carries a pre-filled registration");
        assert_eq!(reg.suggested_transport, "managed_stdio");
        assert_eq!(reg.detected_command.as_deref(), Some("node"));
        assert_eq!(reg.detected_args, vec!["./dist/server.js".to_string()]);
        assert_eq!(mcp.command_preview.as_deref(), Some("node ./dist/server.js"));
        // The npm bin is folded into the MCP candidate, NOT duplicated as a CLI one.
        assert!(find(&cs, "cli_command").is_none(), "the bin is the MCP entrypoint, not a 2nd candidate");
    }

    #[test]
    fn mcp_config_env_keys_become_placeholders_not_values() {
        let d = tmp();
        write(
            d.path(),
            "mcp.json",
            r#"{"mcpServers":{"gh":{"command":"npx","args":["-y","@x/server-github"],
               "env":{"GITHUB_TOKEN":"ghp_secret_value_here"}}}}"#,
        );
        let cs = detect(d.path(), "gh");
        let mcp = find(&cs, "mcp_stdio").expect("a stdio config is a candidate");
        assert_eq!(mcp.env_placeholders, vec!["GITHUB_TOKEN".to_string()]);
        // The secret VALUE is never carried — only the name.
        assert!(
            !format!("{mcp:?}").contains("ghp_secret_value_here"),
            "a candidate must never carry a secret value"
        );
        assert_eq!(mcp.command_preview.as_deref(), Some("npx -y @x/server-github"));
    }

    #[test]
    fn pyproject_console_script_is_a_command_tool_candidate() {
        let d = tmp();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname=\"toolkit\"\n\n[project.scripts]\nserve = \"toolkit.cli:main\"\n",
        );
        let cs = detect(d.path(), "toolkit");
        let cli = find(&cs, "cli_command").expect("a console script is a CLI candidate");
        // A concrete preview ⇒ a governed command-tool activation (no longer a dead end).
        assert_eq!(cli.activation, "command_tool");
        assert_eq!(cli.confidence, "medium");
        assert_eq!(cli.command_preview.as_deref(), Some("serve"));
        assert!(cli.mcp_registration.is_none());
        let ct = cli.command_tool.as_ref().expect("carries a pre-filled argv draft");
        assert_eq!(ct.program, "serve");
        assert!(ct.args.is_empty());
        assert_eq!(ct.tool_name, "serve.run");
        assert!(!cli.next_steps.is_empty(), "a command-tool candidate still carries next steps");
    }

    #[test]
    fn cargo_bin_target_is_a_low_confidence_command_tool_candidate() {
        let d = tmp();
        write(
            d.path(),
            "Cargo.toml",
            "[package]\nname=\"mytool\"\nversion=\"0.1.0\"\n\n[[bin]]\nname=\"mytool-cli\"\npath=\"src/main.rs\"\n",
        );
        let cs = detect(d.path(), "mytool");
        let cli = find(&cs, "cli_command").expect("a cargo bin is a CLI candidate");
        assert_eq!(cli.activation, "command_tool");
        assert_eq!(cli.confidence, "low", "build-required ⇒ low confidence");
        assert_eq!(cli.command_preview.as_deref(), Some("cargo run --bin mytool-cli"));
        let ct = cli.command_tool.as_ref().expect("carries a pre-filled argv draft");
        assert_eq!(ct.program, "cargo");
        assert_eq!(ct.args, vec!["run", "--bin", "mytool-cli"]);
    }

    #[test]
    fn cargo_package_without_explicit_bin_falls_back_to_package_name() {
        let d = tmp();
        write(d.path(), "Cargo.toml", "[package]\nname=\"solo\"\nversion=\"0.1.0\"\n");
        let cs = detect(d.path(), "solo");
        let cli = find(&cs, "cli_command").expect("a crate with no [[bin]] still offers its package name");
        assert_eq!(cli.command_preview.as_deref(), Some("cargo run --bin solo"));
    }

    #[test]
    fn plain_npm_package_without_mcp_yields_a_manual_cli_candidate_per_bin() {
        let d = tmp();
        write(
            d.path(),
            "package.json",
            r#"{"name":"plain","bin":{"plain":"./cli.js"},"dependencies":{"left-pad":"1.0.0"}}"#,
        );
        let cs = detect(d.path(), "plain");
        assert!(find(&cs, "mcp_stdio").is_none(), "no MCP signal ⇒ no MCP candidate");
        let cli = find(&cs, "cli_command").expect("a declared bin is a CLI candidate");
        assert_eq!(cli.activation, "command_tool");
        assert_eq!(cli.command_preview.as_deref(), Some("node ./cli.js"));
        let ct = cli.command_tool.as_ref().expect("carries a pre-filled argv draft");
        assert_eq!(ct.program, "node");
        assert_eq!(ct.args, vec!["./cli.js"]);
    }

    #[test]
    fn candidate_count_is_bounded() {
        let d = tmp();
        // Many cargo bins must not blow past the cap.
        let mut toml = String::from("[package]\nname=\"x\"\nversion=\"0.1.0\"\n");
        for i in 0..50 {
            toml.push_str(&format!("\n[[bin]]\nname=\"b{i}\"\npath=\"src/b{i}.rs\"\n"));
        }
        write(d.path(), "Cargo.toml", &toml);
        let cs = detect(d.path(), "x");
        assert!(cs.len() <= MAX_CANDIDATES, "got {}", cs.len());
    }

    #[test]
    fn slug_is_filesystem_and_id_safe() {
        assert_eq!(slug("Cool MCP/Server!"), "cool-mcp-server");
        assert_eq!(slug("  ../etc/passwd "), "etc-passwd");
        assert_eq!(slug("@scope/pkg"), "scope-pkg");
    }
}
