//! Read-only **run artifact references** captured from an adapter's structured
//! result envelope.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.6 (Run) and section 15 (Run
//! Detail honest review/apply). This is the **first real Relux run artifact
//! model** and is deliberately scoped to *capture*, not *apply*:
//!
//! - A [`RunArtifact`] is a **reference** the adapter declared (name + type +
//!   summary + source, optional sanitized path/size). Relux records it
//!   read-only and **never reads the underlying file** — there is no diff and no
//!   apply here. Diff/apply require a separate model that does not exist yet, so
//!   the dashboard surfaces the references but keeps apply unavailable.
//! - This is NOT the legacy `relix-runtime` `RunArtifact` (a workspace
//!   changed-file with a baseline hash + safe-apply plan). That belongs to the
//!   separate `brief_runs` ledger / legacy `/v1/runs` surface. The two models do
//!   not share ids or storage.
//!
//! Safety bar (master plan section 17.5): references are bounded (count + every
//! field), secret-redacted, and any path is sanitized — an absolute path, a
//! drive/UNC root, or a `..` traversal is dropped rather than surfaced, because a
//! reference must never become a path-traversal primitive even though we never
//! open it.

use serde::{Deserialize, Serialize};

use crate::redact::redact_secrets;

/// The maximum number of artifact references captured from one run. Extra entries
/// beyond this are dropped (the run summary/transcript still records the run).
pub const MAX_ARTIFACTS: usize = 64;

/// Per-field character caps. These bound a single hostile/huge envelope so a run
/// record stays small and the dashboard stays legible.
const MAX_NAME_CHARS: usize = 200;
const MAX_SUMMARY_CHARS: usize = 500;
const MAX_PATH_CHARS: usize = 400;
const MAX_SOURCE_CHARS: usize = 120;

/// The kind of a captured run artifact reference. Unknown/missing types degrade
/// honestly to [`ArtifactKind::Other`] rather than guessing a richer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A file the run produced or referenced.
    File,
    /// A diff/patch the run described (still a reference — not an apply plan).
    Diff,
    /// A patch file.
    Patch,
    /// A log or output capture.
    Log,
    /// A URL the run produced.
    Url,
    /// A free-form note/summary artifact.
    Note,
    /// Any other / unrecognized kind.
    Other,
}

impl ArtifactKind {
    /// Map a free-form envelope `type` string to a known kind, defaulting to
    /// [`ArtifactKind::Other`] for anything we do not recognize.
    fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "file" | "files" => ArtifactKind::File,
            "diff" => ArtifactKind::Diff,
            "patch" => ArtifactKind::Patch,
            "log" | "logs" | "output" => ArtifactKind::Log,
            "url" | "link" => ArtifactKind::Url,
            "note" | "notes" | "summary" => ArtifactKind::Note,
            _ => ArtifactKind::Other,
        }
    }
}

/// A single read-only artifact reference captured from an adapter result
/// envelope. Every field is bounded + secret-redacted; `path` is `None` when the
/// declared path was unsafe (absolute / drive / UNC / `..`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunArtifact {
    /// A short display name (sanitized, capped). Derived from the envelope's
    /// `name`, else the path's basename, else `"artifact"`.
    pub name: String,
    /// The artifact kind (serialized as `type`).
    #[serde(rename = "type")]
    pub kind: ArtifactKind,
    /// A bounded, redacted human summary, when the envelope carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Where the reference came from (the adapter label), so the UI can say "from
    /// claude-cli" honestly.
    pub source: String,
    /// A sanitized, relative display path, when the declared path was safe.
    /// `None` when no path was declared OR the declared path was unsafe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The reported size in bytes, when the envelope carried one. Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// True when any captured field was truncated to its cap, so the UI can show
    /// a "…" honesty marker rather than implying the full value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Capture artifact references from an envelope's `artifacts` value (an array of
/// objects, or of bare-string names). Returns an empty vec for anything else.
/// `source` labels where the references came from (the adapter kind/binary).
///
/// This NEVER touches the filesystem: it only reads, bounds, redacts, and
/// sanitizes the declared references.
pub fn capture_run_artifacts(value: Option<&serde_json::Value>, source: &str) -> Vec<RunArtifact> {
    let arr = match value.and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let source = cap_chars(&redact_secrets(source.trim()), MAX_SOURCE_CHARS).0;
    let source = if source.is_empty() {
        "adapter".to_string()
    } else {
        source
    };
    let mut out = Vec::new();
    for item in arr {
        if out.len() >= MAX_ARTIFACTS {
            break;
        }
        if let Some(a) = capture_one(item, &source) {
            out.push(a);
        }
    }
    out
}

/// Capture one artifact reference from a single envelope item. Returns `None`
/// when the item is neither an object nor a non-empty string.
fn capture_one(item: &serde_json::Value, source: &str) -> Option<RunArtifact> {
    // A bare string is treated as a name-only note reference.
    if let Some(s) = item.as_str() {
        let (name, name_trunc) = clean_field(s, MAX_NAME_CHARS);
        if name.is_empty() {
            return None;
        }
        return Some(RunArtifact {
            name,
            kind: ArtifactKind::Other,
            summary: None,
            source: source.to_string(),
            path: None,
            bytes: None,
            truncated: name_trunc,
        });
    }

    let obj = item.as_object()?;
    let mut truncated = false;

    // Path first, since the name can fall back to its basename.
    let raw_path = obj
        .get("path")
        .or_else(|| obj.get("file"))
        .and_then(|v| v.as_str());
    let path = raw_path.and_then(sanitize_artifact_path);

    // Name: explicit `name`, else the basename of a (possibly unsafe) declared
    // path, else a constant. Sanitized + capped either way.
    let name_source = obj
        .get("name")
        .or_else(|| obj.get("title"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| path.as_deref().and_then(basename).map(|s| s.to_string()))
        .or_else(|| raw_path.and_then(basename).map(|s| s.to_string()))
        .unwrap_or_else(|| "artifact".to_string());
    let (name, name_trunc) = clean_field(&name_source, MAX_NAME_CHARS);
    truncated |= name_trunc;
    let name = if name.is_empty() {
        "artifact".to_string()
    } else {
        name
    };

    let kind = obj
        .get("type")
        .or_else(|| obj.get("kind"))
        .and_then(|v| v.as_str())
        .map(ArtifactKind::from_str_lossy)
        .unwrap_or_else(|| infer_kind(path.as_deref()));

    let summary = obj
        .get("summary")
        .or_else(|| obj.get("description"))
        .and_then(|v| v.as_str())
        .map(|s| {
            let (text, trunc) = clean_field(s, MAX_SUMMARY_CHARS);
            truncated |= trunc;
            text
        })
        .filter(|s| !s.is_empty());

    let bytes = obj
        .get("bytes")
        .or_else(|| obj.get("size"))
        .and_then(|v| v.as_u64());

    Some(RunArtifact {
        name,
        kind,
        summary,
        source: source.to_string(),
        path,
        bytes,
        truncated,
    })
}

/// Light kind inference from a safe path's extension, defaulting to `File` for a
/// known path and `Other` otherwise. Never guesses beyond the extension.
fn infer_kind(path: Option<&str>) -> ArtifactKind {
    match path {
        Some(p) if p.ends_with(".patch") => ArtifactKind::Patch,
        Some(p) if p.ends_with(".diff") => ArtifactKind::Diff,
        Some(p) if p.ends_with(".log") => ArtifactKind::Log,
        Some(_) => ArtifactKind::File,
        None => ArtifactKind::Other,
    }
}

/// Redact secrets, strip control characters, collapse whitespace, and cap a
/// free-text field. Returns the cleaned text and whether it was truncated.
fn clean_field(raw: &str, max_chars: usize) -> (String, bool) {
    let redacted = redact_secrets(raw);
    let cleaned: String = redacted
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    cap_chars(trimmed, max_chars)
}

/// Cap a string to `max_chars` characters, returning `(capped, was_truncated)`.
fn cap_chars(s: &str, max_chars: usize) -> (String, bool) {
    if s.chars().count() > max_chars {
        (s.chars().take(max_chars).collect(), true)
    } else {
        (s.to_string(), false)
    }
}

/// The final path component (basename) of a `/`- or `\`-separated path.
fn basename(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\']).find(|seg| !seg.is_empty())
}

/// Sanitize a declared artifact path into a safe, relative, `/`-separated
/// display string, or `None` when it is unsafe.
///
/// Rejects (returns `None`): an absolute POSIX path (`/...`), a Windows drive
/// path (`C:\...` / `C:/...`), a UNC path (`\\server`), or any `..` traversal
/// component. Drops `.` components. The result is purely a display label — Relux
/// never opens it — but we still refuse traversal so a reference can never be
/// mistaken for a safe relative path downstream.
fn sanitize_artifact_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Absolute POSIX or leading separator.
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return None;
    }
    // Windows drive-letter path: `C:` / `C:\` / `C:/`.
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in trimmed.split(['/', '\\']) {
        match seg {
            "" | "." => continue,
            ".." => return None,
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("/");
    // Redact + cap the display path (defensive: paths can embed secrets).
    let (capped, _trunc) = cap_chars(&redact_secrets(&joined), MAX_PATH_CHARS);
    if capped.is_empty() {
        None
    } else {
        Some(capped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn captures_object_artifacts_with_all_fields() {
        let value = json!([
            { "name": "main.rs", "type": "file", "summary": "edited main", "path": "src/main.rs", "bytes": 1200 },
            { "type": "diff", "path": "changes.diff" }
        ]);
        let arts = capture_run_artifacts(Some(&value), "claude-cli");
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].name, "main.rs");
        assert_eq!(arts[0].kind, ArtifactKind::File);
        assert_eq!(arts[0].summary.as_deref(), Some("edited main"));
        assert_eq!(arts[0].path.as_deref(), Some("src/main.rs"));
        assert_eq!(arts[0].bytes, Some(1200));
        assert_eq!(arts[0].source, "claude-cli");
        assert!(!arts[0].truncated);
        // Name falls back to the basename of the path.
        assert_eq!(arts[1].name, "changes.diff");
        assert_eq!(arts[1].kind, ArtifactKind::Diff);
    }

    #[test]
    fn unknown_type_degrades_to_other() {
        let value = json!([{ "name": "thing", "type": "weird-thing" }]);
        let arts = capture_run_artifacts(Some(&value), "codex-cli");
        assert_eq!(arts[0].kind, ArtifactKind::Other);
    }

    #[test]
    fn absolute_and_traversal_paths_are_dropped_but_artifact_kept() {
        let value = json!([
            { "name": "a", "path": "/etc/passwd" },
            { "name": "b", "path": "..\\..\\secrets.txt" },
            { "name": "c", "path": "C:\\Windows\\System32\\config" },
            { "name": "d", "path": "\\\\server\\share\\x" }
        ]);
        let arts = capture_run_artifacts(Some(&value), "x");
        assert_eq!(arts.len(), 4);
        for a in &arts {
            assert_eq!(a.path, None, "unsafe path must be dropped: {}", a.name);
        }
    }

    #[test]
    fn windows_backslashes_are_normalized_when_safe() {
        let value = json!([{ "name": "n", "path": "src\\nested\\file.rs" }]);
        let arts = capture_run_artifacts(Some(&value), "x");
        assert_eq!(arts[0].path.as_deref(), Some("src/nested/file.rs"));
    }

    #[test]
    fn count_is_capped() {
        let items: Vec<serde_json::Value> =
            (0..(MAX_ARTIFACTS + 20)).map(|i| json!({ "name": format!("a{i}") })).collect();
        let arts = capture_run_artifacts(Some(&json!(items)), "x");
        assert_eq!(arts.len(), MAX_ARTIFACTS);
    }

    #[test]
    fn fields_are_capped_and_flagged_truncated() {
        let long = "x".repeat(MAX_SUMMARY_CHARS + 50);
        let value = json!([{ "name": "n", "summary": long }]);
        let arts = capture_run_artifacts(Some(&value), "x");
        assert!(arts[0].truncated);
        assert_eq!(arts[0].summary.as_ref().unwrap().chars().count(), MAX_SUMMARY_CHARS);
    }

    #[test]
    fn secrets_are_redacted_in_fields() {
        let value = json!([{ "name": "n", "summary": "token sk-ant-abcdefghijklmnop1234567890" }]);
        let arts = capture_run_artifacts(Some(&value), "x");
        assert!(arts[0].summary.as_ref().unwrap().contains("REDACTED"));
        assert!(!arts[0].summary.as_ref().unwrap().contains("sk-ant-abcdefghijklmnop"));
    }

    #[test]
    fn bare_string_item_is_a_named_note() {
        let value = json!(["just-a-name", ""]);
        let arts = capture_run_artifacts(Some(&value), "x");
        // The empty string is skipped.
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].name, "just-a-name");
        assert_eq!(arts[0].kind, ArtifactKind::Other);
    }

    #[test]
    fn non_array_value_is_empty() {
        assert!(capture_run_artifacts(Some(&json!({"artifacts": 1})), "x").is_empty());
        assert!(capture_run_artifacts(Some(&json!("nope")), "x").is_empty());
        assert!(capture_run_artifacts(None, "x").is_empty());
    }

    #[test]
    fn empty_source_defaults_to_adapter() {
        let value = json!([{ "name": "n" }]);
        let arts = capture_run_artifacts(Some(&value), "   ");
        assert_eq!(arts[0].source, "adapter");
    }
}
