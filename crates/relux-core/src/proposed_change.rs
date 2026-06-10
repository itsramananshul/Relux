//! **Reviewed, reviewable proposed file changes** captured from an adapter's
//! structured result envelope — the first real Relux diff/apply model.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 15 ("a Relux diff/apply model
//! and accept/reject review (the next slice — the captured references define the
//! contract for it)"), section 9.6 (Run), and the safety bar of section 17.5.
//!
//! This is the narrow, safe step beyond the read-only [`crate::artifact`]
//! references: where a [`crate::RunArtifact`] is only a *reference* Relux never
//! opens, a [`ProposedChange`] carries the **full proposed new content** of one
//! text file, plus the **baseline hash** of the content the agent based its edit
//! on. It is captured from a dedicated envelope field `proposed_changes: [...]`
//! (kept distinct from `artifacts: [...]` so the read-only reference model is
//! untouched). The model is deliberately a **single-file full-content
//! replacement**, NOT arbitrary patch/diff parsing — a replacement either applies
//! cleanly or refuses, with no fuzzy hunk matching to get subtly wrong.
//!
//! ## Safety model (v1)
//!
//! Capture (this module) is pure and never touches the filesystem. It only
//! bounds, validates, and sanitizes the declared change:
//!
//! - the `path` must be **relative + safe** — an absolute path, a drive/UNC root,
//!   a `..` traversal, or an **excluded** path (vcs/build/secret) is dropped and
//!   the change is NOT captured (it can never become an apply target);
//! - the `content` must be **text** (no NUL byte) and within [`MAX_CONTENT_BYTES`];
//! - the count is capped at [`MAX_PROPOSED_CHANGES`];
//! - a `baseline_sha256`, when present, is validated as 64 lowercase hex chars
//!   (anything else is dropped to `None`);
//! - the `action` is `replace` (the default — full-content replacement over an
//!   existing baseline file), `create` (a brand-new file that must NOT already
//!   exist at apply time), or `rename`/`move` (relocate an existing baseline file
//!   to a `dest_path` that must NOT already exist, preserving its content). A
//!   missing `action` defaults to `replace` so older envelopes and persisted
//!   records stay valid; an unknown action string drops the change (we never store
//!   a change we could not safely interpret).
//! - a `rename` additionally needs a `dest_path` (alias `to`/`to_path`/`dest`/
//!   `destination`/`new_path`) that is itself relative + safe + not excluded and
//!   distinct from the source `path`; otherwise the change is dropped. A rename
//!   carries NO new content (the move preserves the file's bytes).
//!
//! Apply itself lives in the kernel ([`crate::ProposedChange`] is the durable
//! record it works from) and adds the rest of the bar. A `replace` apply requires
//! an explicit operator **approval**, **refuses without a baseline hash** (no force
//! in v1), and verifies the baseline still matches the on-disk file (**conflict**
//! otherwise). A `create` apply also requires **approval**, needs **no baseline**
//! (there is no prior content), and refuses if the target **already exists** (a
//! conflict — it never overwrites). A `rename` apply requires **approval**, also
//! **refuses without a baseline hash** for the source, verifies the source still
//! matches it (**conflict** otherwise), and refuses if the `dest_path` **already
//! exists** (a conflict — it never overwrites); it then moves the file. All only
//! ever write inside the run's controlled workspace root. Capturing a proposed
//! change NEVER applies it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::redact::redact_secrets;

/// The maximum number of proposed changes captured from one run. Extra entries
/// beyond this are dropped (the run summary/transcript still records the run).
pub const MAX_PROPOSED_CHANGES: usize = 32;

/// The maximum byte length of one proposed change's new content. A larger
/// declared content drops the whole change (we never store a half file).
pub const MAX_CONTENT_BYTES: usize = 256 * 1024;

/// Per-field caps for the bounded display/text fields.
const MAX_PATH_CHARS: usize = 400;
const MAX_SOURCE_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 500;
const MAX_REASON_CHARS: usize = 500;

/// Compute the lowercase hex SHA-256 of `bytes`. Used for both the new content's
/// integrity hash and (in the kernel) the on-disk baseline comparison, so the two
/// are always computed the same way.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    out
}

/// The review/apply lifecycle of a [`ProposedChange`].
///
/// `Proposed` → (operator review) → `Approved` → (operator apply) → `Applied`,
/// or `Rejected` at review. Apply is refused in any state other than `Approved`,
/// so a change can never be applied without an explicit approval, and an
/// already-applied change can never be re-applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposedChangeStatus {
    /// Captured, awaiting operator review.
    Proposed,
    /// Operator approved; eligible for an explicit apply.
    Approved,
    /// Operator rejected; will never be applied.
    Rejected,
    /// Successfully written to the workspace.
    Applied,
}

impl ProposedChangeStatus {
    /// The wire/string label (matches the serde rename).
    pub fn as_str(&self) -> &'static str {
        match self {
            ProposedChangeStatus::Proposed => "proposed",
            ProposedChangeStatus::Approved => "approved",
            ProposedChangeStatus::Rejected => "rejected",
            ProposedChangeStatus::Applied => "applied",
        }
    }
}

/// The filesystem action a [`ProposedChange`] applies.
///
/// `Replace` (the default) is a **full-content replacement** of an *existing*
/// file, gated by a baseline hash. `Create` is a **brand-new file** that must NOT
/// already exist at apply time and needs no baseline. `Rename` (move) relocates an
/// *existing* baseline file from `path` to a new `dest_path` that must NOT already
/// exist — content is preserved, so it carries no new content. The set is kept
/// deliberately small — no delete in this slice — so every apply maps to exactly
/// one strict, well-understood write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposedChangeAction {
    /// Full-content replacement of an existing baseline file (the historical
    /// behavior and the serde default, so a missing `action` deserializes here).
    #[default]
    Replace,
    /// Create a new file that must not already exist at apply time.
    Create,
    /// Move an existing baseline file from `path` to `dest_path`, where the
    /// destination must not already exist. Content is preserved (no new content).
    Rename,
}

impl ProposedChangeAction {
    /// The wire/string label (matches the serde rename).
    pub fn as_str(&self) -> &'static str {
        match self {
            ProposedChangeAction::Replace => "replace",
            ProposedChangeAction::Create => "create",
            ProposedChangeAction::Rename => "rename",
        }
    }

    /// Whether this action verifies an existing file against a declared baseline
    /// hash (and so requires one at apply time — no force in v1). A `replace`
    /// overwrites that file; a `rename` moves it; a `create` has no prior file.
    pub fn requires_baseline(&self) -> bool {
        matches!(
            self,
            ProposedChangeAction::Replace | ProposedChangeAction::Rename
        )
    }

    /// Whether this action carries a destination path (a `rename`/move). Replace
    /// and create write a single `path`; only a rename has a distinct `dest_path`.
    pub fn has_destination(&self) -> bool {
        matches!(self, ProposedChangeAction::Rename)
    }
}

/// One reviewable, applyable proposed file change: a **full-content replacement**
/// of one text file at a safe relative `path`, with the agent's declared
/// `baseline_sha256` so apply can detect a conflict. Captured read-only from the
/// envelope; the kernel mutates only `status` / `review_note` / `refused_reason`
/// / `applied_at` as the operator reviews and applies it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedChange {
    /// Safe, relative, `/`-separated target path inside the run's workspace root.
    pub path: String,
    /// The filesystem action this change applies — `replace` (over an existing
    /// baseline file), `create` (a new file), or `rename` (move `path` to
    /// `dest_path`). `#[serde(default)]` means an older record or envelope with no
    /// `action` deserializes as `replace`.
    #[serde(default)]
    pub action: ProposedChangeAction,
    /// For a `rename` (move) action, the safe, relative, `/`-separated destination
    /// path the source `path` is moved to. It must NOT already exist at apply time
    /// and must differ from the source. `None` for `replace`/`create` (which write
    /// a single `path`). `#[serde(default)]` keeps older records without it valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_path: Option<String>,
    /// The full proposed new content of the file (text, within
    /// [`MAX_CONTENT_BYTES`]). Stored verbatim so apply writes exactly this. A
    /// `rename` preserves the file's bytes, so it carries no new content (empty).
    pub new_content: String,
    /// SHA-256 (lowercase hex) of the content the agent based its edit on, when
    /// the envelope declared it. A `replace` or `rename` apply **refuses without
    /// this** (no force in v1) and refuses when it no longer matches the on-disk
    /// source file (a conflict). A `create` change carries no baseline (there is no
    /// prior file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_sha256: Option<String>,
    /// SHA-256 (lowercase hex) of `new_content`, computed at capture. Display +
    /// integrity (the wire value can be checked against the content).
    pub new_sha256: String,
    /// Byte length of `new_content`. Display-only.
    pub bytes: u64,
    /// The adapter that produced the change (e.g. "claude-cli").
    pub source: String,
    /// The review/apply lifecycle state.
    pub status: ProposedChangeStatus,
    /// A bounded, redacted operator note recorded at review time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_note: Option<String>,
    /// The honest reason the last apply attempt was refused (e.g. a baseline
    /// conflict, a missing workspace root, or no baseline hash). Cleared on a
    /// successful apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused_reason: Option<String>,
    /// The logical-clock stamp recorded when the change was applied. `None` until
    /// it is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
}

impl ProposedChange {
    /// Set a bounded, redacted refusal reason (truncated to its cap).
    pub fn set_refused_reason(&mut self, reason: &str) {
        self.refused_reason = Some(cap_chars(&redact_secrets(reason.trim()), MAX_REASON_CHARS));
    }

    /// Set a bounded, redacted operator review note (cleared when blank).
    pub fn set_review_note(&mut self, note: &str) {
        let cleaned = cap_chars(&redact_secrets(note.trim()), MAX_NOTE_CHARS);
        self.review_note = if cleaned.is_empty() { None } else { Some(cleaned) };
    }
}

/// Capture proposed changes from an envelope's `proposed_changes` value (an array
/// of objects). Returns an empty vec for anything else. PURE — never touches the
/// filesystem. `source` labels where the changes came from (the adapter kind).
///
/// A change is captured only when it has BOTH a safe relative `path` AND a
/// non-empty text `content` within the cap; otherwise it is dropped (we never
/// store a change we could not safely apply).
pub fn capture_proposed_changes(
    value: Option<&serde_json::Value>,
    source: &str,
) -> Vec<ProposedChange> {
    let arr = match value.and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let source = cap_chars(&redact_secrets(source.trim()), MAX_SOURCE_CHARS);
    let source = if source.is_empty() {
        "adapter".to_string()
    } else {
        source
    };
    let mut out = Vec::new();
    for item in arr {
        if out.len() >= MAX_PROPOSED_CHANGES {
            break;
        }
        if let Some(c) = capture_one(item, &source) {
            out.push(c);
        }
    }
    out
}

/// Capture one proposed change from a single envelope item, or `None` when it is
/// not a safely-applyable full-content replacement.
fn capture_one(item: &serde_json::Value, source: &str) -> Option<ProposedChange> {
    let obj = item.as_object()?;

    // Path: must be present, safe, relative, and not excluded.
    let raw_path = obj
        .get("path")
        .or_else(|| obj.get("file"))
        .and_then(|v| v.as_str())?;
    let path = sanitize_change_path(raw_path)?;

    // Action: `replace` (default), `create`, or `rename`/`move`. A missing action
    // defaults to replace (backward compatibility); an unknown action drops the
    // change — we never store a change we could not safely interpret as a known
    // write.
    let action = match obj
        .get("action")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("replace") => ProposedChangeAction::Replace,
        Some("create") => ProposedChangeAction::Create,
        Some("rename") | Some("move") => ProposedChangeAction::Rename,
        Some(_) => return None,
    };

    // Destination: required (safe, relative, distinct) for a `rename`; meaningless
    // for replace/create, so it is dropped there to keep the record honest.
    let dest_path = match action {
        ProposedChangeAction::Replace | ProposedChangeAction::Create => None,
        ProposedChangeAction::Rename => {
            let raw_dest = obj
                .get("dest_path")
                .or_else(|| obj.get("to"))
                .or_else(|| obj.get("to_path"))
                .or_else(|| obj.get("dest"))
                .or_else(|| obj.get("destination"))
                .or_else(|| obj.get("new_path"))
                .and_then(|v| v.as_str())?;
            let dest = sanitize_change_path(raw_dest)?;
            // A rename to the source path is a no-op; never store one.
            if dest == path {
                return None;
            }
            Some(dest)
        }
    };

    // Content: required text within the cap for `replace`/`create`. A `rename`
    // moves the file intact, so it carries no new content (empty) and any declared
    // content is ignored.
    let (new_content, new_sha256, bytes) = match action {
        ProposedChangeAction::Rename => {
            let empty = String::new();
            let hash = sha256_hex(empty.as_bytes());
            (empty, hash, 0u64)
        }
        ProposedChangeAction::Replace | ProposedChangeAction::Create => {
            let content = obj
                .get("content")
                .or_else(|| obj.get("new_content"))
                .or_else(|| obj.get("text"))
                .and_then(|v| v.as_str())?;
            if content.is_empty() || content.len() > MAX_CONTENT_BYTES || content.contains('\0') {
                return None;
            }
            let new_content = content.to_string();
            let new_sha256 = sha256_hex(new_content.as_bytes());
            let bytes = new_content.len() as u64;
            (new_content, new_sha256, bytes)
        }
    };

    // Baseline hash: a `replace` or `rename` may carry one (validated as 64
    // lowercase hex chars or dropped); a `create` has no prior file, so any
    // declared baseline is meaningless and is dropped to keep the record honest.
    let baseline_sha256 = if action.requires_baseline() {
        obj.get("baseline_sha256")
            .or_else(|| obj.get("baseline"))
            .or_else(|| obj.get("base_sha256"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| is_sha256_hex(s))
    } else {
        None
    };

    Some(ProposedChange {
        bytes,
        path,
        action,
        dest_path,
        new_content,
        baseline_sha256,
        new_sha256,
        source: source.to_string(),
        status: ProposedChangeStatus::Proposed,
        review_note: None,
        refused_reason: None,
        applied_at: None,
    })
}

/// Is `s` exactly 64 lowercase hex characters (a SHA-256 digest)?
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Sanitize a declared change path into a safe, relative, `/`-separated string,
/// or `None` when it is unsafe OR excluded. This is the **apply gate**, so it is
/// stricter than the display-only [`crate::artifact`] sanitizer: it also refuses
/// vcs/build/secret paths so an approved change can never write into `.git`, a
/// build dir, or a secret file.
pub fn sanitize_change_path(raw: &str) -> Option<String> {
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
    let segs: Vec<&str> = trimmed.split(['/', '\\']).collect();
    let n = segs.len();
    for (i, seg) in segs.iter().enumerate() {
        match *seg {
            "" | "." => continue,
            ".." => return None,
            other => {
                let last = i + 1 == n;
                if (last && is_excluded_file(other)) || (!last && is_excluded_dir(other)) {
                    return None;
                }
                parts.push(other);
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("/");
    let capped = cap_chars(&joined, MAX_PATH_CHARS);
    if capped.is_empty() || capped.chars().count() > MAX_PATH_CHARS {
        None
    } else {
        Some(capped)
    }
}

/// A directory component that safe-apply never writes through (vcs/build/deps).
fn is_excluded_dir(comp: &str) -> bool {
    matches!(
        comp,
        ".git" | ".hg" | ".svn" | "target" | "node_modules" | ".relux" | ".relix"
    )
}

/// A final file component that safe-apply never writes (secrets / key material).
fn is_excluded_file(comp: &str) -> bool {
    let lower = comp.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower == "id_rsa"
        || lower == "id_ed25519"
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".pfx")
        || lower.ends_with(".p12")
}

/// Cap a string to `max_chars` characters.
fn cap_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        s.chars().take(max_chars).collect()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_hex_is_lowercase_64_chars_and_known() {
        let h = sha256_hex(b"");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(is_sha256_hex(&h));
    }

    #[test]
    fn captures_full_content_change_with_baseline() {
        let base = sha256_hex(b"old\n");
        let value = json!([
            { "path": "src/main.rs", "content": "new\n", "baseline_sha256": base }
        ]);
        let cs = capture_proposed_changes(Some(&value), "claude-cli");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].path, "src/main.rs");
        assert_eq!(cs[0].new_content, "new\n");
        assert_eq!(cs[0].bytes, 4);
        assert_eq!(cs[0].baseline_sha256.as_deref(), Some(base.as_str()));
        assert_eq!(cs[0].new_sha256, sha256_hex(b"new\n"));
        assert_eq!(cs[0].source, "claude-cli");
        assert_eq!(cs[0].status, ProposedChangeStatus::Proposed);
    }

    #[test]
    fn missing_action_defaults_to_replace_for_backward_compat() {
        // An envelope item with no `action` field is the historical shape and must
        // still capture as a replace (backward compatibility).
        let value = json!([{ "path": "f.txt", "content": "x" }]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].action, ProposedChangeAction::Replace);
    }

    #[test]
    fn explicit_replace_action_is_captured() {
        let base = sha256_hex(b"old\n");
        let value = json!([
            { "path": "f.txt", "action": "replace", "content": "new\n", "baseline_sha256": base }
        ]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].action, ProposedChangeAction::Replace);
        assert_eq!(cs[0].baseline_sha256.as_deref(), Some(base.as_str()));
    }

    #[test]
    fn create_action_is_captured_without_baseline() {
        // A create has no prior file: any declared baseline is dropped, and the
        // action is recorded so apply can require the target be absent.
        let value = json!([
            { "path": "new/file.rs", "action": "create", "content": "fn main() {}\n",
              "baseline_sha256": sha256_hex(b"ignored") }
        ]);
        let cs = capture_proposed_changes(Some(&value), "claude-cli");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].action, ProposedChangeAction::Create);
        assert_eq!(cs[0].path, "new/file.rs");
        assert_eq!(cs[0].new_content, "fn main() {}\n");
        assert_eq!(cs[0].baseline_sha256, None);
    }

    #[test]
    fn unknown_action_drops_the_change() {
        // `delete` is not a known action in this slice; `frobnicate` is nonsense.
        // Both drop (we never store a change we could not interpret as a write).
        let value = json!([
            { "path": "f.txt", "action": "delete", "content": "x" },
            { "path": "g.txt", "action": "frobnicate", "content": "y" }
        ]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn rename_action_is_captured_with_a_destination_and_no_content() {
        // A rename carries a source `path`, a distinct `dest_path`, and the source
        // baseline; it moves the file intact, so it stores no new content.
        let base = sha256_hex(b"old\n");
        let value = json!([
            { "path": "src/old.rs", "action": "rename", "to": "src/new.rs",
              "baseline_sha256": base }
        ]);
        let cs = capture_proposed_changes(Some(&value), "claude-cli");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].action, ProposedChangeAction::Rename);
        assert_eq!(cs[0].path, "src/old.rs");
        assert_eq!(cs[0].dest_path.as_deref(), Some("src/new.rs"));
        assert_eq!(cs[0].baseline_sha256.as_deref(), Some(base.as_str()));
        assert_eq!(cs[0].new_content, "");
        assert_eq!(cs[0].bytes, 0);
        assert_eq!(cs[0].new_sha256, sha256_hex(b""));
    }

    #[test]
    fn move_is_an_alias_for_rename_and_accepts_dest_aliases() {
        // `move` maps to the rename action, and the destination accepts the
        // `dest_path`/`destination`/`new_path` aliases too.
        for dest_key in ["dest_path", "to", "to_path", "dest", "destination", "new_path"] {
            let value = json!([
                { "path": "a.txt", "action": "move", dest_key: "b.txt",
                  "baseline_sha256": sha256_hex(b"a") }
            ]);
            let cs = capture_proposed_changes(Some(&value), "x");
            assert_eq!(cs.len(), 1, "dest alias {dest_key} should be accepted");
            assert_eq!(cs[0].action, ProposedChangeAction::Rename);
            assert_eq!(cs[0].dest_path.as_deref(), Some("b.txt"));
        }
    }

    #[test]
    fn rename_appears_on_the_wire_with_its_destination() {
        let value = json!([
            { "path": "a.txt", "action": "rename", "to": "b.txt",
              "baseline_sha256": sha256_hex(b"a") }
        ]);
        let c = &capture_proposed_changes(Some(&value), "x")[0];
        let v = serde_json::to_value(c).unwrap();
        assert_eq!(v.get("action").and_then(|s| s.as_str()), Some("rename"));
        assert_eq!(v.get("dest_path").and_then(|s| s.as_str()), Some("b.txt"));
    }

    #[test]
    fn rename_without_a_destination_drops_the_change() {
        let value = json!([{ "path": "a.txt", "action": "rename",
                             "baseline_sha256": sha256_hex(b"a") }]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn rename_to_an_unsafe_or_excluded_destination_drops_the_change() {
        let value = json!([
            { "path": "a.txt", "action": "rename", "to": "../escape.txt",
              "baseline_sha256": sha256_hex(b"a") },
            { "path": "b.txt", "action": "rename", "to": ".git/config",
              "baseline_sha256": sha256_hex(b"b") },
            { "path": "c.txt", "action": "rename", "to": "deploy/prod.pem",
              "baseline_sha256": sha256_hex(b"c") }
        ]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn rename_to_the_same_path_is_a_noop_and_dropped() {
        let value = json!([
            { "path": "a.txt", "action": "rename", "to": "a.txt",
              "baseline_sha256": sha256_hex(b"a") },
            // backslash-normalized source/dest collapse to the same path too.
            { "path": "src/a.txt", "action": "rename", "to": "src\\a.txt",
              "baseline_sha256": sha256_hex(b"a") }
        ]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn rename_without_a_baseline_is_still_captured() {
        // Apply refuses a rename without a baseline, but capture keeps it so the
        // operator can see what was proposed (and why it cannot be applied).
        let value = json!([{ "path": "a.txt", "action": "rename", "to": "b.txt" }]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].action, ProposedChangeAction::Rename);
        assert_eq!(cs[0].baseline_sha256, None);
    }

    #[test]
    fn action_appears_on_the_wire() {
        let value = json!([{ "path": "f.txt", "action": "create", "content": "x" }]);
        let c = &capture_proposed_changes(Some(&value), "x")[0];
        let v = serde_json::to_value(c).unwrap();
        assert_eq!(v.get("action").and_then(|s| s.as_str()), Some("create"));
    }

    #[test]
    fn legacy_record_without_action_deserializes_as_replace() {
        // A persisted record from before the `action` field existed has no
        // `action`; `#[serde(default)]` must read it back as a replace.
        let legacy = json!({
            "path": "f.txt",
            "new_content": "hi",
            "new_sha256": sha256_hex(b"hi"),
            "bytes": 2,
            "source": "x",
            "status": "approved"
        });
        let c: ProposedChange = serde_json::from_value(legacy).unwrap();
        assert_eq!(c.action, ProposedChangeAction::Replace);
    }

    #[test]
    fn accepts_field_aliases() {
        let value = json!([
            { "file": "a/b.txt", "new_content": "x", "base_sha256": sha256_hex(b"y") }
        ]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].path, "a/b.txt");
        assert_eq!(cs[0].new_content, "x");
        assert!(cs[0].baseline_sha256.is_some());
    }

    #[test]
    fn change_without_baseline_is_still_captured() {
        // Apply refuses without a baseline, but capture keeps it so the operator
        // can see what was proposed (and why it cannot be applied).
        let value = json!([{ "path": "f.txt", "content": "hi" }]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].baseline_sha256, None);
    }

    #[test]
    fn drops_change_without_content() {
        let value = json!([{ "path": "f.txt" }, { "path": "g.txt", "content": "" }]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn drops_unsafe_and_excluded_paths() {
        let value = json!([
            { "path": "/etc/passwd", "content": "x" },
            { "path": "../escape.txt", "content": "x" },
            { "path": "C:\\Windows\\x", "content": "x" },
            { "path": ".git/config", "content": "x" },
            { "path": "target/out", "content": "x" },
            { "path": "secrets/.env", "content": "x" },
            { "path": "deploy/prod.pem", "content": "x" }
        ]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn windows_backslashes_normalize_when_safe() {
        let value = json!([{ "path": "src\\nested\\file.rs", "content": "x" }]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs[0].path, "src/nested/file.rs");
    }

    #[test]
    fn drops_oversized_and_binary_content() {
        let big = "x".repeat(MAX_CONTENT_BYTES + 1);
        let value = json!([
            { "path": "big.txt", "content": big },
            { "path": "bin.dat", "content": "a\u{0}b" }
        ]);
        assert!(capture_proposed_changes(Some(&value), "x").is_empty());
    }

    #[test]
    fn invalid_baseline_hex_is_dropped_to_none() {
        let value = json!([
            { "path": "f.txt", "content": "x", "baseline_sha256": "not-hex" },
            { "path": "g.txt", "content": "x", "baseline_sha256": "ABCDEF" }
        ]);
        let cs = capture_proposed_changes(Some(&value), "x");
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].baseline_sha256, None);
        assert_eq!(cs[1].baseline_sha256, None);
    }

    #[test]
    fn count_is_capped() {
        let items: Vec<serde_json::Value> = (0..(MAX_PROPOSED_CHANGES + 10))
            .map(|i| json!({ "path": format!("f{i}.txt"), "content": "x" }))
            .collect();
        let cs = capture_proposed_changes(Some(&json!(items)), "x");
        assert_eq!(cs.len(), MAX_PROPOSED_CHANGES);
    }

    #[test]
    fn non_array_value_is_empty() {
        assert!(capture_proposed_changes(Some(&json!({"a": 1})), "x").is_empty());
        assert!(capture_proposed_changes(None, "x").is_empty());
    }

    #[test]
    fn empty_proposed_changes_omitted_from_wire() {
        let value = json!([{ "path": "f.txt", "content": "x" }]);
        let c = &capture_proposed_changes(Some(&value), "x")[0];
        let v = serde_json::to_value(c).unwrap();
        // Optional fields stay off the wire until set.
        assert!(v.get("review_note").is_none());
        assert!(v.get("refused_reason").is_none());
        assert!(v.get("applied_at").is_none());
        assert_eq!(v.get("status").and_then(|s| s.as_str()), Some("proposed"));
    }
}
