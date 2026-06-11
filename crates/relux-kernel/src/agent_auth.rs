//! Per-agent access tokens — the first safe per-agent identity primitive.
//!
//! Until now Relux had **no per-agent auth identity**: a manager agent could not
//! present its own credential on an HTTP request, so the manager-subtree grant
//! (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §19) was *operator-assisted* — the
//! dashboard operator stood in for the manager. This module adds a bounded,
//! revocable, hashed-at-rest **agent access token** the local operator can mint
//! for a specific agent. A request carrying a valid token is authenticated **as
//! that agent** and admitted on a tiny allowlisted route subset only (agent
//! self-info + the manager-grant-as-self path) — never the operator console.
//!
//! ## Reference mapping (reference-driven-development.md)
//!
//! - **Paperclip** `references/paperclip/server/src/agent-auth-jwt.ts` is the
//!   target shape: a per-agent credential whose subject (`sub`) is an agent id,
//!   with a bounded `exp`, verified before a request is attributed to that agent
//!   (`middleware/auth.ts` sets `req.actor = { type: "agent", agentId: claims.sub
//!   }`). Paperclip signs an HMAC-SHA256 JWT; Relux instead mints an **opaque
//!   high-entropy token stored only as its SHA-256 hash** — there is no
//!   multi-tenant verifier to satisfy and a hashed-at-rest opaque token is
//!   strictly simpler to revoke and impossible to forge from the stored file
//!   (the same rationale [`crate::auth`] uses for session ids). The agent id is
//!   the token's subject, exactly like `claims.sub`.
//! - **OpenClaw** `reference/openclaw-main/src/acp/session-lineage-meta.ts`
//!   (`subagentControlScope: "children" | "none"`, default narrow) grounds the
//!   discipline: a token's authority is **narrow by default** — it unlocks only
//!   the manager-grant-as-self path (where the manager's reach is still bounded
//!   to its own Branch by the unchanged kernel gate), nothing broader.
//!
//! ## Storage / safety
//!
//! The raw token is shown to the operator **once**, at mint, and never again —
//! only its SHA-256 hash is persisted (`dashboard-agent-tokens.json`, gitignored,
//! written through the same atomic, permission-restricted path as the admin
//! credential). A leaked file cannot be replayed: forging a cookie/token would
//! require inverting SHA-256 of a 256-bit CSPRNG token. Every token carries a
//! bounded TTL and is individually revocable by its public, non-secret
//! `token_id`. The raw token is never logged; the redactor also masks its
//! `relux_agt_` prefix defensively (`crate::redact` via `relux_core`). This is a
//! **local-first** primitive, not an internet auth system.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::auth::atomic_write_restricted;

/// The prefix every raw agent token carries, so a leaked token is recognizable
/// (and masked by the secret redactor). The body is 32 CSPRNG bytes, hex-encoded.
pub const AGENT_TOKEN_PREFIX: &str = "relux_agt_";

/// Default token lifetime when the operator does not specify one (30 days). Long
/// enough to be useful for an agent actor, short enough to age out.
pub const AGENT_TOKEN_DEFAULT_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Hard ceiling on a minted token's lifetime (90 days). An operator-supplied TTL
/// is clamped to `[AGENT_TOKEN_MIN_TTL_SECS, AGENT_TOKEN_MAX_TTL_SECS]` so a token
/// can never be minted effectively-immortal.
pub const AGENT_TOKEN_MAX_TTL_SECS: i64 = 90 * 24 * 60 * 60;

/// Floor on a minted token's lifetime (60 seconds) so a non-positive or tiny TTL
/// does not mint an already-dead token.
pub const AGENT_TOKEN_MIN_TTL_SECS: i64 = 60;

/// Max label length kept on a token record (bounded so the persisted file stays
/// small and a label can never carry a huge blob).
const MAX_LABEL_LEN: usize = 120;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Hash a raw token to the value stored on disk and used as the in-memory map key.
/// SHA-256 is correct here (not Argon2): the token is a 256-bit CSPRNG secret, so
/// preimage resistance already makes the hash unforgeable — there is no
/// low-entropy secret to slow-hash. Storing only this hash means a leaked token
/// file cannot be replayed.
fn hash_token(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// One persisted agent-token row. Holds the SHA-256 **hash** of the opaque token
/// (never the raw token) plus the subject agent id, a public non-secret handle
/// (`token_id`) for display/revocation, and the bounded deadline. A reader of the
/// file learns which agents have live tokens and until when, but cannot mint a
/// request: forging one would require inverting SHA-256 of a 256-bit token.
#[derive(Clone, Serialize, Deserialize)]
struct PersistedAgentToken {
    /// Public, non-secret id used to display + revoke this token (`agt_<hex>`).
    token_id: String,
    /// SHA-256 hex of the raw token. The map key; never the raw value.
    token_hash: String,
    /// The subject: the agent this token authenticates **as**.
    agent_id: String,
    /// Operator-supplied label (bounded, sanitized) for the dashboard list.
    label: String,
    created_at: i64,
    expires_at: i64,
}

/// On-disk shape: a versioned envelope around the rows, so the format can evolve
/// without a silent misparse. An unknown/garbled file fails to deserialize and is
/// treated as "no tokens" (fail-closed: a corrupt file revokes everyone rather
/// than admitting a stale token).
#[derive(Serialize, Deserialize)]
struct AgentTokenFile {
    version: u32,
    tokens: Vec<PersistedAgentToken>,
}

const AGENT_TOKEN_FILE_VERSION: u32 = 1;

/// The identity a valid agent token resolves to: the subject agent and the public
/// token handle that authenticated the request. Inserted into the request
/// extensions by the serve middleware and read by the agent-self handlers — the
/// manager id is taken from `agent_id` (the token subject), never from the
/// request body, so a token can only ever act **as itself**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTokenIdentity {
    /// The subject agent id (Paperclip `claims.sub`).
    pub agent_id: String,
    /// The public, non-secret token handle that authenticated this request.
    pub token_id: String,
}

/// The result of minting a token. The `secret` is the ONLY time the raw token is
/// available — it is never stored in plaintext and never shown again. Everything
/// else is non-secret metadata safe to persist/display.
#[derive(Debug, Clone)]
pub struct MintedAgentToken {
    pub token_id: String,
    /// The raw token, shown to the operator exactly once.
    pub secret: String,
    pub agent_id: String,
    pub label: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Non-secret metadata about a live token, for the dashboard list. Never carries
/// the hash or the raw secret.
#[derive(Debug, Clone)]
pub struct AgentTokenMeta {
    pub token_id: String,
    pub agent_id: String,
    pub label: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// File-backed agent-token store keyed by `hash_token(raw)`, mirrored to a durable
/// file so tokens survive a `serve` restart. Every method takes the **raw** token
/// (the request credential) and hashes it internally; the raw token is never
/// stored in memory or on disk. Cloned cheaply (Arc inside).
#[derive(Clone)]
pub struct AgentTokenStore {
    inner: Arc<RwLock<HashMap<String, PersistedAgentToken>>>,
    /// Durable backing file. `None` keeps the table purely in-memory (a test seam);
    /// `Some` mirrors every mutation to disk atomically.
    path: Option<Arc<PathBuf>>,
}

impl AgentTokenStore {
    /// Where the agent-token file lives given the local DB path:
    /// `dashboard-agent-tokens.json` in the SAME directory as the admin credential
    /// and session file, so it sits with the operator's other local Relux state.
    pub fn path_for_db(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.join("dashboard-agent-tokens.json"))
            .unwrap_or_else(|| PathBuf::from("dashboard-agent-tokens.json"))
    }

    /// Build a store backed by `path`, loading any still-live tokens and pruning
    /// expired rows. A missing or unparseable file yields an empty table — never an
    /// error — so a corrupt file just re-prompts the operator to mint, rather than
    /// bricking serve. If anything was pruned on load, the pruned set is rewritten.
    pub fn from_path(path: &Path) -> Self {
        Self::load(Some(path.to_path_buf()))
    }

    /// In-memory-only store (test seam; no durability).
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::load(None)
    }

    fn load(path: Option<PathBuf>) -> Self {
        let mut map = HashMap::new();
        let mut pruned = false;
        if let Some(p) = path.as_ref() {
            if let Ok(bytes) = std::fs::read(p) {
                if let Ok(file) = serde_json::from_slice::<AgentTokenFile>(&bytes) {
                    let now = now_secs();
                    for rec in file.tokens {
                        if rec.expires_at > now {
                            map.insert(rec.token_hash.clone(), rec);
                        } else {
                            pruned = true;
                        }
                    }
                }
            }
        }
        let store = Self {
            inner: Arc::new(RwLock::new(map)),
            path: path.map(Arc::new),
        };
        if pruned {
            if let Ok(m) = store.inner.read() {
                store.persist_locked(&m);
            }
        }
        store
    }

    /// Atomically mirror the live rows of `map` to the backing file. Called while
    /// the caller holds the table lock so the on-disk image always matches a
    /// consistent in-memory snapshot. Only live (unexpired) rows are written, so
    /// persistence doubles as pruning. Best-effort: a write failure is swallowed —
    /// it costs durability across the next restart, never correctness of the
    /// running process. No-op when the store is in-memory only.
    fn persist_locked(&self, map: &HashMap<String, PersistedAgentToken>) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let now = now_secs();
        let mut tokens: Vec<PersistedAgentToken> = map
            .values()
            .filter(|t| t.expires_at > now)
            .cloned()
            .collect();
        // Deterministic on-disk order (by public id) so the file is diff-stable.
        tokens.sort_by(|a, b| a.token_id.cmp(&b.token_id));
        let file = AgentTokenFile {
            version: AGENT_TOKEN_FILE_VERSION,
            tokens,
        };
        if let Ok(body) = serde_json::to_vec_pretty(&file) {
            let _ = atomic_write_restricted(path, &body);
        }
    }

    /// Mint a fresh token for `agent_id`. Returns the raw token ONCE; only its hash
    /// is persisted. `ttl_secs` is clamped to the bounded window; `label` is trimmed
    /// and length-capped. The caller is responsible for verifying the agent exists
    /// (the store is identity-agnostic — it mints for whatever subject it is given).
    pub fn mint(
        &self,
        agent_id: &str,
        label: &str,
        ttl_secs: Option<i64>,
    ) -> MintedAgentToken {
        let ttl = ttl_secs
            .unwrap_or(AGENT_TOKEN_DEFAULT_TTL_SECS)
            .clamp(AGENT_TOKEN_MIN_TTL_SECS, AGENT_TOKEN_MAX_TTL_SECS);
        let now = now_secs();
        let expires_at = now + ttl;

        // 32 CSPRNG bytes → the secret body; 6 more → the public handle.
        let mut secret_buf = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut secret_buf);
        let secret = format!("{AGENT_TOKEN_PREFIX}{}", hex::encode(secret_buf));
        let mut id_buf = [0u8; 6];
        rand::rngs::OsRng.fill_bytes(&mut id_buf);
        let token_id = format!("agt_{}", hex::encode(id_buf));

        let label = sanitize_label(label);
        let rec = PersistedAgentToken {
            token_id: token_id.clone(),
            token_hash: hash_token(&secret),
            agent_id: agent_id.to_string(),
            label: label.clone(),
            created_at: now,
            expires_at,
        };
        if let Ok(mut m) = self.inner.write() {
            m.insert(rec.token_hash.clone(), rec);
            self.persist_locked(&m);
        }
        MintedAgentToken {
            token_id,
            secret,
            agent_id: agent_id.to_string(),
            label,
            created_at: now,
            expires_at,
        }
    }

    /// Authenticate a raw token: hash it, look it up, and return the subject
    /// identity iff it exists and has not expired. An expired token is pruned. A
    /// missing/garbage token returns `None`. This is the single chokepoint the
    /// serve agent-token middleware calls.
    pub fn authenticate(&self, raw: &str) -> Option<AgentTokenIdentity> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let key = hash_token(raw);
        let now = now_secs();
        // Fast read path.
        if let Ok(m) = self.inner.read() {
            match m.get(&key) {
                Some(t) if t.expires_at > now => {
                    return Some(AgentTokenIdentity {
                        agent_id: t.agent_id.clone(),
                        token_id: t.token_id.clone(),
                    });
                }
                Some(_) => {} // expired → fall through to prune
                None => return None,
            }
        }
        // Prune the expired entry.
        if let Ok(mut m) = self.inner.write() {
            if m.remove(&key).is_some() {
                self.persist_locked(&m);
            }
        }
        None
    }

    /// Revoke the token with public id `token_id` belonging to `agent_id`. Scoped
    /// to the agent so the operator console's `/agents/:id/tokens/:token_id` path
    /// can only revoke that agent's own tokens. Returns whether a row was removed
    /// (false → an honest 404 at the HTTP layer).
    pub fn revoke(&self, agent_id: &str, token_id: &str) -> bool {
        if let Ok(mut m) = self.inner.write() {
            let before = m.len();
            m.retain(|_, t| !(t.agent_id == agent_id && t.token_id == token_id));
            if m.len() != before {
                self.persist_locked(&m);
                return true;
            }
        }
        false
    }

    /// Non-secret metadata for every live token of `agent_id`, newest first. Never
    /// returns the hash or the raw secret.
    pub fn list_for_agent(&self, agent_id: &str) -> Vec<AgentTokenMeta> {
        let now = now_secs();
        let mut out: Vec<AgentTokenMeta> = self
            .inner
            .read()
            .map(|m| {
                m.values()
                    .filter(|t| t.agent_id == agent_id && t.expires_at > now)
                    .map(|t| AgentTokenMeta {
                        token_id: t.token_id.clone(),
                        agent_id: t.agent_id.clone(),
                        label: t.label.clone(),
                        created_at: t.created_at,
                        expires_at: t.expires_at,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.token_id.cmp(&b.token_id)));
        out
    }
}

/// Trim a label, strip control characters, and cap its length so a persisted token
/// row stays small and a label cannot smuggle newlines/control bytes.
fn sanitize_label(label: &str) -> String {
    let cleaned: String = label
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_LEN)
        .collect();
    cleaned
}

/// Pull a bearer token out of a request's `Authorization` header. Accepts
/// `Authorization: Bearer <token>` (case-insensitive scheme). Returns `None` when
/// the header is absent, malformed, or carries an empty token.
pub fn bearer_token_from_headers(headers: &axum::http::header::HeaderMap) -> Option<String> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?;
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (AgentTokenStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dashboard-agent-tokens.json");
        (AgentTokenStore::from_path(&path), tmp)
    }

    #[test]
    fn mint_then_authenticate_roundtrips_and_token_is_scoped_to_its_agent() {
        let (store, _tmp) = store();
        let minted = store.mint("lead-1", "ci runner", None);
        assert!(minted.secret.starts_with(AGENT_TOKEN_PREFIX));
        assert_eq!(minted.agent_id, "lead-1");
        // A valid token authenticates as exactly its subject agent.
        let id = store.authenticate(&minted.secret).expect("valid token authenticates");
        assert_eq!(id.agent_id, "lead-1");
        assert_eq!(id.token_id, minted.token_id);
        // A garbage / unknown token does not authenticate.
        assert!(store.authenticate("relux_agt_deadbeef").is_none());
        assert!(store.authenticate("").is_none());
    }

    #[test]
    fn the_raw_token_is_never_persisted_only_its_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dashboard-agent-tokens.json");
        let store = AgentTokenStore::from_path(&path);
        let minted = store.mint("lead-1", "ci", None);
        let on_disk = std::fs::read_to_string(&path).expect("token file written");
        // The raw secret must NOT appear in the persisted file in any form.
        assert!(
            !on_disk.contains(&minted.secret),
            "raw token leaked to disk: {on_disk}"
        );
        // The SHA-256 hash of the secret IS what is stored.
        assert!(
            on_disk.contains(&hash_token(&minted.secret)),
            "expected the token hash on disk"
        );
        // The public, non-secret handle is fine to store/display.
        assert!(on_disk.contains(&minted.token_id));
    }

    #[test]
    fn revoke_invalidates_the_token_and_is_agent_scoped() {
        let (store, _tmp) = store();
        let a = store.mint("lead-1", "a", None);
        let b = store.mint("lead-2", "b", None);
        // Revoking with the wrong agent id does nothing (scoped).
        assert!(!store.revoke("lead-2", &a.token_id));
        assert!(store.authenticate(&a.secret).is_some());
        // Revoking with the right agent id removes exactly that token.
        assert!(store.revoke("lead-1", &a.token_id));
        assert!(store.authenticate(&a.secret).is_none());
        // The other agent's token is untouched.
        assert!(store.authenticate(&b.secret).is_some());
        // Revoking an unknown token id is an honest false.
        assert!(!store.revoke("lead-2", "agt_nope"));
    }

    #[test]
    fn an_expired_token_does_not_authenticate_and_is_pruned() {
        let (store, _tmp) = store();
        // Mint, then forcibly back-date its expiry by rewriting the in-memory row.
        let minted = store.mint("lead-1", "short", Some(AGENT_TOKEN_MIN_TTL_SECS));
        {
            let mut m = store.inner.write().unwrap();
            for t in m.values_mut() {
                t.expires_at = now_secs() - 1;
            }
            // Don't persist the back-date; we only need the in-memory state expired.
        }
        assert!(
            store.authenticate(&minted.secret).is_none(),
            "an expired token must not authenticate"
        );
        // It was pruned from the in-memory table.
        assert!(store.inner.read().unwrap().is_empty());
    }

    #[test]
    fn ttl_is_clamped_to_the_bounded_window() {
        let (store, _tmp) = store();
        // A huge TTL is clamped to the max ceiling.
        let big = store.mint("lead-1", "", Some(i64::MAX));
        assert!(big.expires_at - big.created_at <= AGENT_TOKEN_MAX_TTL_SECS);
        // A non-positive TTL is clamped up to the floor (never mints a dead token).
        let tiny = store.mint("lead-1", "", Some(-100));
        assert!(tiny.expires_at - tiny.created_at >= AGENT_TOKEN_MIN_TTL_SECS);
    }

    #[test]
    fn list_for_agent_returns_metadata_without_secrets() {
        let (store, _tmp) = store();
        let m1 = store.mint("lead-1", "first", None);
        let _m2 = store.mint("lead-1", "second", None);
        let _other = store.mint("lead-2", "other", None);
        let list = store.list_for_agent("lead-1");
        assert_eq!(list.len(), 2, "only lead-1's tokens");
        assert!(list.iter().all(|m| m.agent_id == "lead-1"));
        assert!(list.iter().any(|m| m.token_id == m1.token_id && m.label == "first"));
    }

    #[test]
    fn tokens_survive_a_simulated_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dashboard-agent-tokens.json");
        let s1 = AgentTokenStore::from_path(&path);
        let minted = s1.mint("lead-1", "persist", None);
        // A fresh handle on the same file (a serve restart) still authenticates it.
        let s2 = AgentTokenStore::from_path(&path);
        let id = s2.authenticate(&minted.secret).expect("token survives restart");
        assert_eq!(id.agent_id, "lead-1");
    }

    #[test]
    fn bearer_header_parsing() {
        use axum::http::header::{HeaderMap, HeaderValue, AUTHORIZATION};
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer relux_agt_abc"));
        assert_eq!(bearer_token_from_headers(&h).as_deref(), Some("relux_agt_abc"));
        // Case-insensitive scheme.
        h.insert(AUTHORIZATION, HeaderValue::from_static("bearer relux_agt_xyz"));
        assert_eq!(bearer_token_from_headers(&h).as_deref(), Some("relux_agt_xyz"));
        // Empty / malformed → None.
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert!(bearer_token_from_headers(&h).is_none());
        h.insert(AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert!(bearer_token_from_headers(&h).is_none());
    }
}
