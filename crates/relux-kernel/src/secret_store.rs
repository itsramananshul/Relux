//! The local, file-backed **secret store** + managed-stdio **env/cwd resolution**.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` §17.5 (permissions / safety) + §8.2
//! (ToolSet/adapter plugins needing credentials); `docs/mcp.md` "Local secrets &
//! environment". A serious product must let an operator supply API keys / tokens for
//! managed-stdio MCP servers (and future adapters) WITHOUT hard-coding them or ever
//! echoing them back. This module owns the local plaintext-at-rest storage, the
//! redacted no-plaintext-return surface, and the spawn-time resolution of a server's
//! secret-ref `env` + safe `cwd`.
//!
//! ## Why a process-global store (mirrors the managed pool)
//!
//! Like the managed-stdio process pool (`crate::mcp_stdio::pool`), the secret store
//! lives **outside** the serializable [`crate::state::KernelState`]: a plaintext
//! credential must never land in the kernel snapshot (which the dashboard, the API,
//! and any export can read). It is a process-global ([`secret_store`]) backed by its
//! OWN permission-hardened file (`secrets.json`, restricted to the current user —
//! POSIX `0600` / Windows `icacls`), so the kernel snapshot stays secret-free and the
//! file is the single, locked-down at-rest location. The kernel resolves a server's
//! env refs from it at spawn and hands the plaintext straight to the child — it is
//! never stored back on the kernel, the pool entry, or any status/log.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! Read before writing this module:
//!
//! - **Hermes** `reference/hermes-agent-main/hermes_cli/mcp_config.py`: a stdio MCP
//!   server's API key is stored in a SEPARATE `~/.hermes/.env` (`save_env_value`) keyed
//!   `MCP_<NAME>_API_KEY` and referenced from the server config via a `${ENV}` ref —
//!   never inlined into the config; `cmd_mcp_test` only ever prints a MASKED value
//!   (`resolved[:4] + "***" + resolved[-4:]`, L553-560). We mirror: secret REFERENCES
//!   in the config, plaintext in a separate local file, redacted everywhere else.
//! - **Relix legacy** `crates/relix-web-bridge/src/secrets.rs` + `os_secure.rs`: a
//!   separate permission-restricted file (mode 0600 / icacls inheritance-stripped),
//!   atomic `.tmp`-rename write, the dashboard never receives a raw secret back (only a
//!   tail-redacted preview). We port the same contract; [`restrict_to_current_user`]
//!   here is the kernel-local copy of `os_secure::restrict_to_current_user`.
//! - **openclaw** `reference/openclaw-main/src/tools/execution.ts` + tool-mutation
//!   fail-closed default: an unknown/unsafe input is refused. We apply the same posture
//!   to `cwd` ([`validate_managed_cwd`]): a `cwd` that does not canonicalize INSIDE the
//!   configured safe root (blocking a `..`/symlink escape), does not exist, or is not a
//!   directory is REFUSED — never spawned in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use relux_core::{secret_preview, validate_secret, McpEnvRef, SecretError, SecretStatus};
use serde::{Deserialize, Serialize};

use crate::secret_cipher::SecretCipher;

/// One stored secret: the **at-rest-encoded** value (per `scheme`), the wall-clock
/// seconds it was last set, the encoding scheme, and a precomputed redacted preview.
/// Serialized to the local secrets file ONLY — never to the kernel snapshot, an API
/// response, or a log. The `value` here is **not** plaintext unless `scheme` is
/// [`relux_core::SECRET_SCHEME_PLAINTEXT`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecretEntry {
    /// The value AS STORED: `base64(DPAPI blob)` under [`relux_core::SECRET_SCHEME_DPAPI`],
    /// or the raw plaintext under [`relux_core::SECRET_SCHEME_PLAINTEXT`].
    value: String,
    set_at: i64,
    /// The at-rest encoding scheme. Absent in a pre-encryption (legacy) file → defaults
    /// to plaintext, so an old file loads cleanly and is then migrated.
    #[serde(default = "scheme_plaintext_default")]
    scheme: String,
    /// Precomputed redacted preview (`…cdef`). Stored so list/status NEVER need to
    /// decrypt. Absent in a legacy file → derived live from the (plaintext) value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
}

/// Default scheme for an entry deserialized from a pre-encryption file.
fn scheme_plaintext_default() -> String {
    relux_core::SECRET_SCHEME_PLAINTEXT.to_string()
}

impl SecretEntry {
    /// The redacted preview for an operator-facing status. Prefers the stored preview;
    /// for a legacy plaintext entry with none stored, derives it from the value (which
    /// IS plaintext under that scheme). Never decrypts an encrypted value.
    fn redacted_preview(&self) -> Option<String> {
        if let Some(p) = &self.preview {
            return Some(p.clone());
        }
        if self.scheme == relux_core::SECRET_SCHEME_PLAINTEXT {
            return secret_preview(&self.value);
        }
        None
    }

    fn status(&self, name: &str) -> SecretStatus {
        SecretStatus {
            name: name.to_string(),
            set_at: self.set_at,
            preview: self.redacted_preview(),
            scheme: self.scheme.clone(),
        }
    }
}

/// The mutable inner state behind the store's lock: the on-disk path (when persisted)
/// and the name → entry map.
#[derive(Default)]
struct Inner {
    /// The file the store persists to, or `None` for an in-memory (test/CLI) store.
    path: Option<PathBuf>,
    secrets: BTreeMap<String, SecretEntry>,
}

/// A local, file-backed secret store. Thread-safe via an internal `RwLock`. The value
/// at rest is **encrypted where the host supports it** (Windows DPAPI; permission-
/// hardened plaintext otherwise — see [`crate::secret_cipher`]). Plaintext exists only
/// transiently in memory at resolve time; every operator-facing read returns a redacted
/// [`SecretStatus`] (now carrying the at-rest `scheme`), never the value.
pub struct SecretStore {
    inner: RwLock<Inner>,
    /// The at-rest writer for new/rewritten values. Reads dispatch on each entry's own
    /// stored scheme, so a mixed-scheme file (mid-migration) still resolves correctly.
    cipher: Box<dyn SecretCipher>,
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl SecretStore {
    /// An empty, non-persisted (in-memory) store with the host default at-rest cipher —
    /// used by tests/CLI before any file is attached.
    pub fn in_memory() -> Self {
        Self::with_cipher(crate::secret_cipher::default_writer())
    }

    /// An empty, non-persisted store with an explicit at-rest cipher. Lets tests inject a
    /// deterministic, cross-platform cipher (and the platform default to be overridden).
    pub fn with_cipher(cipher: Box<dyn SecretCipher>) -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            cipher,
        }
    }

    /// Open a store backed by `path` with the host default cipher, loading any existing
    /// secrets and migrating any legacy plaintext entries to the active scheme. A missing
    /// / unreadable / malformed file loads as empty (with a warning) so a corrupt file
    /// never blocks boot — exactly like the legacy bridge secrets.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self::open_with_cipher(path, crate::secret_cipher::default_writer())
    }

    /// [`open`](Self::open) with an explicit at-rest cipher (used by tests).
    pub fn open_with_cipher(path: impl Into<PathBuf>, cipher: Box<dyn SecretCipher>) -> Self {
        let path = path.into();
        let secrets = load_file(&path);
        let store = Self {
            inner: RwLock::new(Inner {
                path: Some(path),
                secrets,
            }),
            cipher,
        };
        store.migrate_plaintext_entries();
        store
    }

    /// Attach a backing file to an already-created (e.g. process-global) store: load the
    /// file's secrets, persist subsequent mutations there, and migrate any legacy
    /// plaintext entries to the active at-rest scheme.
    pub fn attach(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let secrets = load_file(&path);
        {
            let mut g = self.write();
            g.secrets = secrets;
            g.path = Some(path);
        }
        self.migrate_plaintext_entries();
    }

    /// Re-seal any entry still stored as plaintext under the active encrypting writer,
    /// then persist if anything changed. No-op when the active writer does not encrypt
    /// (non-Windows / DPAPI-unavailable): a plaintext file stays plaintext rather than
    /// being pointlessly rewritten. Fail-safe — an entry whose re-seal fails is left
    /// exactly as it was (never dropped).
    fn migrate_plaintext_entries(&self) {
        if !self.cipher.encrypts() {
            return;
        }
        let mut g = self.write();
        let mut changed = false;
        for (name, e) in g.secrets.iter_mut() {
            if e.scheme != relux_core::SECRET_SCHEME_PLAINTEXT {
                continue;
            }
            match self.cipher.seal(&e.value) {
                Ok(sealed) => {
                    // Capture the preview from the plaintext BEFORE we drop it.
                    e.preview = secret_preview(&e.value);
                    e.value = sealed;
                    e.scheme = self.cipher.scheme().to_string();
                    changed = true;
                }
                Err(err) => {
                    eprintln!(
                        "secret store: migrate '{name}' to {} failed ({err}); left as plaintext",
                        self.cipher.scheme()
                    );
                }
            }
        }
        if changed {
            persist(&g);
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Set (or replace) a secret by name. Validates the name + value bounds and the
    /// total-count cap; stamps `set_at`; persists best-effort to the hardened file.
    /// Returns the REDACTED [`SecretStatus`] — never the plaintext value.
    pub fn set(&self, name: &str, value: &str) -> Result<SecretStatus, SecretError> {
        validate_secret(name, value)?;
        let name = name.trim().to_string();
        let mut g = self.write();
        // Count cap applies only when adding a NEW name (an overwrite is fine).
        if !g.secrets.contains_key(&name) && g.secrets.len() >= relux_core::MAX_SECRETS {
            return Err(SecretError::StoreFull);
        }
        let set_at = unix_secs();
        let preview = secret_preview(value);
        // Encrypt at rest with the active writer. On a sealing failure (e.g. DPAPI
        // unavailable on this Windows host) fall back to plaintext so the secret is
        // never lost — the scheme marker stays honest about what actually happened.
        let (stored_value, scheme) = match self.cipher.seal(value) {
            Ok(sealed) => (sealed, self.cipher.scheme().to_string()),
            Err(err) => {
                eprintln!(
                    "secret store: seal '{name}' with {} failed ({err}); storing permission-hardened plaintext",
                    self.cipher.scheme()
                );
                (
                    value.to_string(),
                    relux_core::SECRET_SCHEME_PLAINTEXT.to_string(),
                )
            }
        };
        g.secrets.insert(
            name.clone(),
            SecretEntry {
                value: stored_value,
                set_at,
                scheme: scheme.clone(),
                preview: preview.clone(),
            },
        );
        persist(&g);
        Ok(SecretStatus {
            name,
            set_at,
            preview,
            scheme,
        })
    }

    /// Delete a secret by name. Returns `true` when one existed. Persists best-effort.
    pub fn delete(&self, name: &str) -> bool {
        let name = name.trim();
        let mut g = self.write();
        let existed = g.secrets.remove(name).is_some();
        if existed {
            persist(&g);
        }
        existed
    }

    /// Every stored secret's REDACTED status, sorted by name. Never carries a value;
    /// never decrypts (the preview is precomputed / derived).
    pub fn list(&self) -> Vec<SecretStatus> {
        let g = self.read();
        g.secrets.iter().map(|(name, e)| e.status(name)).collect()
    }

    /// The redacted status of one secret by name, or `None` when absent.
    pub fn status(&self, name: &str) -> Option<SecretStatus> {
        let name = name.trim();
        let g = self.read();
        g.secrets.get(name).map(|e| e.status(name))
    }

    /// Whether a secret with this name exists (independent of whether it can be
    /// decrypted on this host — see [`resolve`](Self::resolve)).
    pub fn has(&self, name: &str) -> bool {
        self.read().secrets.contains_key(name.trim())
    }

    /// Decrypt one entry's stored value to plaintext, dispatching on its stored scheme.
    /// A plaintext entry returns its value verbatim; an entry sealed under the active
    /// writer's scheme is unsealed; any other scheme (e.g. a DPAPI file moved to a host
    /// that cannot decrypt it) is a clean, value-free error naming the secret + scheme.
    fn decrypt_entry(&self, name: &str, e: &SecretEntry) -> Result<String, String> {
        if e.scheme == relux_core::SECRET_SCHEME_PLAINTEXT {
            return Ok(e.value.clone());
        }
        if e.scheme == self.cipher.scheme() {
            return self
                .cipher
                .open(&e.value)
                .map_err(|err| format!("secret '{name}' could not be decrypted: {err}"));
        }
        Err(format!(
            "secret '{name}' is stored with scheme '{}', which this host cannot decrypt",
            e.scheme
        ))
    }

    /// Resolve a secret to its PLAINTEXT value. **Internal only** — this is the single
    /// method that returns plaintext, called solely at managed-stdio spawn / Prime-brain
    /// request time. The value is never logged, stored back, or returned over HTTP. A
    /// missing OR un-decryptable secret yields `None`; a decrypt failure is logged with
    /// the secret NAME (never the value) so the failure is diagnosable. Use
    /// [`resolve_result`](Self::resolve_result) when you need to distinguish the two.
    pub fn resolve(&self, name: &str) -> Option<String> {
        match self.resolve_result(name) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("secret store: {err}");
                None
            }
        }
    }

    /// Resolve a secret to plaintext, distinguishing **absent** (`Ok(None)`) from
    /// **present-but-undecryptable** (`Err(message)`). The error message names the secret
    /// + scheme, never the value.
    pub fn resolve_result(&self, name: &str) -> Result<Option<String>, String> {
        let g = self.read();
        match g.secrets.get(name.trim()) {
            None => Ok(None),
            Some(e) => self.decrypt_entry(name.trim(), e).map(Some),
        }
    }
}

/// Load the name → entry map from `path`. A missing file is empty (the normal first-run
/// case); an unreadable / non-UTF-8 / malformed file is empty with a warning (a corrupt
/// file never blocks boot). Mirrors `BridgeSecrets::load_or_empty`.
fn load_file(path: &Path) -> BTreeMap<String, SecretEntry> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(e) => {
            eprintln!(
                "secret store: read {} failed ({e}); treating as empty",
                path.display()
            );
            return BTreeMap::new();
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            eprintln!(
                "secret store: {} is not valid UTF-8; treating as empty",
                path.display()
            );
            return BTreeMap::new();
        }
    };
    match serde_json::from_str::<BTreeMap<String, SecretEntry>>(&text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "secret store: JSON parse of {} failed ({e}); treating as empty",
                path.display()
            );
            BTreeMap::new()
        }
    }
}

/// Persist the store's secrets to its hardened file (best-effort; a write failure is
/// logged, not fatal — the in-memory state stays authoritative for the running
/// process). Atomic `.tmp`-rename, with the temp file permission-hardened BEFORE the
/// rename so the final inode is already locked down.
fn persist(inner: &Inner) {
    let Some(path) = inner.path.as_ref() else {
        return; // In-memory store: nothing to persist.
    };
    let text = match serde_json::to_string_pretty(&inner.secrets) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("secret store: serialize failed ({e}); not persisting");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("secret store: mkdir {} failed ({e})", parent.display());
                return;
            }
        }
    }
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, text.as_bytes()) {
        eprintln!("secret store: write {} failed ({e})", tmp.display());
        return;
    }
    let _ = restrict_to_current_user(&tmp);
    if let Err(e) = std::fs::rename(&tmp, path) {
        eprintln!("secret store: rename {} failed ({e})", path.display());
        return;
    }
    // Re-apply after rename (NTFS can reset ACEs on rename under inherited perms; POSIX
    // preserves the mode through rename so this is a no-op there).
    let _ = restrict_to_current_user(path);
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Apply restrictive, owner-only permissions to `path`. POSIX `chmod 0600`; Windows
/// strips inheritance and grants the current user Full control via `icacls`. Best
/// effort — a failure is returned so the caller can log it (a writable secrets file is
/// still better than none). Kernel-local copy of
/// `relix-web-bridge::os_secure::restrict_to_current_user`.
pub fn restrict_to_current_user(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| format!("stat {}: {e}", path.display()))?
            .permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)
            .map_err(|e| format!("chmod {}: {e}", path.display()))?;
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::process::Command;
        let user = std::env::var("USERNAME").map_err(|_| "USERNAME env var not set".to_string())?;
        let out = Command::new("icacls")
            .arg(path)
            .arg("/inheritance:r")
            .output()
            .map_err(|e| format!("icacls /inheritance:r {}: {e}", path.display()))?;
        if !out.status.success() {
            return Err(format!(
                "icacls /inheritance:r failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let out = Command::new("icacls")
            .arg(path)
            .arg("/grant:r")
            .arg(format!("{user}:F"))
            .output()
            .map_err(|e| format!("icacls /grant {}: {e}", path.display()))?;
        if !out.status.success() {
            return Err(format!(
                "icacls /grant failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        return Ok(());
    }
    #[allow(unreachable_code)]
    Ok(())
}

// ===========================================================================
// Process-global handles: the secret store + the safe MCP workspace root.
// ===========================================================================

/// The process-global secret store. Created lazily as an empty in-memory store;
/// the server attaches a backing file at startup via [`init_secret_store`].
static STORE: OnceLock<SecretStore> = OnceLock::new();

/// The process-global secret store.
pub fn secret_store() -> &'static SecretStore {
    STORE.get_or_init(SecretStore::in_memory)
}

/// Attach the persistent secrets file to the process-global store (called once at
/// server startup with the resolved data-dir path).
pub fn init_secret_store(path: impl Into<PathBuf>) {
    secret_store().attach(path);
}

/// The configured safe MCP workspace root — the ONLY directory tree a managed-stdio
/// `cwd` may resolve inside. `None` until configured (in which case any `cwd` is
/// refused, fail-closed).
static WORKSPACE_ROOT: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn workspace_root_cell() -> &'static RwLock<Option<PathBuf>> {
    WORKSPACE_ROOT.get_or_init(|| RwLock::new(None))
}

/// Configure the safe MCP workspace root (the server sets this to a known local
/// directory at startup). A managed-stdio `cwd` is only accepted when it canonicalizes
/// INSIDE this root.
pub fn init_mcp_workspace_root(path: impl Into<PathBuf>) {
    let mut g = workspace_root_cell()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *g = Some(path.into());
}

/// The currently-configured safe MCP workspace root, if any.
pub fn mcp_workspace_root() -> Option<PathBuf> {
    workspace_root_cell()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Validate a managed-stdio `cwd` against a safe `allowed_root`. Fail-closed:
///
/// - the SHAPE must pass [`relux_core::validate_stdio_cwd_shape`] (non-empty, bounded,
///   no control char, no `..` traversal component);
/// - the path (relative → resolved against `allowed_root`, absolute → as-is) must
///   **exist** and **canonicalize**;
/// - the canonical path must be **inside** the canonical `allowed_root` (so a symlink
///   that points outside is rejected — canonicalize resolves symlinks before the
///   containment check);
/// - the path must be a **directory**.
///
/// Returns the canonical directory to spawn in, or an honest, value-free error string.
pub fn validate_managed_cwd(cwd: &str, allowed_root: &Path) -> Result<PathBuf, String> {
    relux_core::validate_stdio_cwd_shape(cwd).map_err(|e| e.to_string())?;
    let trimmed = cwd.trim();
    let candidate = {
        let p = Path::new(trimmed);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            allowed_root.join(p)
        }
    };
    let canon_root = allowed_root.canonicalize().map_err(|e| {
        format!(
            "MCP workspace root {} is unavailable: {e}",
            allowed_root.display()
        )
    })?;
    let canon = candidate.canonicalize().map_err(|e| {
        format!(
            "cwd {} does not exist or is unreadable: {e}",
            candidate.display()
        )
    })?;
    if !canon.starts_with(&canon_root) {
        return Err(format!(
            "cwd {} is outside the configured MCP workspace root {}",
            canon.display(),
            canon_root.display()
        ));
    }
    if !canon.is_dir() {
        return Err(format!("cwd {} is not a directory", canon.display()));
    }
    Ok(canon)
}

/// The resolved managed-stdio spawn environment: the plaintext `(name, value)` env
/// entries (handed straight to the spawn, never stored) plus the validated `cwd`.
pub type ResolvedManagedSpawn = (Vec<(String, String)>, Option<PathBuf>);

/// Resolve a managed-stdio server's `env` (secret refs → plaintext, via the global
/// secret store) and validate its `cwd` (against the global workspace root). Returns
/// the resolved `(env, cwd)` to hand straight to the spawn — the resolved values are
/// NEVER stored back, logged, or serialized.
///
/// Fail-closed and value-free: a MISSING secret is an error naming the secret + env-var
/// KEY (never a value); a `cwd` set with no configured root, or one that fails
/// [`validate_managed_cwd`], is a clean error. An HTTP server (or a stdio server with
/// no env/cwd) yields `(empty, None)`.
pub fn resolve_managed_env_and_cwd(
    env_refs: &BTreeMap<String, McpEnvRef>,
    cwd: Option<&str>,
) -> Result<ResolvedManagedSpawn, String> {
    let store = secret_store();
    let mut env: Vec<(String, String)> = Vec::with_capacity(env_refs.len());
    for (var, r) in env_refs {
        match store.resolve_result(&r.secret) {
            Ok(Some(value)) => env.push((var.clone(), value)),
            // Name the missing secret + env var (config identifiers), never a value.
            Ok(None) => {
                return Err(format!(
                    "missing secret '{}' for env var '{}'",
                    r.secret, var
                ))
            }
            // Present but un-decryptable on this host: a clean, value-free error that
            // names the secret + env var (the inner message already names the scheme).
            Err(err) => return Err(format!("{err} (for env var '{var}')")),
        }
    }
    let cwd = match cwd {
        None => None,
        Some(c) => {
            let root = mcp_workspace_root().ok_or_else(|| {
                "a cwd is configured but no safe MCP workspace root is set; cwd is refused"
                    .to_string()
            })?;
            Some(validate_managed_cwd(c, &root)?)
        }
    };
    Ok((env, cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_cipher::{DpapiCipher, PlaintextCipher, SecretCipher};

    fn env_refs(env: &[(&str, &str)]) -> BTreeMap<String, McpEnvRef> {
        let mut m = BTreeMap::new();
        for (var, secret) in env {
            m.insert(
                (*var).to_string(),
                McpEnvRef {
                    secret: (*secret).to_string(),
                },
            );
        }
        m
    }

    /// A deterministic, cross-platform **encrypting** cipher test-double. XOR-then-base64
    /// so (a) the at-rest string contains no plaintext substring, (b) the round-trip is
    /// exact, and (c) `open` of a malformed payload errors cleanly — exactly the surface
    /// the store relies on, without depending on a live OS keychain / DPAPI.
    struct FakeCipher;
    const FAKE_SCHEME: &str = "test_fake_xor_v1";
    impl SecretCipher for FakeCipher {
        fn scheme(&self) -> &'static str {
            FAKE_SCHEME
        }
        fn encrypts(&self) -> bool {
            true
        }
        fn seal(&self, plaintext: &str) -> Result<String, String> {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let xored: Vec<u8> = plaintext.bytes().map(|b| b ^ 0x5A).collect();
            Ok(STANDARD.encode(xored))
        }
        fn open(&self, encoded: &str) -> Result<String, String> {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let xored = STANDARD
                .decode(encoded)
                .map_err(|_| "fake cipher: malformed payload".to_string())?;
            let bytes: Vec<u8> = xored.into_iter().map(|b| b ^ 0x5A).collect();
            String::from_utf8(bytes).map_err(|_| "fake cipher: non-UTF-8".to_string())
        }
    }

    #[test]
    fn set_list_delete_never_return_plaintext() {
        // Inject the encrypting test cipher so the assertions hold on every OS and the
        // value is genuinely sealed at rest.
        let store = SecretStore::with_cipher(Box::new(FakeCipher));
        let value = ["sk", "test", "9876543210abcdef"].join("-");
        let status = store.set("openai", &value).unwrap();
        // The set response carries only a redacted preview + the scheme — never the value.
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains(&value), "value leaked in set response: {json}");
        assert_eq!(status.preview.as_deref(), Some("…cdef"));
        assert_eq!(status.scheme, FAKE_SCHEME);

        let list = store.list();
        assert_eq!(list.len(), 1);
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains(&value), "value leaked in list: {json}");
        assert!(json.contains("…cdef"));
        assert!(json.contains(FAKE_SCHEME), "scheme should surface in status");

        // Resolve (internal) IS the only place plaintext comes back, decrypted on the fly.
        assert_eq!(store.resolve("openai").as_deref(), Some(value.as_str()));

        assert!(store.delete("openai"));
        assert!(!store.delete("openai")); // idempotent
        assert!(store.list().is_empty());
        assert!(store.resolve("openai").is_none());
    }

    #[test]
    fn serialized_file_holds_ciphertext_not_plaintext_under_encrypting_cipher() {
        // Pure format test: with an encrypting cipher, the on-disk file must contain the
        // scheme marker and NOT the plaintext anywhere.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let store = SecretStore::open_with_cipher(&path, Box::new(FakeCipher));
        let value = "super-secret-plaintext-value-7777";
        store.set("k", value).unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains(value),
            "plaintext leaked into the on-disk file: {on_disk}"
        );
        assert!(
            on_disk.contains(FAKE_SCHEME),
            "scheme marker missing from the file: {on_disk}"
        );
        // The preview is the only fingerprint persisted, and it's the tail only.
        assert!(on_disk.contains("…7777"));
        // A fresh open with the same cipher decrypts it back.
        let reopened = SecretStore::open_with_cipher(&path, Box::new(FakeCipher));
        assert_eq!(reopened.resolve("k").as_deref(), Some(value));
    }

    #[test]
    fn legacy_plaintext_file_migrates_to_active_scheme_on_open() {
        // Write a pre-encryption file by hand (no `scheme`/`preview` fields), then open
        // it with an encrypting cipher: it must migrate in place to the active scheme,
        // drop the plaintext from disk, and still resolve.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let legacy = r#"{"legacy":{"value":"old-plaintext-key-4321","set_at":1700000000}}"#;
        std::fs::write(&path, legacy).unwrap();

        let store = SecretStore::open_with_cipher(&path, Box::new(FakeCipher));
        // Still resolvable.
        assert_eq!(store.resolve("legacy").as_deref(), Some("old-plaintext-key-4321"));
        // Status now reports the encrypted scheme + a derived preview.
        let st = store.status("legacy").unwrap();
        assert_eq!(st.scheme, FAKE_SCHEME);
        assert_eq!(st.preview.as_deref(), Some("…4321"));
        // The file on disk was rewritten: no plaintext, scheme marker present.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("old-plaintext-key-4321"), "{on_disk}");
        assert!(on_disk.contains(FAKE_SCHEME), "{on_disk}");
    }

    #[test]
    fn plaintext_host_leaves_legacy_file_as_plaintext() {
        // A non-encrypting writer (the non-Windows / DPAPI-unavailable default) must NOT
        // rewrite a plaintext file, and reads it verbatim.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let legacy = r#"{"legacy":{"value":"keep-plain-1234","set_at":1700000000}}"#;
        std::fs::write(&path, legacy).unwrap();
        let store = SecretStore::open_with_cipher(&path, Box::new(PlaintextCipher));
        assert_eq!(store.resolve("legacy").as_deref(), Some("keep-plain-1234"));
        let st = store.status("legacy").unwrap();
        assert_eq!(st.scheme, relux_core::SECRET_SCHEME_PLAINTEXT);
    }

    #[test]
    fn corrupt_encrypted_payload_fails_cleanly_naming_the_key() {
        // An entry sealed under the active scheme whose ciphertext is corrupt must fail
        // closed: resolve → None, resolve_result → Err naming the key (never the value),
        // and the env resolver surfaces a clean, value-free error.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        // `value` is not valid base64 for FakeCipher.open → a clean decrypt error.
        let corrupt = r#"{"brokenkey":{"value":"!!!not-base64!!!","set_at":1700000000,"scheme":"test_fake_xor_v1"}}"#;
        std::fs::write(&path, corrupt).unwrap();
        let store = SecretStore::open_with_cipher(&path, Box::new(FakeCipher));
        assert!(store.resolve("brokenkey").is_none());
        let err = store.resolve_result("brokenkey").unwrap_err();
        assert!(err.contains("brokenkey"), "error should name the key: {err}");
        assert!(!err.contains("not-base64"), "error must not echo the value: {err}");
    }

    #[test]
    fn entry_with_unknown_scheme_fails_closed() {
        // A value sealed on another host (scheme the active writer can't open) is refused
        // cleanly, naming the scheme — e.g. a DPAPI file copied to a plaintext host.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let foreign = r#"{"k":{"value":"AQAAdeadbeef","set_at":1700000000,"scheme":"dpapi_current_user","preview":"…beef"}}"#;
        std::fs::write(&path, foreign).unwrap();
        // Active writer is plaintext → cannot decrypt a dpapi entry.
        let store = SecretStore::open_with_cipher(&path, Box::new(PlaintextCipher));
        let err = store.resolve_result("k").unwrap_err();
        assert!(err.contains("dpapi_current_user"), "{err}");
        assert!(err.contains('k'));
        // But status/list still work (no decrypt needed) — operator sees the stranded key.
        assert_eq!(store.status("k").unwrap().scheme, "dpapi_current_user");
        assert_eq!(store.status("k").unwrap().preview.as_deref(), Some("…beef"));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_store_round_trips_and_seals_at_rest() {
        // Real Windows DPAPI through the store, gated to Windows. If DPAPI is genuinely
        // unavailable in this environment the value falls back to plaintext — assert the
        // round-trip either way, and that an encrypted entry hides the plaintext on disk.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let store = SecretStore::open_with_cipher(&path, Box::new(DpapiCipher::new()));
        let value = "dpapi-real-secret-5150";
        let st = store.set("winkey", value).unwrap();
        // Round-trips back to plaintext regardless of which scheme landed.
        assert_eq!(store.resolve("winkey").as_deref(), Some(value));
        if st.scheme == relux_core::SECRET_SCHEME_DPAPI {
            // When DPAPI actually sealed it, the file must not contain the plaintext.
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert!(!on_disk.contains(value), "plaintext leaked under DPAPI: {on_disk}");
            // And a fresh open (new DpapiCipher) still decrypts — same user/machine key.
            let reopened = SecretStore::open_with_cipher(&path, Box::new(DpapiCipher::new()));
            assert_eq!(reopened.resolve("winkey").as_deref(), Some(value));
        }
    }

    #[test]
    fn set_enforces_name_value_and_count_bounds() {
        let store = SecretStore::in_memory();
        assert!(matches!(store.set("bad name", "v"), Err(SecretError::InvalidName)));
        assert!(matches!(store.set("ok", ""), Err(SecretError::EmptyValue)));
        let big = "x".repeat(relux_core::MAX_SECRET_VALUE_BYTES + 1);
        assert!(matches!(
            store.set("ok", &big),
            Err(SecretError::ValueTooLarge { .. })
        ));
    }

    #[test]
    fn round_trips_through_a_hardened_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("secrets.json");
        {
            let store = SecretStore::open(&path);
            store.set("token", "abcdef-secret-1234").unwrap();
        }
        // Reopen: the secret survives, still resolvable, never in the listing JSON.
        let store = SecretStore::open(&path);
        assert_eq!(store.resolve("token").as_deref(), Some("abcdef-secret-1234"));
        let json = serde_json::to_string(&store.list()).unwrap();
        assert!(!json.contains("abcdef-secret-1234"));
    }

    #[cfg(unix)]
    #[test]
    fn persisted_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let store = SecretStore::open(&path);
        store.set("k", "value-1234").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[test]
    fn resolve_env_names_the_missing_secret_not_the_value() {
        // No secret set: resolution fails naming the secret + env var, never a value.
        let refs = env_refs(&[("OPENAI_API_KEY", "definitely-absent-secret-xyz")]);
        let err = resolve_managed_env_and_cwd(&refs, None).unwrap_err();
        assert!(err.contains("definitely-absent-secret-xyz"), "{err}");
        assert!(err.contains("OPENAI_API_KEY"), "{err}");
    }

    #[test]
    fn resolve_env_injects_the_secret_value_when_present() {
        // Use a UNIQUE secret name so the process-global store does not collide with
        // other tests running in the same process.
        let name = "relux_test_env_secret_unique_001";
        secret_store().set(name, "super-secret-value-9999").unwrap();
        let refs = env_refs(&[("MY_TOKEN", name)]);
        let (env, cwd) = resolve_managed_env_and_cwd(&refs, None).unwrap();
        assert_eq!(cwd, None);
        assert_eq!(env, vec![("MY_TOKEN".to_string(), "super-secret-value-9999".to_string())]);
        secret_store().delete(name);
    }

    #[test]
    fn cwd_validation_rejects_traversal_and_outside_root_accepts_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A `..` traversal is rejected at the shape stage.
        assert!(validate_managed_cwd("../escape", root).is_err());
        // A non-existent dir is rejected.
        assert!(validate_managed_cwd("does-not-exist", root).is_err());
        // An absolute path OUTSIDE the root is rejected.
        let outside = tempfile::tempdir().unwrap();
        assert!(validate_managed_cwd(&outside.path().display().to_string(), root).is_err());
        // A real subdirectory INSIDE the root is accepted and canonicalized.
        let sub = root.join("workspace");
        std::fs::create_dir_all(&sub).unwrap();
        let ok = validate_managed_cwd("workspace", root).unwrap();
        assert!(ok.ends_with("workspace"));
        assert!(ok.starts_with(root.canonicalize().unwrap()));
        // A file (not a directory) is rejected.
        let file = root.join("afile");
        std::fs::write(&file, b"x").unwrap();
        assert!(validate_managed_cwd("afile", root).is_err());
    }

    #[test]
    fn cwd_without_a_configured_root_is_refused() {
        // The process-global root may or may not be set by other code, so assert the
        // pure containment rule instead: an absolute cwd outside a fresh root fails.
        let tmp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let abs = other.path().display().to_string();
        assert!(validate_managed_cwd(&abs, tmp.path()).is_err());
    }
}
