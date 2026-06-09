//! Per-plugin tool runtime configuration (the HTTP loopback ToolSet runtime).
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.2 (ToolSet Plugins) and
//! section 18 (no auto-running of downloaded plugin code). Relux does NOT execute
//! arbitrary downloaded plugin code. A ToolSet plugin becomes executable only when
//! the operator explicitly configures a **loopback HTTP endpoint** for it - a
//! server the operator runs themselves. The kernel then calls that server through
//! a narrow, permission-checked, audited protocol.
//!
//! These are pure types + a validation helper. The kernel client that actually
//! speaks to the loopback server lives in `relux-kernel::runtime`.
//!
//! Safety rules pinned here:
//!
//! - Only `http://` loopback URLs are accepted: `127.0.0.1`, `localhost`, or
//!   `[::1]`, with an explicit port. `https`, remote hosts, embedded credentials,
//!   query/fragment, and traversal-shaped paths are all rejected.
//! - This config NEVER stores secrets - only a base URL, an enabled flag, and a
//!   per-call timeout.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default per-call timeout for an HTTP loopback runtime, in milliseconds.
pub const DEFAULT_RUNTIME_TIMEOUT_MS: u64 = 5_000;
/// Lower clamp for a configured timeout (a stray tiny value can't make every call
/// time out instantly).
pub const MIN_RUNTIME_TIMEOUT_MS: u64 = 100;
/// Upper clamp for a configured timeout (a stray huge value can't make a call
/// hang for minutes).
pub const MAX_RUNTIME_TIMEOUT_MS: u64 = 60_000;

/// The kind of runtime configured for an installed plugin.
///
/// Only one kind exists today - an operator-run loopback HTTP server. It is an
/// enum so future safe runtimes can be added without changing the wire shape of
/// the rest of the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// The plugin is executed by POSTing JSON to a loopback HTTP server the
    /// operator started separately.
    HttpLoopback,
}

impl RuntimeKind {
    /// The stable wire string for this kind (`"http_loopback"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            RuntimeKind::HttpLoopback => "http_loopback",
        }
    }
}

/// Durable, per-installed-plugin runtime configuration.
///
/// Persisted locally alongside the rest of the control plane. It carries no
/// secrets - just how to reach the operator's loopback server, whether the
/// runtime is enabled, and the per-call timeout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRuntimeConfig {
    /// The installed plugin this runtime backs.
    pub plugin_id: String,
    /// The runtime kind (currently always `HttpLoopback`).
    pub kind: RuntimeKind,
    /// The validated loopback base URL, e.g. `http://127.0.0.1:19999`.
    pub base_url: String,
    /// Whether the runtime is currently enabled. A disabled runtime is kept (so
    /// the URL survives) but refuses invocations.
    pub enabled: bool,
    /// Per-call timeout in milliseconds (already clamped to a sane range).
    pub timeout_ms: u64,
}

/// Why a candidate loopback URL was rejected by [`validate_loopback_url`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LoopbackUrlError {
    #[error("runtime URL must not be empty")]
    Empty,
    #[error("runtime URL must not contain whitespace")]
    Whitespace,
    #[error("runtime URL must use the http:// scheme (loopback only), not https or another scheme")]
    NotHttp,
    #[error("runtime URL must not embed credentials")]
    EmbeddedCredentials,
    #[error("runtime URL must not contain a query or fragment")]
    QueryOrFragment,
    #[error("runtime URL path must not contain '..'")]
    TraversalPath,
    #[error("runtime URL host must be loopback (127.0.0.1, localhost, or [::1]), got '{0}'")]
    NonLoopbackHost(String),
    #[error("runtime URL must include an explicit port")]
    MissingPort,
    #[error("runtime URL has an invalid port: '{0}'")]
    InvalidPort(String),
    #[error("runtime URL is malformed: {0}")]
    Malformed(String),
}

/// The parsed, validated pieces of a loopback URL.
///
/// Returned by [`parse_loopback_url`] so a client does not re-implement the
/// parse. `host` is normalized to one of `127.0.0.1`, `localhost`, or `::1`
/// (bare, without brackets); `path` is the base path (`""` or e.g. `/relux`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackUrl {
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Validate that `url` is a safe loopback HTTP base URL.
///
/// Accepts `http://127.0.0.1:<port>`, `http://localhost:<port>`, or
/// `http://[::1]:<port>`, each with an optional non-traversal path. Rejects
/// `https`, remote hosts, embedded credentials, a missing port, query/fragment,
/// and `..` paths. See [`LoopbackUrlError`] for the exact reasons.
pub fn validate_loopback_url(url: &str) -> Result<(), LoopbackUrlError> {
    parse_loopback_url(url).map(|_| ())
}

/// Parse and validate a loopback URL into its [`LoopbackUrl`] parts.
pub fn parse_loopback_url(url: &str) -> Result<LoopbackUrl, LoopbackUrlError> {
    if url.is_empty() {
        return Err(LoopbackUrlError::Empty);
    }
    if url.chars().any(|c| c.is_whitespace()) {
        return Err(LoopbackUrlError::Whitespace);
    }

    // Scheme: only http:// (this rejects https:// and every other scheme).
    let rest = url
        .strip_prefix("http://")
        .ok_or(LoopbackUrlError::NotHttp)?;

    if rest.contains('?') || rest.contains('#') {
        return Err(LoopbackUrlError::QueryOrFragment);
    }

    // Split authority from the optional path.
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err(LoopbackUrlError::EmbeddedCredentials);
    }
    if path.contains("..") {
        return Err(LoopbackUrlError::TraversalPath);
    }
    // A lone trailing slash is meaningless for our `/invoke` join; drop it.
    let path = path.trim_end_matches('/').to_string();

    // Authority: host:port, with IPv6 hosts bracketed as [::1].
    let (host, port_str) = if let Some(after) = authority.strip_prefix('[') {
        // Bracketed IPv6: [host]:port
        let close = after
            .find(']')
            .ok_or_else(|| LoopbackUrlError::Malformed("unterminated IPv6 bracket".to_string()))?;
        let host = &after[..close];
        let tail = &after[close + 1..];
        let port = tail
            .strip_prefix(':')
            .ok_or(LoopbackUrlError::MissingPort)?;
        (host.to_string(), port.to_string())
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => return Err(LoopbackUrlError::MissingPort),
        }
    };

    // Host must be loopback. IPv6 host is the bare form `::1` (brackets stripped).
    let normalized_host = match host.as_str() {
        "127.0.0.1" => "127.0.0.1",
        "localhost" => "localhost",
        "::1" => "::1",
        other => return Err(LoopbackUrlError::NonLoopbackHost(other.to_string())),
    };

    if port_str.is_empty() {
        return Err(LoopbackUrlError::MissingPort);
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| LoopbackUrlError::InvalidPort(port_str.clone()))?;
    if port == 0 {
        return Err(LoopbackUrlError::InvalidPort(port_str));
    }

    Ok(LoopbackUrl {
        host: normalized_host.to_string(),
        port,
        path,
    })
}

/// Clamp a requested timeout into the supported range, defaulting when absent.
pub fn clamp_runtime_timeout(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .unwrap_or(DEFAULT_RUNTIME_TIMEOUT_MS)
        .clamp(MIN_RUNTIME_TIMEOUT_MS, MAX_RUNTIME_TIMEOUT_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_hosts_with_ports() {
        assert!(validate_loopback_url("http://127.0.0.1:19999").is_ok());
        assert!(validate_loopback_url("http://localhost:8080").is_ok());
        assert!(validate_loopback_url("http://[::1]:3000").is_ok());
        // An optional non-traversal path is allowed.
        assert!(validate_loopback_url("http://127.0.0.1:19999/relux").is_ok());
        assert!(validate_loopback_url("http://127.0.0.1:19999/").is_ok());
    }

    #[test]
    fn parses_parts() {
        let u = parse_loopback_url("http://127.0.0.1:19999/relux").unwrap();
        assert_eq!(u.host, "127.0.0.1");
        assert_eq!(u.port, 19999);
        assert_eq!(u.path, "/relux");

        let v6 = parse_loopback_url("http://[::1]:3000").unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.port, 3000);
        assert_eq!(v6.path, "");

        // A lone trailing slash normalizes to an empty base path.
        let slash = parse_loopback_url("http://localhost:8080/").unwrap();
        assert_eq!(slash.path, "");
    }

    #[test]
    fn rejects_https_and_other_schemes() {
        assert_eq!(
            validate_loopback_url("https://127.0.0.1:19999"),
            Err(LoopbackUrlError::NotHttp)
        );
        assert_eq!(
            validate_loopback_url("ftp://127.0.0.1:19999"),
            Err(LoopbackUrlError::NotHttp)
        );
        assert_eq!(
            validate_loopback_url("127.0.0.1:19999"),
            Err(LoopbackUrlError::NotHttp)
        );
    }

    #[test]
    fn rejects_remote_hosts() {
        assert_eq!(
            validate_loopback_url("http://example.com:80"),
            Err(LoopbackUrlError::NonLoopbackHost("example.com".to_string()))
        );
        assert_eq!(
            validate_loopback_url("http://10.0.0.5:8080"),
            Err(LoopbackUrlError::NonLoopbackHost("10.0.0.5".to_string()))
        );
        // A public IPv6 must be rejected too.
        assert!(matches!(
            validate_loopback_url("http://[2001:db8::1]:8080"),
            Err(LoopbackUrlError::NonLoopbackHost(_))
        ));
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert_eq!(
            validate_loopback_url("http://user:tok@127.0.0.1:19999"),
            Err(LoopbackUrlError::EmbeddedCredentials)
        );
        assert_eq!(
            validate_loopback_url("http://tok@localhost:8080"),
            Err(LoopbackUrlError::EmbeddedCredentials)
        );
    }

    #[test]
    fn rejects_missing_or_bad_port() {
        assert_eq!(
            validate_loopback_url("http://127.0.0.1"),
            Err(LoopbackUrlError::MissingPort)
        );
        assert_eq!(
            validate_loopback_url("http://localhost"),
            Err(LoopbackUrlError::MissingPort)
        );
        assert_eq!(
            validate_loopback_url("http://[::1]"),
            Err(LoopbackUrlError::MissingPort)
        );
        assert!(matches!(
            validate_loopback_url("http://127.0.0.1:notaport"),
            Err(LoopbackUrlError::InvalidPort(_))
        ));
        assert!(matches!(
            validate_loopback_url("http://127.0.0.1:0"),
            Err(LoopbackUrlError::InvalidPort(_))
        ));
        // Port out of u16 range.
        assert!(matches!(
            validate_loopback_url("http://127.0.0.1:99999"),
            Err(LoopbackUrlError::InvalidPort(_))
        ));
    }

    #[test]
    fn rejects_weird_urls() {
        assert_eq!(validate_loopback_url(""), Err(LoopbackUrlError::Empty));
        assert_eq!(
            validate_loopback_url("http://127.0.0.1:19999 "),
            Err(LoopbackUrlError::Whitespace)
        );
        assert_eq!(
            validate_loopback_url("http://127.0.0.1:19999/../etc"),
            Err(LoopbackUrlError::TraversalPath)
        );
        assert_eq!(
            validate_loopback_url("http://127.0.0.1:19999?x=1"),
            Err(LoopbackUrlError::QueryOrFragment)
        );
        assert_eq!(
            validate_loopback_url("http://127.0.0.1:19999#frag"),
            Err(LoopbackUrlError::QueryOrFragment)
        );
    }

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(clamp_runtime_timeout(None), DEFAULT_RUNTIME_TIMEOUT_MS);
        assert_eq!(clamp_runtime_timeout(Some(1)), MIN_RUNTIME_TIMEOUT_MS);
        assert_eq!(
            clamp_runtime_timeout(Some(10_000_000)),
            MAX_RUNTIME_TIMEOUT_MS
        );
        assert_eq!(clamp_runtime_timeout(Some(2_500)), 2_500);
    }

    #[test]
    fn config_never_serializes_a_secret_field() {
        // The config type has no secret field by construction; this pins that the
        // serialized shape is exactly the safe, non-secret fields.
        let cfg = ToolRuntimeConfig {
            plugin_id: "relux-tools-demo".to_string(),
            kind: RuntimeKind::HttpLoopback,
            base_url: "http://127.0.0.1:19999".to_string(),
            enabled: true,
            timeout_ms: 5_000,
        };
        let v: serde_json::Value = serde_json::to_value(&cfg).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["base_url", "enabled", "kind", "plugin_id", "timeout_ms"]
        );
        assert_eq!(v["kind"], "http_loopback");
    }
}
