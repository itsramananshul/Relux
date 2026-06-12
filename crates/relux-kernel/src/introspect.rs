//! Safe, read-only introspection of an imported plugin source.
//!
//! When an arbitrary GitHub repo / zip / folder is installed without a
//! `relux-plugin.json`, Relux scaffolds a metadata-only wrapper that declares NO
//! tools and runs nothing (see [`crate::plugin_install::scaffold_manifest`]). The
//! operator still needs to understand *what is in the source* so they can decide
//! how to wire it up - is it an MCP server? an npm package? a Python entrypoint?
//!
//! This module answers that by scanning the installed directory for **hints**.
//! Every hint is purely informational: it tells the operator what configuration
//! might be relevant. Crucially:
//!
//! - Nothing here ever executes repo content, spawns a process, or follows a
//!   command. It only reads a bounded set of well-known metadata files.
//! - A hint is never promoted into a runnable tool. Tools come only from a real
//!   `relux-plugin.json` or operator-authored tool definitions, never inferred.
//! - The scan is bounded: it reads the top level of the directory once, inspects
//!   a fixed allow-list of metadata files, and caps how much of each it reads.
//!
//! Reference grounding (`docs/reference-driven-development.md`, BINDING):
//! - `reference/openclaw-main/src/plugins/install-security-scan.ts`
//!   (`scanBundleInstallSource`) - static, filesystem-only inspection of an
//!   install source that surfaces findings without ever running the source.
//! - `reference/openclaw-main/src/plugins/manifest-tool-availability.ts`
//!   (`manifestConfigSignalPasses`) - availability/readiness is derived from
//!   config/auth *signals* and surfaced honestly, never faked into "ready".
//! - `reference/hermes-agent-main/hermes_cli/mcp_config.py` (`_MCP_PRESETS`,
//!   `_get_mcp_servers`) - an MCP server is a `{command, args}` config keyed by
//!   the `mcp_servers` map and the `@modelcontextprotocol/sdk` package; that is
//!   the signal we detect (and only *hint* at) here.

use std::fs;
use std::path::Path;

use serde::Serialize;

/// One read-only finding about an imported plugin source. Informational only -
/// Relux never turns a hint into a runnable tool and never executes the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginHint {
    /// A stable machine kind so the dashboard can group/badge hints, e.g.
    /// `"mcp-server"`, `"npm-package"`, `"python-entrypoint"`, `"container"`,
    /// `"scripts"`, `"readme"`. Never used to execute anything.
    pub kind: String,
    /// A short human label, e.g. "Possible MCP server".
    pub label: String,
    /// A one-line, non-secret detail (a file name, package name, or matched
    /// signal). Always bounded; never includes file contents wholesale.
    pub detail: String,
}

impl PluginHint {
    fn new(kind: &str, label: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            label: label.to_string(),
            detail: detail.into(),
        }
    }
}

/// The largest metadata file we will read while introspecting (bytes). Generous
/// for a `package.json`/`pyproject.toml`/README, but bounded so a hostile source
/// can never make us read an enormous file.
const MAX_FILE_BYTES: u64 = 256 * 1024;

/// The most hints we return for any one source - a safety cap so a pathological
/// tree can never produce an unbounded list.
const MAX_HINTS: usize = 32;

/// Detect read-only hints about what an imported plugin source contains.
///
/// `dir` is the installed directory of the plugin (already copied inside the
/// plugins root). Scanning is bounded and never executes anything. Returns an
/// empty vec for a source with no recognizable signals (an honest "nothing
/// detected"), or when `dir` does not exist.
pub fn detect_hints(dir: &Path) -> Vec<PluginHint> {
    let mut hints: Vec<PluginHint> = Vec::new();
    if !dir.is_dir() {
        return hints;
    }

    // A Relux manifest at the top level means this is already a real plugin, not
    // a metadata-only wrapper; note it honestly and skip the rest.
    if dir.join(crate::loader::MANIFEST_FILENAME).is_file() {
        push(
            &mut hints,
            PluginHint::new(
                "relux-manifest",
                "Relux manifest present",
                crate::loader::MANIFEST_FILENAME,
            ),
        );
    }

    detect_npm(dir, &mut hints);
    detect_python(dir, &mut hints);
    detect_mcp_config(dir, &mut hints);
    detect_container(dir, &mut hints);
    detect_rust(dir, &mut hints);
    detect_scripts(dir, &mut hints);
    detect_readme(dir, &mut hints);

    hints.truncate(MAX_HINTS);
    hints
}

/// Push a hint unless the list is already at the safety cap or an identical hint
/// (same kind + detail) is already present.
fn push(hints: &mut Vec<PluginHint>, hint: PluginHint) {
    if hints.len() >= MAX_HINTS {
        return;
    }
    if hints
        .iter()
        .any(|h| h.kind == hint.kind && h.detail == hint.detail)
    {
        return;
    }
    hints.push(hint);
}

/// Read a bounded UTF-8 file at `path`, or `None` if it is missing, too large, or
/// not valid UTF-8. Never reads more than [`MAX_FILE_BYTES`].
fn read_bounded(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// `package.json` -> an npm package; surface its name, declared `bin`
/// entrypoints, and (the strongest signal) whether it depends on the MCP SDK,
/// which marks it as a likely MCP server.
fn detect_npm(dir: &Path, hints: &mut Vec<PluginHint>) {
    let Some(text) = read_bounded(&dir.join("package.json")) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        // Present but unparseable - still an honest signal.
        push(
            hints,
            PluginHint::new("npm-package", "npm package", "package.json (unparsed)"),
        );
        return;
    };

    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.chars().take(120).collect::<String>())
        .unwrap_or_else(|| "package.json".to_string());
    push(
        hints,
        PluginHint::new("npm-package", "npm package", name.clone()),
    );

    // `bin` may be a string or an object map of name -> path.
    match json.get("bin") {
        Some(serde_json::Value::String(s)) => push(
            hints,
            PluginHint::new(
                "npm-bin",
                "npm executable",
                s.chars().take(120).collect::<String>(),
            ),
        ),
        Some(serde_json::Value::Object(map)) => {
            for key in map.keys().take(6) {
                push(
                    hints,
                    PluginHint::new(
                        "npm-bin",
                        "npm executable",
                        key.chars().take(120).collect::<String>(),
                    ),
                );
            }
        }
        _ => {}
    }

    // The MCP SDK in dependencies is the canonical "this is an MCP server" signal
    // (Hermes keys MCP servers by the @modelcontextprotocol/sdk package).
    if depends_on(&json, "@modelcontextprotocol/sdk") {
        push(
            hints,
            PluginHint::new(
                "mcp-server",
                "Possible MCP server",
                "depends on @modelcontextprotocol/sdk",
            ),
        );
    }
}

/// True if `@modelcontextprotocol/...` (or the exact package) appears in any of
/// the standard dependency maps of a parsed `package.json`.
fn depends_on(json: &serde_json::Value, pkg: &str) -> bool {
    for field in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(serde_json::Value::Object(map)) = json.get(field) {
            if map.keys().any(|k| k == pkg) {
                return true;
            }
        }
    }
    false
}

/// Python packaging metadata -> a Python package and/or entrypoint, and an MCP
/// server when an `mcp`/`modelcontextprotocol` dependency is named.
fn detect_python(dir: &Path, hints: &mut Vec<PluginHint>) {
    // pyproject.toml / setup.py / setup.cfg / requirements.txt are all "this is a
    // Python package" signals; we read them as text and scan for an MCP hint
    // without parsing TOML (no extra dependency, and we only need substrings).
    let mut named_package = false;
    for (file, label) in [
        ("pyproject.toml", "Python package (pyproject.toml)"),
        ("setup.py", "Python package (setup.py)"),
        ("setup.cfg", "Python package (setup.cfg)"),
    ] {
        if let Some(text) = read_bounded(&dir.join(file)) {
            push(hints, PluginHint::new("python-package", label, file));
            named_package = true;
            if mentions_mcp(&text) {
                push(
                    hints,
                    PluginHint::new(
                        "mcp-server",
                        "Possible MCP server",
                        format!("{file} references mcp"),
                    ),
                );
            }
        }
    }
    if let Some(text) = read_bounded(&dir.join("requirements.txt")) {
        if mentions_mcp(&text) {
            push(
                hints,
                PluginHint::new(
                    "mcp-server",
                    "Possible MCP server",
                    "requirements.txt references mcp",
                ),
            );
        }
    }

    // A top-level Python entrypoint (`__main__.py`, `main.py`, or `server.py`) is
    // a runnable-by-hand signal - purely a hint, never executed by Relux.
    for entry in ["__main__.py", "main.py", "server.py"] {
        if dir.join(entry).is_file() {
            push(
                hints,
                PluginHint::new("python-entrypoint", "Python entrypoint", entry),
            );
            named_package = true;
        }
    }
    let _ = named_package;
}

/// Scan packaging text for an MCP signal. Lowercased, bounded. Matches the SDK
/// names (`modelcontextprotocol`, `fastmcp`, `mcp.server`/`mcp-server`) and a
/// standalone `mcp` dependency token (e.g. `mcp>=1.0`, `"mcp"`, a bare `mcp`
/// line) without matching `mcp` embedded inside a longer word.
fn mentions_mcp(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("modelcontextprotocol")
        || lower.contains("mcp-server")
        || lower.contains("mcp.server")
        || lower.contains("fastmcp")
    {
        return true;
    }
    // A standalone `mcp` token: "mcp" with a non-alphanumeric boundary on each
    // side (a version specifier, quote, bracket, whitespace, or end of string).
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find("mcp") {
        let start = from + rel;
        let end = start + 3;
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// A standalone MCP config file (`mcp.json`, `.mcp.json`, `mcp-config.json`,
/// `mcp_config.json`) is a direct "wire this up as an MCP server" signal.
fn detect_mcp_config(dir: &Path, hints: &mut Vec<PluginHint>) {
    for file in ["mcp.json", ".mcp.json", "mcp-config.json", "mcp_config.json"] {
        if dir.join(file).is_file() {
            push(
                hints,
                PluginHint::new("mcp-config", "MCP config file", file),
            );
        }
    }
}

/// A `Dockerfile`/`compose` file -> the source ships as a container. A hint only;
/// Relux never builds or runs it.
fn detect_container(dir: &Path, hints: &mut Vec<PluginHint>) {
    for file in [
        "Dockerfile",
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ] {
        if dir.join(file).is_file() {
            push(hints, PluginHint::new("container", "Container image", file));
        }
    }
}

/// A `Cargo.toml` -> the source is a Rust crate.
fn detect_rust(dir: &Path, hints: &mut Vec<PluginHint>) {
    if dir.join("Cargo.toml").is_file() {
        push(
            hints,
            PluginHint::new("rust-crate", "Rust crate", "Cargo.toml"),
        );
    }
}

/// Build/automation scripts (`Makefile`, `Justfile`, top-level `*.sh`) -> there
/// are scripts an operator might run by hand. A hint only.
fn detect_scripts(dir: &Path, hints: &mut Vec<PluginHint>) {
    for file in ["Makefile", "makefile", "Justfile", "justfile"] {
        if dir.join(file).is_file() {
            push(hints, PluginHint::new("scripts", "Build scripts", file));
        }
    }
    // Up to a few top-level shell scripts, named honestly (their existence, not
    // their content). Reading the directory once keeps the scan bounded.
    if let Ok(read) = fs::read_dir(dir) {
        let mut count = 0;
        for entry in read.flatten() {
            if count >= 4 {
                break;
            }
            let path = entry.path();
            let is_sh = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("sh"))
                .unwrap_or(false);
            if path.is_file() && is_sh {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    push(
                        hints,
                        PluginHint::new("scripts", "Shell script", name.to_string()),
                    );
                    count += 1;
                }
            }
        }
    }
}

/// A README -> note its presence (the wrapper already uses its first line as the
/// description). Operators read it to learn the real wiring; we only flag it.
fn detect_readme(dir: &Path, hints: &mut Vec<PluginHint>) {
    for file in ["README.md", "README", "README.txt", "readme.md", "Readme.md"] {
        if dir.join(file).is_file() {
            push(hints, PluginHint::new("readme", "Readme present", file));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    fn has(hints: &[PluginHint], kind: &str) -> bool {
        hints.iter().any(|h| h.kind == kind)
    }

    #[test]
    fn empty_source_yields_no_hints() {
        let d = tmp();
        assert!(detect_hints(d.path()).is_empty());
    }

    #[test]
    fn missing_dir_yields_no_hints() {
        let d = tmp();
        let missing = d.path().join("nope");
        assert!(detect_hints(&missing).is_empty());
    }

    #[test]
    fn npm_package_with_mcp_sdk_is_detected_as_mcp_server() {
        let d = tmp();
        write(
            d.path(),
            "package.json",
            r#"{"name":"cool-mcp","bin":{"cool":"./bin.js"},
               "dependencies":{"@modelcontextprotocol/sdk":"^1.0.0"}}"#,
        );
        let hints = detect_hints(d.path());
        assert!(has(&hints, "npm-package"));
        assert!(has(&hints, "npm-bin"));
        assert!(
            has(&hints, "mcp-server"),
            "the MCP SDK dependency must surface an mcp-server hint: {hints:?}"
        );
        // The package name is surfaced, not invented.
        assert!(hints
            .iter()
            .any(|h| h.kind == "npm-package" && h.detail == "cool-mcp"));
    }

    #[test]
    fn npm_package_without_mcp_is_just_a_package() {
        let d = tmp();
        write(
            d.path(),
            "package.json",
            r#"{"name":"plain","dependencies":{"left-pad":"1.0.0"}}"#,
        );
        let hints = detect_hints(d.path());
        assert!(has(&hints, "npm-package"));
        assert!(!has(&hints, "mcp-server"), "no MCP signal => no MCP hint");
    }

    #[test]
    fn unparseable_package_json_still_hints_npm() {
        let d = tmp();
        write(d.path(), "package.json", "{ not valid json ");
        let hints = detect_hints(d.path());
        assert!(has(&hints, "npm-package"));
    }

    #[test]
    fn python_package_and_entrypoint_and_mcp_are_detected() {
        let d = tmp();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname=\"x\"\ndependencies=[\"mcp>=1.0\"]\n",
        );
        write(d.path(), "__main__.py", "print('hi')\n");
        let hints = detect_hints(d.path());
        assert!(has(&hints, "python-package"));
        assert!(has(&hints, "python-entrypoint"));
        assert!(
            has(&hints, "mcp-server"),
            "an mcp dependency in pyproject must hint mcp-server: {hints:?}"
        );
    }

    #[test]
    fn standalone_mcp_config_is_detected() {
        let d = tmp();
        write(d.path(), "mcp.json", "{\"mcpServers\":{}}");
        let hints = detect_hints(d.path());
        assert!(has(&hints, "mcp-config"));
    }

    #[test]
    fn container_rust_scripts_and_readme_are_detected() {
        let d = tmp();
        write(d.path(), "Dockerfile", "FROM scratch\n");
        write(d.path(), "Cargo.toml", "[package]\nname=\"x\"\n");
        write(d.path(), "build.sh", "#!/bin/sh\necho hi\n");
        write(d.path(), "README.md", "# Title\nbody\n");
        let hints = detect_hints(d.path());
        assert!(has(&hints, "container"));
        assert!(has(&hints, "rust-crate"));
        assert!(has(&hints, "scripts"));
        assert!(has(&hints, "readme"));
    }

    #[test]
    fn a_relux_manifest_is_noted() {
        let d = tmp();
        write(d.path(), crate::loader::MANIFEST_FILENAME, "{}");
        let hints = detect_hints(d.path());
        assert!(has(&hints, "relux-manifest"));
    }

    #[test]
    fn an_oversized_metadata_file_is_not_read() {
        let d = tmp();
        // A package.json larger than the bound: it is skipped (no npm hint), and
        // the scan never reads it wholesale.
        let big = "x".repeat((MAX_FILE_BYTES as usize) + 1);
        write(d.path(), "package.json", &big);
        let hints = detect_hints(d.path());
        assert!(
            !has(&hints, "npm-package"),
            "an oversized package.json is skipped, not read"
        );
    }

    #[test]
    fn hint_count_is_bounded() {
        let d = tmp();
        // Many shell scripts must not blow past the per-source cap.
        for i in 0..100 {
            write(d.path(), &format!("s{i}.sh"), "#!/bin/sh\n");
        }
        let hints = detect_hints(d.path());
        assert!(hints.len() <= MAX_HINTS, "got {}", hints.len());
    }
}
