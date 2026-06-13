//! Read-only, path-confined source introspection for installed plugins ("Plugin Lens").
//!
//! The product contract (`docs/plugins.md` "Plugin Lens (read-only source capabilities)"):
//! *if a thing is installed as a plugin, Prime must be able to discover it and use it
//! somehow.* A normal GitHub repo / ZIP / local folder does not ship a `relux-plugin.json`,
//! so a manifestless install scaffolds a metadata-only manifest with an EMPTY
//! `capabilities.tools` (`crate::plugin_install::scaffold_manifest`). Before this module that
//! left a dead row: visible, but with nothing Prime could invoke.
//!
//! This module gives EVERY non-bundled installed plugin four real, runnable, Prime-visible,
//! path-safe, audited read-only capabilities — modelled on Hermes `skill_view`/`skills_list`
//! (progressive disclosure scoped to the installed source dir) and OpenClaw's
//! `discoverOpenClawPlugins` path-confined source discovery
//! (`docs/reference-driven-development.md` "universal read-only plugin-source capabilities"):
//!
//! - `plugin.summary`   — what this plugin is: manifest metadata + detected hints + README
//!   excerpt + file/dir counts.
//! - `plugin.inspect`   — a bounded file tree with sizes.
//! - `plugin.search`    — a bounded text search over the source.
//! - `plugin.read_file` — one bounded UTF-8 text file, path-confined.
//!
//! SAFETY: nothing here executes the plugin. There is no spawn, no network, no write — only
//! bounded reads of bytes already copied into the plugin's install dir, confined to that dir
//! by [`resolve_within`]. This honors `docs/RELUX_MASTER_PLAN.md` §8.2/§18 ("no shelling out,
//! no side effects *from* installed plugins"): reading copied bytes is not running the plugin.
//! The tools are `RiskLevel::Low` + `ApprovalRequirement::Never`, so they are directly
//! runnable, but they still require the single `plugin:source:read` capability (granted to
//! Prime at bootstrap) and route through the unchanged `invoke_tool` permission/audit gate.

use std::path::{Component, Path, PathBuf};

use relux_core::permission::{ApprovalRequirement, RiskLevel};
use serde::Serialize;

/// The single capability every source tool requires. One flat, individually-revocable grant
/// that authorizes read-only source introspection across ALL installed plugins — least
/// privilege at the right granularity (a uniformly safe, read-only operation), so Prime does
/// not need a separate per-plugin grant to answer "what is this installed plugin?".
pub const SOURCE_READ_PERMISSION: &str = "plugin:source:read";

/// One synthetic read-only source tool.
pub struct SourceToolSpec {
    /// The bare tool name (e.g. `plugin.summary`).
    pub name: &'static str,
    /// A one-line description shown in Prime's catalogue + the dashboard.
    pub description: &'static str,
}

/// The four read-only source tools attached to every non-bundled installed plugin.
pub const SOURCE_TOOLS: &[SourceToolSpec] = &[
    SourceToolSpec {
        name: "plugin.summary",
        description: "Summarize what this installed plugin is: manifest metadata, detected signals (MCP/CLI/npm/python/etc.), a README excerpt, and file/directory counts. Read-only.",
    },
    SourceToolSpec {
        name: "plugin.inspect",
        description: "List this installed plugin's files as a bounded tree with sizes. Optional args: {\"path\":\"subdir\",\"max_entries\":N}. Read-only.",
    },
    SourceToolSpec {
        name: "plugin.search",
        description: "Search this installed plugin's text files for a string. Args: {\"query\":\"...\",\"max_matches\":N}. Read-only.",
    },
    SourceToolSpec {
        name: "plugin.read_file",
        description: "Read one UTF-8 text file from this installed plugin. Args: {\"path\":\"relative/path\",\"max_bytes\":N}. Path-confined to the plugin dir; read-only.",
    },
];

/// True when `name` is one of the synthetic read-only source tools.
pub fn is_source_tool(name: &str) -> bool {
    SOURCE_TOOLS.iter().any(|t| t.name == name)
}

/// The spec for a source tool by name, if any.
pub fn source_tool_spec(name: &str) -> Option<&'static SourceToolSpec> {
    SOURCE_TOOLS.iter().find(|t| t.name == name)
}

/// The risk every source tool declares — `Low`, because it is read-only.
pub fn source_risk() -> RiskLevel {
    RiskLevel::Low
}

/// The approval every source tool declares — `Never`, because it is read-only and confined.
pub fn source_approval() -> ApprovalRequirement {
    ApprovalRequirement::Never
}

// --- Bounds ---------------------------------------------------------------------------------
//
// Every read is bounded so a hostile or merely huge source can never exhaust memory or hang
// the kernel lock. These are generous for inspecting a real plugin but hard caps.

/// Max bytes returned from a single `plugin.read_file` (the caller may lower it via `max_bytes`).
const MAX_READ_BYTES: usize = 64 * 1024;
/// Largest file `plugin.search` will open + scan; bigger files are skipped (counted, not read).
const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
/// Default + ceiling for `plugin.inspect` tree entries.
const DEFAULT_TREE_ENTRIES: usize = 200;
const MAX_TREE_ENTRIES: usize = 1_000;
/// Default + ceiling for `plugin.search` matches.
const DEFAULT_SEARCH_MATCHES: usize = 50;
const MAX_SEARCH_MATCHES: usize = 500;
/// Max directory depth any walk descends.
const MAX_DEPTH: usize = 12;
/// Max characters kept from a matched line (so a minified line cannot bloat output).
const MAX_LINE_CHARS: usize = 240;
/// Max characters of README returned in a summary.
const MAX_README_CHARS: usize = 1_400;
/// Total files/dirs any single walk will visit before stopping (hard ceiling).
const MAX_WALK_VISITS: usize = 20_000;

/// Directory names skipped while walking for inspect/search/summary — noisy, huge, and never
/// the interesting source. A specific file inside one can still be read via `plugin.read_file`
/// (path-confined), so nothing is hidden, just kept out of the bounded scans.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
    "build",
];

/// An error from a read-only source operation. Mapped by the kernel to a clean
/// `ToolRuntimeInvocation` failure — never a fabricated result.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SourceError {
    #[error("the plugin install directory does not exist or is not a directory")]
    MissingInstallDir,
    #[error("required argument '{0}' is missing or empty")]
    MissingArg(&'static str),
    #[error("path '{0}' escapes the plugin directory or is not allowed")]
    PathEscape(String),
    #[error("path '{0}' was not found in the plugin")]
    NotFound(String),
    #[error("'{0}' is not a regular file")]
    NotAFile(String),
    #[error("'{0}' is not a directory")]
    NotADir(String),
    #[error("'{0}' is not a UTF-8 text file")]
    NotText(String),
    #[error("read failed: {0}")]
    Io(String),
}

/// Resolve a caller-supplied RELATIVE path inside `base`, fail-closed against traversal.
///
/// Mirrors OpenClaw's `checkSourceEscapesRoot`: rejects absolute paths and any `..`/root
/// component up front, then canonicalizes both sides and requires the resolved path to stay
/// within the canonical base — so a symlink that points outside the install dir is rejected
/// too. The target must exist (canonicalize requires it), which is correct here: every op
/// resolves a concrete file/dir.
pub fn resolve_within(base: &Path, rel: &str) -> Result<PathBuf, SourceError> {
    let rel = rel.trim();
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(SourceError::PathEscape(rel.to_string()));
    }
    // Only ordinary path components (and a leading `./`) are allowed — no `..`, no root, no
    // Windows drive prefix.
    for comp in rel_path.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(SourceError::PathEscape(rel.to_string())),
        }
    }
    let base_can = base
        .canonicalize()
        .map_err(|_| SourceError::MissingInstallDir)?;
    let joined = base_can.join(rel_path);
    let joined_can = joined
        .canonicalize()
        .map_err(|_| SourceError::NotFound(rel.to_string()))?;
    if !joined_can.starts_with(&base_can) {
        return Err(SourceError::PathEscape(rel.to_string()));
    }
    Ok(joined_can)
}

/// True when `name` is a skipped noisy directory.
fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// Read the trimmed string at `key` from an args object, if present and non-empty.
fn arg_str<'a>(input: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Read a bounded `usize` at `key`, clamped to `[1, ceiling]`, defaulting to `default`.
fn arg_count(input: &serde_json::Value, key: &str, default: usize, ceiling: usize) -> usize {
    input
        .get(key)
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(1, ceiling))
        .unwrap_or(default)
}

/// One entry in an `plugin.inspect` tree.
#[derive(Debug, Clone, Serialize)]
struct TreeEntry {
    /// Path relative to the plugin install dir, using `/` separators.
    path: String,
    /// `"file"` or `"dir"`.
    kind: &'static str,
    /// File size in bytes (omitted for directories).
    #[serde(skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
}

/// Normalize a path relative to `base` into a forward-slashed display string.
fn rel_display(base: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(base).unwrap_or(p);
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `plugin.inspect`: a bounded, depth-limited file tree with sizes, scoped to an optional
/// sub-path. Symlinked directories are listed but never descended into. Read-only.
pub fn inspect(install_dir: &Path, input: &serde_json::Value) -> Result<serde_json::Value, SourceError> {
    let base = install_dir
        .canonicalize()
        .map_err(|_| SourceError::MissingInstallDir)?;
    let start = match arg_str(input, "path") {
        Some(rel) => resolve_within(&base, rel)?,
        None => base.clone(),
    };
    if !start.is_dir() {
        return Err(SourceError::NotADir(rel_display(&base, &start)));
    }
    let max_entries = arg_count(input, "max_entries", DEFAULT_TREE_ENTRIES, MAX_TREE_ENTRIES);

    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut visits = 0usize;
    let mut truncated = false;
    // Iterative DFS so depth is explicit and bounded.
    let mut stack: Vec<(PathBuf, usize)> = vec![(start.clone(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            continue;
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut children: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        children.sort();
        for child in children {
            visits += 1;
            if visits > MAX_WALK_VISITS {
                truncated = true;
                break;
            }
            if entries.len() >= max_entries {
                truncated = true;
                break;
            }
            // Use symlink metadata so a symlinked dir is reported as a file-ish leaf and never
            // descended (path-confinement defence in depth).
            let meta = match std::fs::symlink_metadata(&child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if meta.is_dir() {
                // List the directory either way; only descend into non-noisy ones.
                entries.push(TreeEntry {
                    path: format!("{}/", rel_display(&base, &child)),
                    kind: "dir",
                    size_bytes: None,
                });
                if !is_skipped_dir(&name) {
                    stack.push((child, depth + 1));
                }
            } else if meta.is_file() {
                entries.push(TreeEntry {
                    path: rel_display(&base, &child),
                    kind: "file",
                    size_bytes: Some(meta.len()),
                });
            }
        }
        if truncated {
            break;
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(serde_json::json!({
        "root": rel_display(&base, &start),
        "entry_count": entries.len(),
        "truncated": truncated,
        "entries": entries,
    }))
}

/// One `plugin.search` hit.
#[derive(Debug, Clone, Serialize)]
struct SearchMatch {
    path: String,
    line: usize,
    text: String,
}

/// Heuristic: treat the first bytes of a file as text iff valid UTF-8 with no NUL byte.
fn looks_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// `plugin.search`: a bounded, case-insensitive substring search across the plugin's text
/// files. Skips noisy dirs + files larger than [`MAX_SEARCH_FILE_BYTES`] + binary files.
pub fn search(install_dir: &Path, input: &serde_json::Value) -> Result<serde_json::Value, SourceError> {
    let base = install_dir
        .canonicalize()
        .map_err(|_| SourceError::MissingInstallDir)?;
    let query = arg_str(input, "query").ok_or(SourceError::MissingArg("query"))?;
    let needle = query.to_lowercase();
    let max_matches = arg_count(input, "max_matches", DEFAULT_SEARCH_MATCHES, MAX_SEARCH_MATCHES);

    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut visits = 0usize;
    let mut files_scanned = 0usize;
    let mut truncated = false;
    let mut stack: Vec<(PathBuf, usize)> = vec![(base.clone(), 0)];
    'walk: while let Some((dir, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            continue;
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut children: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        children.sort();
        for child in children {
            visits += 1;
            if visits > MAX_WALK_VISITS {
                truncated = true;
                break 'walk;
            }
            let meta = match std::fs::symlink_metadata(&child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                let name = child
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !is_skipped_dir(&name) {
                    stack.push((child, depth + 1));
                }
                continue;
            }
            if !meta.is_file() || meta.len() > MAX_SEARCH_FILE_BYTES {
                continue;
            }
            let bytes = match std::fs::read(&child) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if !looks_text(&bytes) {
                continue;
            }
            files_scanned += 1;
            let content = String::from_utf8_lossy(&bytes);
            let rel = rel_display(&base, &child);
            for (idx, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    matches.push(SearchMatch {
                        path: rel.clone(),
                        line: idx + 1,
                        text: clamp_chars(line.trim(), MAX_LINE_CHARS),
                    });
                    if matches.len() >= max_matches {
                        truncated = true;
                        break 'walk;
                    }
                }
            }
        }
    }
    Ok(serde_json::json!({
        "query": query,
        "match_count": matches.len(),
        "files_scanned": files_scanned,
        "truncated": truncated,
        "matches": matches,
    }))
}

/// `plugin.read_file`: read one UTF-8 text file inside the plugin, path-confined + bounded.
pub fn read_file(install_dir: &Path, input: &serde_json::Value) -> Result<serde_json::Value, SourceError> {
    let base = install_dir
        .canonicalize()
        .map_err(|_| SourceError::MissingInstallDir)?;
    let rel = arg_str(input, "path").ok_or(SourceError::MissingArg("path"))?;
    let resolved = resolve_within(&base, rel)?;
    let meta = std::fs::symlink_metadata(&resolved).map_err(|e| SourceError::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(SourceError::NotAFile(rel.to_string()));
    }
    let max_bytes = arg_count(input, "max_bytes", MAX_READ_BYTES, MAX_READ_BYTES);
    let bytes = std::fs::read(&resolved).map_err(|e| SourceError::Io(e.to_string()))?;
    let total = bytes.len();
    if !looks_text(&bytes[..bytes.len().min(8 * 1024)]) {
        return Err(SourceError::NotText(rel.to_string()));
    }
    let truncated = total > max_bytes;
    let slice = &bytes[..total.min(max_bytes)];
    let content = String::from_utf8_lossy(slice).into_owned();
    Ok(serde_json::json!({
        "path": rel_display(&base, &resolved),
        "total_bytes": total,
        "bytes_returned": slice.len(),
        "truncated": truncated,
        "content": content,
    }))
}

/// Metadata the caller (the kernel) supplies to [`summary`] from the installed-plugin record
/// + its manifest, so this module stays free of the kernel's plugin maps.
#[derive(Debug, Clone, Default)]
pub struct SummaryMeta {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub description: String,
    pub author: String,
    pub trust_level: String,
    pub source_kind: String,
    pub source_label: String,
    /// How many tools the manifest declares (0 for a manifestless scaffold).
    pub declared_tool_count: usize,
    /// True when this is a generated metadata-only wrapper (manifestless install).
    pub generated_manifest: bool,
}

/// `plugin.summary`: a high-level, read-only "what is this plugin?" — manifest metadata, the
/// read-only detected hints ([`crate::introspect::detect_hints`]), a README excerpt, and
/// bounded file/dir counts. The single most useful first call for "what can this plugin do?".
pub fn summary(
    install_dir: &Path,
    meta: &SummaryMeta,
    _input: &serde_json::Value,
) -> Result<serde_json::Value, SourceError> {
    let base = install_dir
        .canonicalize()
        .map_err(|_| SourceError::MissingInstallDir)?;

    // Bounded counts of files/dirs (skipping noisy dirs), and the top-level entries.
    let (file_count, dir_count, top_level, counts_truncated) = count_tree(&base);
    let hints = crate::introspect::detect_hints(&base);
    let readme = read_readme_excerpt(&base);

    Ok(serde_json::json!({
        "plugin_id": meta.plugin_id,
        "name": meta.name,
        "version": meta.version,
        "kind": meta.kind,
        "description": meta.description,
        "author": meta.author,
        "trust_level": meta.trust_level,
        "source_kind": meta.source_kind,
        "source_label": meta.source_label,
        "declared_tool_count": meta.declared_tool_count,
        "generated_manifest": meta.generated_manifest,
        "file_count": file_count,
        "dir_count": dir_count,
        "counts_truncated": counts_truncated,
        "top_level": top_level,
        "detected_hints": hints,
        "readme_excerpt": readme,
    }))
}

/// Bounded count of files + dirs (skipping noisy dirs) plus the sorted top-level entry names.
fn count_tree(base: &Path) -> (usize, usize, Vec<String>, bool) {
    let mut files = 0usize;
    let mut dirs = 0usize;
    let mut top_level: Vec<String> = Vec::new();
    let mut visits = 0usize;
    let mut truncated = false;
    let mut stack: Vec<(PathBuf, usize)> = vec![(base.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            continue;
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in read.flatten() {
            visits += 1;
            if visits > MAX_WALK_VISITS {
                truncated = true;
                break;
            }
            let child = entry.path();
            let meta = match std::fs::symlink_metadata(&child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if depth == 0 {
                top_level.push(if meta.is_dir() {
                    format!("{name}/")
                } else {
                    name.clone()
                });
            }
            if meta.is_dir() {
                dirs += 1;
                if !is_skipped_dir(&name) {
                    stack.push((child, depth + 1));
                }
            } else if meta.is_file() {
                files += 1;
            }
        }
        if truncated {
            break;
        }
    }
    top_level.sort();
    top_level.truncate(64);
    (files, dirs, top_level, truncated)
}

/// Read a bounded excerpt from a top-level README, if any. Empty string when none.
fn read_readme_excerpt(base: &Path) -> String {
    const CANDIDATES: &[&str] = &["README.md", "README", "README.txt", "readme.md"];
    for name in CANDIDATES {
        let p = base.join(name);
        if p.is_file() {
            if let Ok(bytes) = std::fs::read(&p) {
                if looks_text(&bytes[..bytes.len().min(8 * 1024)]) {
                    let text = String::from_utf8_lossy(&bytes);
                    return clamp_chars(text.trim(), MAX_README_CHARS);
                }
            }
        }
    }
    String::new()
}

/// Clamp a string to `max` chars, appending an ellipsis marker when truncated.
fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

// --- Natural-language shaping ---------------------------------------------------------------
//
// Every source tool returns a STRUCTURED JSON body (a tree, a match list, a file read, the
// summary fields). Handing that raw to Prime's chat surface or to the agent loop's brain
// observation is exactly the "raw implementation envelope in the chat bubble" the product
// forbids (`docs/RELUX_MASTER_PLAN.md` §10.5/§11.1, `docs/plugins.md` "Plugin Lens"). Instead,
// the kernel shapes each result into the Hermes `mcp_tool.py` `{ result, structuredContent }`
// envelope (`docs/reference-driven-development.md`): a human `result` SUMMARY of what was found
// rides up front (what the chat bubble + the brain read), and the full structured detail rides
// along in `structuredContent` (expandable / audited, never the primary view). Pure + lossless:
// the summary is derived from the structured value, never fabricated, and the structured value
// is preserved verbatim.

/// Wrap one source tool's structured output in the `{ result, structuredContent }` envelope.
/// `result` is the human-readable summary from [`humanize`]; `structuredContent` is the original
/// value. This is the single chokepoint the kernel uses for every Plugin Lens result, so the chat
/// single-invoke path, the agent loop, and the dashboard all see clean prose.
///
/// SAFETY (redaction parity, `docs/RELUX_MASTER_PLAN.md` §11.1): both halves are secret-scrubbed
/// before they leave the kernel. A source file body folded into a `plugin.read_file` summary, or
/// a `plugin.search` hit, can carry a credential the user committed; Prime must never splash that
/// into chat. The human `result` runs through [`relux_core::redact_secrets`] and the structured
/// detail through [`relux_core::redact_json`] (key-aware deep scrub), so neither the natural answer
/// nor the expandable "raw details" expander can leak an obvious secret. The structure is
/// otherwise preserved verbatim — redaction only masks key-shaped tokens.
pub fn shape_result(tool_name: &str, value: serde_json::Value) -> serde_json::Value {
    let result = relux_core::redact_secrets(&humanize(tool_name, &value));
    let structured = relux_core::redact_json(&value);
    serde_json::json!({ "result": result, "structuredContent": structured })
}

/// Read a string field from a JSON object, defaulting to `""`.
fn js_str<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

/// Read an unsigned integer field from a JSON object, defaulting to `0`.
fn js_u64(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

/// "1 file" / "3 files" — naive ASCII pluralization for a count line.
fn count_noun(n: u64, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Turn one source tool's structured output into a compact, human-readable summary — "what I
/// found", not raw JSON. Read-only and derived purely from the structured value; the full detail
/// is preserved by the caller in `structuredContent`. An unrecognized tool / shape falls back to
/// pretty JSON so nothing is ever lost.
pub fn humanize(tool_name: &str, value: &serde_json::Value) -> String {
    match tool_name {
        "plugin.summary" => humanize_summary(value),
        "plugin.inspect" => humanize_inspect(value),
        "plugin.search" => humanize_search(value),
        "plugin.read_file" => humanize_read_file(value),
        _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn humanize_summary(v: &serde_json::Value) -> String {
    let name = js_str(v, "name");
    let id = js_str(v, "plugin_id");
    let title = if name.is_empty() { id } else { name };
    let version = js_str(v, "version");
    let kind = js_str(v, "kind");
    let desc = js_str(v, "description");
    let files = js_u64(v, "file_count");
    let dirs = js_u64(v, "dir_count");
    let declared = js_u64(v, "declared_tool_count");
    let generated = v.get("generated_manifest").and_then(|x| x.as_bool()).unwrap_or(false);

    let mut head = format!("**{title}**");
    if !version.is_empty() {
        head.push_str(&format!(" v{version}"));
    }
    if !kind.is_empty() {
        head.push_str(&format!(" — {kind}"));
    }
    head.push('.');
    let mut lines = vec![head];
    if !desc.is_empty() {
        lines.push(desc.to_string());
    }

    let mut counts = format!("{}, {}", count_noun(files, "file"), count_noun(dirs, "directory"));
    counts = counts.replace("directorys", "directories");
    if declared > 0 {
        counts.push_str(&format!(", {} declared", count_noun(declared, "tool")));
    } else if generated {
        counts.push_str(" · manifestless install (no declared tools — read-only source tools only)");
    }
    lines.push(counts);

    // Detected hints: list the human labels (what kind of thing this looks like), or say so plainly.
    let hint_labels: Vec<String> = v
        .get("detected_hints")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|h| h.get("label").and_then(|l| l.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if hint_labels.is_empty() {
        lines.push("Detected signals: none — no MCP/CLI/package entrypoints recognized.".to_string());
    } else {
        lines.push(format!("Detected signals: {}.", hint_labels.join(", ")));
    }

    let readme = js_str(v, "readme_excerpt");
    if !readme.is_empty() {
        lines.push(format!("README:\n{}", clamp_chars(readme.trim(), 400)));
    }
    lines.join("\n")
}

fn humanize_inspect(v: &serde_json::Value) -> String {
    let root = js_str(v, "root");
    let count = js_u64(v, "entry_count");
    let truncated = v.get("truncated").and_then(|x| x.as_bool()).unwrap_or(false);
    let where_ = if root.is_empty() || root == "." { "the plugin root".to_string() } else { format!("`{root}`") };
    let mut head = format!("Listed {} under {where_}", count_noun(count, "entry").replace("entrys", "entries"));
    if truncated {
        head.push_str(" (truncated)");
    }
    head.push('.');
    let mut lines = vec![head];
    if let Some(entries) = v.get("entries").and_then(|e| e.as_array()) {
        for e in entries.iter().take(25) {
            let path = e.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let size = e.get("size_bytes").and_then(|s| s.as_u64());
            match size {
                Some(b) => lines.push(format!("  {path} ({b} B)")),
                None => lines.push(format!("  {path}")),
            }
        }
        if entries.len() > 25 {
            lines.push(format!("  …and {} more", entries.len() - 25));
        }
    }
    lines.join("\n")
}

fn humanize_search(v: &serde_json::Value) -> String {
    let query = js_str(v, "query");
    let count = js_u64(v, "match_count");
    let files = js_u64(v, "files_scanned");
    let truncated = v.get("truncated").and_then(|x| x.as_bool()).unwrap_or(false);
    if count == 0 {
        return format!("No matches for \"{query}\" across {}.", count_noun(files, "file"));
    }
    let mut head = format!(
        "Found {} for \"{query}\" across {}",
        count_noun(count, "match").replace("matchs", "matches"),
        count_noun(files, "file"),
    );
    if truncated {
        head.push_str(" (truncated)");
    }
    head.push(':');
    let mut lines = vec![head];
    if let Some(matches) = v.get("matches").and_then(|m| m.as_array()) {
        for m in matches.iter().take(15) {
            let path = m.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let line = m.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
            let text = m.get("text").and_then(|t| t.as_str()).unwrap_or("");
            lines.push(format!("  {path}:{line} — {text}"));
        }
        if matches.len() > 15 {
            lines.push(format!("  …and {} more", matches.len() - 15));
        }
    }
    lines.join("\n")
}

fn humanize_read_file(v: &serde_json::Value) -> String {
    let path = js_str(v, "path");
    let total = js_u64(v, "total_bytes");
    let returned = js_u64(v, "bytes_returned");
    let truncated = v.get("truncated").and_then(|x| x.as_bool()).unwrap_or(false);
    let content = js_str(v, "content");
    let mut head = if truncated {
        format!("Read {path} (first {returned} of {total} bytes, truncated):")
    } else {
        format!("Read {path} ({total} bytes):")
    };
    head.push('\n');
    head.push('\n');
    head.push_str(content);
    head
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_plugin() -> PathBuf {
        // A unique-enough temp dir without depending on wall-clock/randomness (forbidden in
        // some crates). Use the process id + a static counter.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("relux-plugin-source-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("README.md"), "# Acme\nDoes acme things.\n").unwrap();
        fs::write(dir.join("src").join("main.rs"), "fn main() { println!(\"hello acme\"); }\n").unwrap();
        fs::write(dir.join("package.json"), "{\"name\":\"acme\"}\n").unwrap();
        dir
    }

    #[test]
    fn read_file_reads_within_dir() {
        let dir = tmp_plugin();
        let out = read_file(&dir, &serde_json::json!({"path": "README.md"})).unwrap();
        assert!(out["content"].as_str().unwrap().contains("acme things"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn traversal_is_denied() {
        let dir = tmp_plugin();
        // Classic dot-dot traversal.
        let err = read_file(&dir, &serde_json::json!({"path": "../../etc/passwd"})).unwrap_err();
        assert!(matches!(err, SourceError::PathEscape(_)));
        // Absolute path.
        let abs = if cfg!(windows) { "C:/Windows/win.ini" } else { "/etc/passwd" };
        let err = read_file(&dir, &serde_json::json!({"path": abs})).unwrap_err();
        assert!(matches!(err, SourceError::PathEscape(_)));
        // Inspect with a traversal sub-path is denied too.
        let err = inspect(&dir, &serde_json::json!({"path": ".."})).unwrap_err();
        assert!(matches!(err, SourceError::PathEscape(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_lists_the_tree() {
        let dir = tmp_plugin();
        let out = inspect(&dir, &serde_json::json!({})).unwrap();
        let entries = out["entries"].as_array().unwrap();
        let paths: Vec<&str> = entries.iter().filter_map(|e| e["path"].as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"src/main.rs"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_finds_text() {
        let dir = tmp_plugin();
        let out = search(&dir, &serde_json::json!({"query": "hello acme"})).unwrap();
        assert_eq!(out["match_count"].as_u64().unwrap(), 1);
        let m = &out["matches"][0];
        assert_eq!(m["path"].as_str().unwrap(), "src/main.rs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_requires_a_query() {
        let dir = tmp_plugin();
        let err = search(&dir, &serde_json::json!({})).unwrap_err();
        assert!(matches!(err, SourceError::MissingArg("query")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_reports_metadata_and_readme() {
        let dir = tmp_plugin();
        let meta = SummaryMeta {
            plugin_id: "acme".to_string(),
            name: "Acme".to_string(),
            generated_manifest: true,
            ..Default::default()
        };
        let out = summary(&dir, &meta, &serde_json::json!({})).unwrap();
        assert_eq!(out["plugin_id"].as_str().unwrap(), "acme");
        assert!(out["readme_excerpt"].as_str().unwrap().contains("acme things"));
        assert!(out["file_count"].as_u64().unwrap() >= 3);
        assert!(out["generated_manifest"].as_bool().unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_tool_membership() {
        assert!(is_source_tool("plugin.summary"));
        assert!(is_source_tool("plugin.read_file"));
        assert!(!is_source_tool("echo.say"));
        assert_eq!(source_risk(), RiskLevel::Low);
        assert_eq!(source_approval(), ApprovalRequirement::Never);
    }

    // --- Natural-language shaping ---------------------------------------------
    //
    // Every Plugin Lens result must reach the chat surface / brain as a human SUMMARY, never raw
    // structured JSON. These pin that `humanize` derives clean prose for each tool and that
    // `shape_result` preserves the full structured detail under `structuredContent`.

    #[test]
    fn shape_result_wraps_human_summary_and_keeps_structured_detail() {
        let raw = serde_json::json!({ "match_count": 0, "files_scanned": 4, "query": "todo", "matches": [] });
        let shaped = shape_result("plugin.search", raw.clone());
        // The human summary rides up front and is NOT raw JSON.
        let result = shaped["result"].as_str().unwrap();
        assert!(!result.trim_start().starts_with('{'), "leaked raw JSON: {result}");
        assert!(result.contains("No matches for \"todo\""), "{result}");
        // The full structured detail is preserved verbatim for audit / expansion.
        assert_eq!(shaped["structuredContent"], raw);
    }

    #[test]
    fn shape_result_redacts_secrets_in_both_halves() {
        // Build a key-shaped token at runtime so no literal secret appears in source.
        let sk = format!("sk-ant-{}", "0123456789abcdef0123");
        let opaque = format!("Zq{}", "83hh21pPlainOpaqueToken");
        // A read_file whose human result folds in the body AND whose structured `content`
        // carries the same secret — the worst case for leaking a committed credential.
        let raw = serde_json::json!({
            "path": ".env",
            "total_bytes": 42,
            "bytes_returned": 42,
            "truncated": false,
            "content": format!("OPENAI_API_KEY={sk}\napi_key={opaque}\n"),
        });
        let shaped = shape_result("plugin.read_file", raw);
        let flat = shaped.to_string();
        // Neither the visible `result` text nor the structured detail may carry the raw secret.
        assert!(!flat.contains(&sk), "prefix secret leaked: {flat}");
        assert!(!flat.contains(&opaque), "opaque key-named secret leaked: {flat}");
        let result = shaped["result"].as_str().unwrap();
        assert!(!result.contains(&sk), "result text leaked secret: {result}");
        assert!(
            flat.contains(relux_core::redact::REDACTION_PLACEHOLDER),
            "expected a redaction marker: {flat}"
        );
    }

    #[test]
    fn humanize_summary_is_readable_prose() {
        let v = serde_json::json!({
            "plugin_id": "acme-repo",
            "name": "Acme",
            "version": "1.2.0",
            "kind": "Manifestless",
            "description": "Does acme things.",
            "file_count": 7,
            "dir_count": 2,
            "declared_tool_count": 0,
            "generated_manifest": true,
            "detected_hints": [ { "kind": "npm-package", "label": "npm package", "detail": "acme" } ],
            "readme_excerpt": "# Acme\nDoes acme things.",
        });
        let s = humanize("plugin.summary", &v);
        assert!(s.contains("**Acme**"), "{s}");
        assert!(s.contains("Does acme things."), "{s}");
        assert!(s.contains("7 files, 2 directories"), "{s}");
        assert!(s.contains("manifestless install"), "{s}");
        assert!(s.contains("Detected signals: npm package."), "{s}");
        assert!(s.contains("README:"), "{s}");
        assert!(!s.trim_start().starts_with('{'), "leaked raw JSON: {s}");
    }

    #[test]
    fn humanize_search_lists_hits_naturally() {
        let v = serde_json::json!({
            "query": "fixme",
            "match_count": 2,
            "files_scanned": 5,
            "truncated": false,
            "matches": [
                { "path": "src/a.rs", "line": 12, "text": "// FIXME: handle" },
                { "path": "src/b.rs", "line": 3, "text": "let fixme = 1;" },
            ],
        });
        let s = humanize("plugin.search", &v);
        assert!(s.starts_with("Found 2 matches for \"fixme\" across 5 files:"), "{s}");
        assert!(s.contains("src/a.rs:12 — // FIXME: handle"), "{s}");
    }

    #[test]
    fn humanize_read_file_folds_the_body() {
        let v = serde_json::json!({
            "path": "README.md",
            "total_bytes": 24,
            "bytes_returned": 24,
            "truncated": false,
            "content": "# Acme\nDoes acme things.",
        });
        let s = humanize("plugin.read_file", &v);
        assert!(s.starts_with("Read README.md (24 bytes):"), "{s}");
        assert!(s.contains("Does acme things."), "{s}");
    }

    #[test]
    fn humanize_inspect_describes_the_tree() {
        let v = serde_json::json!({
            "root": ".",
            "entry_count": 2,
            "truncated": false,
            "entries": [
                { "path": "README.md", "kind": "file", "size_bytes": 24 },
                { "path": "src/", "kind": "dir", "size_bytes": null },
            ],
        });
        let s = humanize("plugin.inspect", &v);
        assert!(s.contains("Listed 2 entries under the plugin root."), "{s}");
        assert!(s.contains("README.md (24 B)"), "{s}");
        assert!(s.contains("src/"), "{s}");
    }
}
