//! Local secret store — pure types, validation, and redaction (no I/O).
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 17.5 (permissions / safety) +
//! section 8.2 (ToolSet plugins / adapters needing credentials), and `docs/mcp.md`
//! "Local secrets & environment". A real product must let an operator supply API
//! keys / tokens for managed-stdio MCP servers (and future adapters) WITHOUT
//! hard-coding them or ever echoing them back. This module defines the pure,
//! serializable contract for that store; the kernel
//! (`relux-kernel::secret_store`) owns the actual file-backed, permission-hardened
//! storage and resolution.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! Read before writing this module:
//!
//! - **Hermes** `reference/hermes-agent-main/hermes_cli/mcp_config.py`: stdio MCP
//!   servers carry `{"command","args","env"}`; a per-server API key is stored in a
//!   separate `~/.hermes/.env` keyed `MCP_<NAME>_API_KEY` (`_env_key_for_server`,
//!   L107-109) and referenced from the server config via `${ENV_VAR}` interpolation
//!   (`save_env_value` / `_ENV_VAR_PATTERN`); `cmd_mcp_test` MASKS the resolved value
//!   (`resolved[:4] + "***" + resolved[-4:]`, L553-560) — it never prints the raw
//!   secret. We mirror the posture: env values are SECRET REFERENCES in the config
//!   (never literals), the store keeps the plaintext locally, and every operator-facing
//!   surface shows only a redacted preview.
//! - **Relix legacy** `crates/relix-web-bridge/src/secrets.rs`: a separate
//!   permission-restricted file (`bridge-secrets.toml`, mode 0600 / icacls), the
//!   dashboard never receives a raw secret back (`ProviderStatus` carries only
//!   `key_preview` = ellipsis + last 4 chars via `redact`). We port the same
//!   "no-plaintext-return + redacted preview + permission-hardened file" contract to
//!   the relux layer.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Max characters accepted for a secret name. The name is the store key and is
/// referenced from a managed-stdio MCP server's `env` map, so it is bounded and
/// restricted to a safe identifier charset.
pub const MAX_SECRET_NAME_CHARS: usize = 128;

/// Max bytes accepted for a secret value. API keys / bearer tokens are short, but
/// some credentials (PEM keys, service-account JSON) are larger — 16 KiB is a
/// generous bound that still stops a runaway paste from bloating the local store.
pub const MAX_SECRET_VALUE_BYTES: usize = 16 * 1024;

/// Hard cap on how many distinct secrets the local store keeps, so a misbehaving
/// caller cannot grow the file unboundedly.
pub const MAX_SECRETS: usize = 256;

/// At-rest encoding scheme: **Windows DPAPI, CurrentUser scope**. The stored value
/// is base64 of the `CryptProtectData` blob — only the same Windows user account on
/// the same machine can decrypt it. The default writer on Windows.
pub const SECRET_SCHEME_DPAPI: &str = "dpapi_current_user";

/// At-rest encoding scheme: **plaintext** in the permission-hardened file (POSIX
/// `0600` / Windows `icacls` owner-only). The value is stored verbatim. Used on
/// non-Windows hosts (no OS keychain integration yet) and as the fail-safe fallback
/// when DPAPI is unavailable. Also the assumed scheme for a pre-encryption (legacy)
/// file with no per-secret scheme marker.
pub const SECRET_SCHEME_PLAINTEXT: &str = "plaintext_file_v1";

/// Whether `name` is a safe secret name: non-empty, at most [`MAX_SECRET_NAME_CHARS`]
/// characters, and restricted to `[A-Za-z0-9._-]`. The name is the store key and is
/// echoed into config / status / logs (never the value), so it must never carry
/// whitespace, path separators, or other injection-shaped characters.
pub fn is_valid_secret_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.chars().count() <= MAX_SECRET_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Why setting a secret was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("secret name must not be empty")]
    EmptyName,
    #[error(
        "secret name must use only letters, digits, '.', '-' or '_' (max {MAX_SECRET_NAME_CHARS} chars)"
    )]
    InvalidName,
    #[error("secret value must not be empty")]
    EmptyValue,
    #[error("secret value is too large ({size} bytes; max {MAX_SECRET_VALUE_BYTES})")]
    ValueTooLarge { size: usize },
    #[error("secret store is full (max {MAX_SECRETS} secrets); delete one before adding another")]
    StoreFull,
}

/// Validate a candidate secret name + value against the store's bounds. Pure — no
/// I/O, no storage. The kernel store calls this before persisting.
pub fn validate_secret(name: &str, value: &str) -> Result<(), SecretError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(SecretError::EmptyName);
    }
    if !is_valid_secret_name(trimmed) {
        return Err(SecretError::InvalidName);
    }
    if value.is_empty() {
        return Err(SecretError::EmptyValue);
    }
    if value.len() > MAX_SECRET_VALUE_BYTES {
        return Err(SecretError::ValueTooLarge { size: value.len() });
    }
    Ok(())
}

/// A redacted, operator-facing view of one stored secret. Carries the name, the
/// wall-clock seconds it was last set, and a **redacted preview** — NEVER the value.
/// This is the only shape any HTTP response / UI / log ever sees for a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretStatus {
    /// The secret's name (the store key).
    pub name: String,
    /// Wall-clock unix seconds the secret was last set.
    pub set_at: i64,
    /// A redacted preview (ellipsis + last 4 chars), or `None` for an empty value.
    /// Deliberately the TAIL, not the head — a provider prefix (`sk-`, `ghp_`, …)
    /// would be too revealing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// The at-rest encoding scheme this secret is stored under
    /// ([`SECRET_SCHEME_DPAPI`] / [`SECRET_SCHEME_PLAINTEXT`]). Operator-facing
    /// metadata so the dashboard can show "encrypted at rest (DPAPI)" vs
    /// "permission-hardened plaintext". Never the value. Defaults to plaintext for
    /// a status deserialized from an older payload that predates the field.
    #[serde(default = "default_scheme")]
    pub scheme: String,
}

/// Default scheme for a [`SecretStatus`] deserialized from a payload that predates
/// the `scheme` field — the only stores that ever existed before encryption were
/// plaintext.
fn default_scheme() -> String {
    SECRET_SCHEME_PLAINTEXT.to_string()
}

/// Return a redacted preview of `value`: an ellipsis plus the last 4 characters.
/// Empty values return `None`; values of 4 or fewer characters return the
/// `"…****"` sentinel so an obviously-short secret never leaks a fingerprint.
///
/// Mirrors `relix-web-bridge::secrets::redact` so the preview shape is identical
/// across the two layers.
pub fn secret_preview(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        return Some("…****".to_string());
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    Some(format!("…{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_are_safe_identifiers() {
        assert!(is_valid_secret_name("openrouter_api_key"));
        assert!(is_valid_secret_name("gh.token-1"));
        assert!(!is_valid_secret_name(""));
        assert!(!is_valid_secret_name("has space"));
        assert!(!is_valid_secret_name("a/b"));
        assert!(!is_valid_secret_name("a:b"));
        assert!(!is_valid_secret_name(&"a".repeat(MAX_SECRET_NAME_CHARS + 1)));
    }

    #[test]
    fn validate_secret_enforces_bounds() {
        assert_eq!(validate_secret("  ", "v"), Err(SecretError::EmptyName));
        assert_eq!(validate_secret("bad name", "v"), Err(SecretError::InvalidName));
        assert_eq!(validate_secret("ok", ""), Err(SecretError::EmptyValue));
        let big = "x".repeat(MAX_SECRET_VALUE_BYTES + 1);
        assert!(matches!(
            validate_secret("ok", &big),
            Err(SecretError::ValueTooLarge { .. })
        ));
        assert!(validate_secret("ok", "a-real-key").is_ok());
    }

    #[test]
    fn preview_never_leaks_the_value() {
        assert_eq!(secret_preview(""), None);
        assert_eq!(secret_preview("a").as_deref(), Some("…****"));
        assert_eq!(secret_preview("abcd").as_deref(), Some("…****"));
        // A realistic key reveals only its last 4 chars.
        let key = ["sk", "test", "1234567890abcdef"].join("-");
        let prev = secret_preview(&key).unwrap();
        assert_eq!(prev, "…cdef");
        assert!(!prev.contains("1234567890"));
    }

    #[test]
    fn status_serialization_carries_no_value_field() {
        let s = SecretStatus {
            name: "openai".to_string(),
            set_at: 1_700_000_000,
            preview: Some("…cdef".to_string()),
            scheme: SECRET_SCHEME_DPAPI.to_string(),
        };
        let v = serde_json::to_value(&s).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        // Only name + set_at + preview + scheme — never a `value`.
        assert!(keys.contains(&"name"));
        assert!(keys.contains(&"set_at"));
        assert!(keys.contains(&"preview"));
        assert!(keys.contains(&"scheme"));
        assert!(!keys.contains(&"value"));
    }

    #[test]
    fn status_defaults_scheme_to_plaintext_for_legacy_payload() {
        // A status JSON written before the `scheme` field existed must deserialize
        // with the plaintext default — never panic, never lose the row.
        let legacy = r#"{"name":"openai","set_at":1700000000,"preview":"…cdef"}"#;
        let s: SecretStatus = serde_json::from_str(legacy).unwrap();
        assert_eq!(s.scheme, SECRET_SCHEME_PLAINTEXT);
    }
}
