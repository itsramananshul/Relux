//! Local operator login — first-run admin setup, Argon2id password storage, and
//! HTTP-only session cookies for the standalone `relux-kernel serve` dashboard.
//!
//! Until now the standalone Relux API bound loopback and was unauthenticated by
//! design (`docs/RELUX_MASTER_PLAN.md` §22). That is fine for a single trusted
//! operator on their own machine, but the dashboard token-paste flow was awkward
//! and any other local process/user could probe the open port. This module adds
//! a simple username/password login on top, mirroring the proven legacy bridge
//! design (`crates/relix-web-bridge/src/dashboard_auth.rs`) but self-contained in
//! the kernel:
//!
//! - First run: an admin account is created (username + Argon2id PHC hash),
//!   persisted next to the local DB as `dashboard-admin.json`.
//! - Login verifies the password and mints an in-memory session.
//! - A successful setup/login sets an **HTTP-only** `relux_session` cookie; the
//!   serve auth middleware admits any request carrying a valid session cookie, so
//!   every dashboard `fetch` authenticates automatically — no token paste.
//!
//! Sessions live in memory (a single-process kernel); they reset on restart,
//! which simply re-prompts the operator to log in. The admin credential is
//! durable on disk. There is no network/unauthenticated reset path — recovery is
//! the local `relux-kernel reset-admin` CLI ([`reset_admin_credential`]).
//!
//! **Honest scope:** this is a local-first single-admin console guard, not an
//! internet auth system. The cookie omits `Secure` because the operator console
//! runs over loopback `http://`; a reverse proxy terminating TLS can re-add it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::header;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Name of the HTTP-only session cookie the dashboard rides on.
pub const SESSION_COOKIE: &str = "relux_session";

/// Idle timeout in seconds. A logged-in operator stays authenticated for this
/// long **of inactivity** before having to sign in again (12 hours — the same
/// window the fixed-lifetime v1 used). Each authenticated control-plane request
/// slides the session's idle deadline forward by this much (see
/// [`DashboardAuth::refresh_session`]), so an actively-used console never expires
/// out from under the operator.
pub const SESSION_TTL_SECS: i64 = 12 * 60 * 60;

/// Absolute maximum session lifetime in seconds, measured from when the session
/// was first minted (7 days). The sliding idle window can renew a session
/// repeatedly, but **never past this cap** — after a week a session is forced to
/// re-authenticate regardless of activity. This bounds how long a single stolen
/// or forgotten cookie stays useful even under continuous traffic.
pub const SESSION_ABSOLUTE_MAX_SECS: i64 = 7 * 24 * 60 * 60;

/// Minimum password length accepted at setup. Deliberately modest — this guards
/// a loopback operator console, not an internet service.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Why an authenticated password change was refused. The HTTP layer maps each
/// variant to an honest status code; **no variant ever carries the plaintext
/// password or the stored hash**, so a logged/serialized error can never leak a
/// secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangePasswordError {
    /// No admin account exists yet (first-run setup has not happened).
    NoAdmin,
    /// The supplied current password did not match the stored credential.
    WrongCurrent,
    /// The proposed new password is shorter than [`MIN_PASSWORD_LEN`].
    TooShort,
    /// Persisting the new credential failed (I/O or encode). The message is a
    /// safe, secret-free description of the storage failure.
    Storage(String),
}

impl std::fmt::Display for ChangePasswordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdmin => write!(f, "no admin configured — run setup first"),
            Self::WrongCurrent => write!(f, "current password is incorrect"),
            Self::TooShort => {
                write!(f, "new password too short (min {MIN_PASSWORD_LEN} chars)")
            }
            Self::Storage(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ChangePasswordError {}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Admin credential (durable) ──────────────────────────────────

/// The single dashboard admin account, persisted as JSON next to the local DB.
/// `hash` is an Argon2id PHC string — never the plaintext password.
#[derive(Clone, Serialize, Deserialize)]
struct AdminRecord {
    username: String,
    hash: String,
    #[serde(default)]
    created_at: i64,
}

/// File-backed admin credential store with an in-memory cache.
#[derive(Clone)]
struct AdminStore {
    path: Arc<PathBuf>,
    /// Cached record; `None` until first-run setup completes.
    cached: Arc<RwLock<Option<AdminRecord>>>,
}

impl AdminStore {
    fn load(path: &Path) -> Self {
        let cached = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<AdminRecord>(&b).ok());
        Self {
            path: Arc::new(path.to_path_buf()),
            cached: Arc::new(RwLock::new(cached)),
        }
    }

    fn exists(&self) -> bool {
        self.cached.read().map(|c| c.is_some()).unwrap_or(false)
    }

    fn username(&self) -> Option<String> {
        self.cached
            .read()
            .ok()
            .and_then(|c| c.as_ref().map(|r| r.username.clone()))
    }

    /// Create the admin account (first run only). Hashes `password` with
    /// Argon2id and persists the record at restrictive permissions.
    fn create(&self, username: &str, password: &str) -> Result<(), String> {
        let rec = write_admin_record(&self.path, username, password)?;
        if let Ok(mut c) = self.cached.write() {
            *c = Some(rec);
        }
        Ok(())
    }

    /// Change the stored password for the (single) admin: verify `current`
    /// against the stored hash, then rewrite the record with a freshly
    /// Argon2id-hashed `new` password via the SAME atomic write as setup/reset.
    /// The username is preserved. Neither password is ever logged or returned.
    fn change_password(&self, current: &str, new: &str) -> Result<(), ChangePasswordError> {
        let rec = self
            .cached
            .read()
            .ok()
            .and_then(|c| c.clone())
            .ok_or(ChangePasswordError::NoAdmin)?;
        // Verify the current password before doing anything else. A corrupt stored
        // hash and a wrong password both read as WrongCurrent (no detail leaks).
        let parsed =
            PasswordHash::new(&rec.hash).map_err(|_| ChangePasswordError::WrongCurrent)?;
        Argon2::default()
            .verify_password(current.as_bytes(), &parsed)
            .map_err(|_| ChangePasswordError::WrongCurrent)?;
        // Only after identity is proven do we validate the proposed new password.
        if new.len() < MIN_PASSWORD_LEN {
            return Err(ChangePasswordError::TooShort);
        }
        let updated = write_admin_record(&self.path, &rec.username, new)
            .map_err(ChangePasswordError::Storage)?;
        if let Ok(mut c) = self.cached.write() {
            *c = Some(updated);
        }
        Ok(())
    }

    /// Verify a login. Returns the canonical username on success. A constant-ish
    /// Argon2 verify runs only after the username matches; a wrong password and
    /// an unknown username both return `None`.
    fn verify(&self, username: &str, password: &str) -> Option<String> {
        let rec = self.cached.read().ok()?.clone()?;
        if rec.username != username {
            return None;
        }
        let parsed = PasswordHash::new(&rec.hash).ok()?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .ok()
            .map(|_| rec.username)
    }
}

/// Where the dashboard admin credential lives, given the local DB path:
/// `dashboard-admin.json` in the SAME directory. Used by both the running serve
/// process and the `reset-admin` CLI so they always agree on the file.
pub fn admin_path_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join("dashboard-admin.json"))
        .unwrap_or_else(|| PathBuf::from("dashboard-admin.json"))
}

/// Hash `password` (Argon2id) + atomically write the admin record at `path`,
/// restricting it to the current user. Shared by first-run setup and the reset
/// path so the on-disk format is identical.
fn write_admin_record(path: &Path, username: &str, password: &str) -> Result<AdminRecord, String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| format!("hash: {e}"))?
        .to_string();
    let rec = AdminRecord {
        username: username.to_string(),
        hash,
        created_at: now_secs(),
    };
    let body = serde_json::to_vec_pretty(&rec).map_err(|e| format!("encode: {e}"))?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &body).map_err(|e| format!("write: {e}"))?;
    let _ = restrict_to_current_user(&tmp);
    std::fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    let _ = restrict_to_current_user(path);
    Ok(rec)
}

/// The current admin username at `admin_path`, or `None` if no admin exists yet.
/// Never returns the password hash — callers that reset reuse the existing
/// username without ever seeing the secret.
pub fn read_admin_username(admin_path: &Path) -> Option<String> {
    let bytes = std::fs::read(admin_path).ok()?;
    serde_json::from_slice::<AdminRecord>(&bytes)
        .ok()
        .map(|r| r.username)
}

/// **Local operator recovery only.** Overwrite the dashboard admin credential at
/// `admin_path` with a new username + a freshly Argon2id-hashed password, using
/// the SAME storage format as first-run setup.
///
/// There is deliberately NO network path to this — it is a CLI / filesystem
/// operation an operator runs locally (it requires write access to the admin
/// file). It does NOT print or read the existing password/hash, does NOT weaken
/// session auth, and does NOT touch any other state. Existing in-memory sessions
/// are not invalidated here; restart `relux-kernel serve` to drop them (a
/// restart also reloads this new credential).
pub fn reset_admin_credential(
    admin_path: &Path,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("username required".to_string());
    }
    if password.len() < MIN_PASSWORD_LEN {
        return Err(format!("password too short (min {MIN_PASSWORD_LEN} chars)"));
    }
    write_admin_record(admin_path, username, password).map(|_| ())
}

// ── Sessions (in-memory) ────────────────────────────────────────

/// Safe, secret-free snapshot of a live session for the dashboard Account
/// control. Carries only the operator name and the two deadlines — **never** the
/// session id, the cookie value, the admin hash, or any other secret — so it can
/// be serialized to the page without leaking anything that admits a request.
///
/// Read by [`DashboardAuth::session_meta`], which (like [`SessionStore::validate`])
/// is **non-mutating on the idle window**: fetching this snapshot does NOT slide
/// the session forward, so the Account modal can poll it without keeping an
/// otherwise-idle console alive. The deadlines are therefore the *current,
/// pre-refresh* values — what the operator's cookie reflects right now, not a
/// window bumped by the act of reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    /// The signed-in operator's username.
    pub username: String,
    /// Idle deadline (unix seconds): the session expires at this instant unless a
    /// real authenticated control-plane request slides it forward first.
    pub idle_expires_at: i64,
    /// Absolute ceiling (unix seconds): the session is forced to re-authenticate
    /// at this instant regardless of activity. Always `>= idle_expires_at`.
    pub absolute_expires_at: i64,
}

struct Session {
    username: String,
    /// Idle deadline: the session is valid until this instant unless an
    /// authenticated request slides it forward. Always `<= absolute_deadline`.
    expires_at: i64,
    /// Hard ceiling set at creation (`created_at + SESSION_ABSOLUTE_MAX_SECS`).
    /// `expires_at` is never slid past this, so the session cannot outlive it.
    absolute_deadline: i64,
}

/// In-memory session table, keyed by a random opaque session id.
#[derive(Clone)]
struct SessionStore {
    inner: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn create(&self, username: &str) -> String {
        let mut buf = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        let sid = hex::encode(buf);
        let now = now_secs();
        if let Ok(mut m) = self.inner.write() {
            m.insert(
                sid.clone(),
                Session {
                    username: username.to_string(),
                    expires_at: now + SESSION_TTL_SECS,
                    absolute_deadline: now + SESSION_ABSOLUTE_MAX_SECS,
                },
            );
        }
        sid
    }

    /// Whether a session is still live: within BOTH the idle window
    /// (`expires_at`) and the absolute lifetime (`absolute_deadline`). The
    /// invariant `expires_at <= absolute_deadline` makes the second check
    /// redundant in practice, but it is kept explicit so a hand-constructed or
    /// future-edited session can never slip past the hard ceiling.
    fn is_live(s: &Session, now: i64) -> bool {
        s.expires_at > now && s.absolute_deadline > now
    }

    /// Return the session's username if it exists and has not expired. Prunes
    /// the entry when expired. **Non-mutating on the deadline** — used by status
    /// reads (`/v1/auth/status`, `/v1/auth/me`) so polling never slides the idle
    /// window; only real control-plane activity refreshes it (see [`Self::refresh`]).
    fn validate(&self, sid: &str) -> Option<String> {
        let now = now_secs();
        if let Ok(m) = self.inner.read() {
            match m.get(sid) {
                Some(s) if Self::is_live(s, now) => return Some(s.username.clone()),
                Some(_) => {} // expired → fall through to prune
                None => return None,
            }
        }
        if let Ok(mut m) = self.inner.write() {
            m.remove(sid);
        }
        None
    }

    /// Return a non-secret [`SessionMeta`] for a live session, or `None` (pruning
    /// the entry) when it is missing or past either deadline. **Non-mutating on the
    /// idle window** — like [`Self::validate`], reading metadata never slides the
    /// session, so the Account modal can poll it without keeping an idle console
    /// alive. The deadlines returned are the current pre-refresh values.
    fn meta(&self, sid: &str) -> Option<SessionMeta> {
        let now = now_secs();
        if let Ok(m) = self.inner.read() {
            match m.get(sid) {
                Some(s) if Self::is_live(s, now) => {
                    return Some(SessionMeta {
                        username: s.username.clone(),
                        idle_expires_at: s.expires_at,
                        absolute_expires_at: s.absolute_deadline,
                    });
                }
                Some(_) => {} // expired → fall through to prune
                None => return None,
            }
        }
        if let Ok(mut m) = self.inner.write() {
            m.remove(sid);
        }
        None
    }

    /// Slide a live session's idle deadline forward and report the cookie
    /// `Max-Age` (seconds) the caller should re-emit. The new idle deadline is
    /// `now + SESSION_TTL_SECS`, **capped at `absolute_deadline`** so the session
    /// can never be renewed past its hard ceiling. Returns `None` (and prunes the
    /// entry) when the session is missing or already past either deadline — in
    /// that case no cookie should be sent. The session id itself is unchanged
    /// (the window slides; the opaque id is not rotated).
    fn refresh(&self, sid: &str) -> Option<i64> {
        let now = now_secs();
        if let Ok(mut m) = self.inner.write() {
            match m.get_mut(sid) {
                Some(s) if Self::is_live(s, now) => {
                    let new_exp = (now + SESSION_TTL_SECS).min(s.absolute_deadline);
                    s.expires_at = new_exp;
                    return Some(new_exp - now);
                }
                Some(_) => {
                    m.remove(sid);
                }
                None => {}
            }
        }
        None
    }

    fn remove(&self, sid: &str) {
        if let Ok(mut m) = self.inner.write() {
            m.remove(sid);
        }
    }

    /// Drop every session EXCEPT `keep` (the caller's own session). Used after a
    /// password change so any OTHER live session is invalidated immediately while
    /// the operator who just changed their password stays signed in.
    fn retain_only(&self, keep: &str) {
        if let Ok(mut m) = self.inner.write() {
            m.retain(|sid, _| sid == keep);
        }
    }

    /// Test seam: insert a session with explicit deadlines so the sliding/absolute
    /// behavior can be exercised without sleeping for real-time hours.
    #[cfg(test)]
    fn insert_raw(&self, sid: &str, username: &str, expires_at: i64, absolute_deadline: i64) {
        if let Ok(mut m) = self.inner.write() {
            m.insert(
                sid.to_string(),
                Session {
                    username: username.to_string(),
                    expires_at,
                    absolute_deadline,
                },
            );
        }
    }

    /// Test seam: read back a session's `(expires_at, absolute_deadline)`.
    #[cfg(test)]
    fn peek(&self, sid: &str) -> Option<(i64, i64)> {
        self.inner
            .read()
            .ok()?
            .get(sid)
            .map(|s| (s.expires_at, s.absolute_deadline))
    }
}

// ── Combined handle stored on AppState ──────────────────────────

/// Dashboard auth state: the durable admin credential + the in-memory session
/// table. Cloned cheaply (Arc inside).
#[derive(Clone)]
pub struct DashboardAuth {
    admin: AdminStore,
    sessions: SessionStore,
}

impl DashboardAuth {
    /// Build from the local DB path: the admin record lives in the same
    /// directory (`dashboard-admin.json`) so it sits with the operator's other
    /// Relux state.
    pub fn from_db_path(db_path: &Path) -> Self {
        let admin_path = admin_path_for_db(db_path);
        Self::from_admin_path(&admin_path)
    }

    /// Build directly from an explicit admin-file path (used by tests and any
    /// caller that resolves the file itself).
    pub fn from_admin_path(admin_path: &Path) -> Self {
        Self {
            admin: AdminStore::load(admin_path),
            sessions: SessionStore::new(),
        }
    }

    /// Whether the first-run admin account has been configured.
    pub fn admin_exists(&self) -> bool {
        self.admin.exists()
    }

    /// The configured admin username, if any.
    pub fn admin_username(&self) -> Option<String> {
        self.admin.username()
    }

    /// Create the first-run admin account. Errors if one already exists, the
    /// username is empty, or the password is too short.
    pub fn create_admin(&self, username: &str, password: &str) -> Result<(), String> {
        if self.admin.exists() {
            return Err("admin already configured".to_string());
        }
        let username = username.trim();
        if username.is_empty() {
            return Err("username required".to_string());
        }
        if password.len() < MIN_PASSWORD_LEN {
            return Err(format!("password too short (min {MIN_PASSWORD_LEN} chars)"));
        }
        self.admin.create(username, password)
    }

    /// Verify a login. Returns the canonical username on success.
    pub fn verify_login(&self, username: &str, password: &str) -> Option<String> {
        self.admin.verify(username.trim(), password)
    }

    /// Change the admin password for an already-authenticated operator.
    ///
    /// `current_sid` is the caller's OWN session id. The flow:
    /// 1. Verify `current` against the stored Argon2id hash (wrong → error).
    /// 2. Validate the new password length.
    /// 3. Atomically rewrite the on-disk credential with a fresh Argon2id hash.
    /// 4. Invalidate every OTHER live session, preserving only `current_sid`.
    ///
    /// Step 4 means a password change boots any other browser/device that still
    /// holds a session, but does NOT log the operator out of the tab they just
    /// used. Neither password is ever logged or returned. Recovery when the
    /// current password is unknown stays the local `reset-admin` CLI
    /// ([`reset_admin_credential`]).
    pub fn change_password(
        &self,
        current_sid: &str,
        current: &str,
        new: &str,
    ) -> Result<(), ChangePasswordError> {
        self.admin.change_password(current, new)?;
        self.sessions.retain_only(current_sid);
        Ok(())
    }

    /// Mint a new session for `username` and return its opaque id.
    pub fn create_session(&self, username: &str) -> String {
        self.sessions.create(username)
    }

    /// Validate a raw session-cookie value **without** sliding its idle window.
    /// Used by the serve auth middleware to decide admission and by the public
    /// status endpoints to report login state. Returns the username.
    pub fn validate_session(&self, sid: &str) -> Option<String> {
        self.sessions.validate(sid)
    }

    /// A non-secret [`SessionMeta`] snapshot for a raw session-cookie value, or
    /// `None` when the session is missing/expired. Like [`Self::validate_session`]
    /// this does **not** slide the idle window, so the dashboard Account control
    /// can poll it for an expiry/idle readout without keeping an idle console
    /// alive. The returned deadlines are pre-refresh (the current cookie state).
    pub fn session_meta(&self, sid: &str) -> Option<SessionMeta> {
        self.sessions.meta(sid)
    }

    /// Slide a live session forward by the idle timeout and return the cookie
    /// `Max-Age` (seconds) to re-emit, capped at the session's absolute
    /// lifetime. Returns `None` when the session is missing/expired, in which
    /// case the caller must NOT set a refreshed cookie. The serve auth middleware
    /// calls this on a successful protected response so an actively-used console
    /// keeps a rolling session up to [`SESSION_ABSOLUTE_MAX_SECS`].
    pub fn refresh_session(&self, sid: &str) -> Option<i64> {
        self.sessions.refresh(sid)
    }

    /// Drop a session (logout).
    pub fn remove_session(&self, sid: &str) {
        self.sessions.remove(sid)
    }
}

// ── Cookie helpers ──────────────────────────────────────────────

/// Pull the `relux_session` value out of a request's `Cookie` header.
pub fn session_cookie_from_headers(headers: &header::HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some(v) = pair.strip_prefix(&format!("{SESSION_COOKIE}=")) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Build the `Set-Cookie` value that establishes a logged-in session.
///
/// `HttpOnly` so page JS cannot read it; `SameSite=Lax` so a cross-site form
/// POST cannot ride it while a normal top-level navigation still carries it;
/// `Path=/` for the whole app; `Max-Age` matching the session TTL. No `Secure`
/// because the operator console runs over loopback `http://` — a reverse proxy
/// terminating TLS can re-add it.
///
/// Used at login/setup to establish a fresh full-length idle window. The sliding
/// refresh on subsequent requests uses [`set_session_cookie_with_max_age`] so the
/// browser's cookie expiry tracks the server session as the window slides (and
/// shrinks near the absolute deadline).
pub fn set_session_cookie(sid: &str) -> String {
    set_session_cookie_with_max_age(sid, SESSION_TTL_SECS)
}

/// Same cookie as [`set_session_cookie`] but with an explicit `Max-Age`. The
/// serve auth middleware emits this on a successful protected request, passing
/// the remaining seconds reported by [`DashboardAuth::refresh_session`], so the
/// browser keeps the cookie exactly as long as the server keeps the session.
/// A non-positive `max_age` is clamped to `0`, which expires the cookie
/// immediately rather than emitting a negative attribute.
pub fn set_session_cookie_with_max_age(sid: &str, max_age: i64) -> String {
    let max_age = max_age.max(0);
    format!("{SESSION_COOKIE}={sid}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}")
}

/// Build the `Set-Cookie` value that clears the session on logout.
pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

// ── Best-effort OS file hardening ───────────────────────────────

/// Restrict `path` to the current user. On POSIX this is `chmod 0600`; on
/// Windows it strips inheritance and grants only the current user via `icacls`.
/// Best-effort: a failure is returned (callers ignore it — a writable secret
/// file is still better than none) and never blocks setup.
fn restrict_to_current_user(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .map_err(|e| format!("chmod {}: {e}", path.display()))
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_default();
        if user.is_empty() {
            return Err("USERNAME not set; cannot scope ACL".to_string());
        }
        let status = std::process::Command::new("icacls")
            .arg(path)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{user}:(F)"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("spawn icacls: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("icacls exited with {status}"))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> (DashboardAuth, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("dashboard-admin.json");
        (DashboardAuth::from_admin_path(&admin), tmp)
    }

    #[test]
    fn admin_setup_then_verify_roundtrips() {
        let (auth, _tmp) = auth();
        assert!(!auth.admin_exists());
        auth.create_admin("ops", "hunter2pass").unwrap();
        assert!(auth.admin_exists());
        assert_eq!(auth.admin_username().as_deref(), Some("ops"));
        assert_eq!(auth.verify_login("ops", "hunter2pass").as_deref(), Some("ops"));
        assert!(auth.verify_login("ops", "wrong").is_none());
        assert!(auth.verify_login("other", "hunter2pass").is_none());
    }

    #[test]
    fn setup_is_first_run_only_and_validates() {
        let (auth, _tmp) = auth();
        // Empty username + short password are refused.
        assert!(auth.create_admin("  ", "longenough").is_err());
        assert!(auth.create_admin("ops", "short").is_err());
        // A valid setup succeeds once...
        auth.create_admin("ops", "validpassword").unwrap();
        // ...and a second setup is refused (use login / reset instead).
        assert!(auth.create_admin("ops", "anotherpassword").is_err());
    }

    #[test]
    fn admin_record_persists_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("dashboard-admin.json");
        let a1 = DashboardAuth::from_admin_path(&admin);
        a1.create_admin("ops", "hunter2pass").unwrap();
        // A fresh handle on the same path reads the persisted admin.
        let a2 = DashboardAuth::from_admin_path(&admin);
        assert!(a2.admin_exists());
        assert_eq!(a2.verify_login("ops", "hunter2pass").as_deref(), Some("ops"));
    }

    #[test]
    fn session_create_validate_remove() {
        let (auth, _tmp) = auth();
        let sid = auth.create_session("ops");
        assert_eq!(auth.validate_session(&sid).as_deref(), Some("ops"));
        auth.remove_session(&sid);
        assert!(auth.validate_session(&sid).is_none());
        // Unknown session id is rejected.
        assert!(auth.validate_session("deadbeef").is_none());
    }

    #[test]
    fn refresh_slides_the_idle_deadline_forward() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        // A session that is live but about to time out (10s of idle left), with a
        // far-off absolute ceiling.
        auth.sessions
            .insert_raw("sid", "ops", now + 10, now + SESSION_ABSOLUTE_MAX_SECS);
        let max_age = auth.refresh_session("sid").expect("a live session refreshes");
        // The returned Max-Age is the full idle window (the cap is far away).
        assert!(
            (max_age - SESSION_TTL_SECS).abs() <= 2,
            "expected ~{SESSION_TTL_SECS}, got {max_age}"
        );
        // The stored idle deadline jumped forward to ~now + idle.
        let (expires_at, _abs) = auth.sessions.peek("sid").unwrap();
        assert!(
            expires_at >= now + SESSION_TTL_SECS - 2,
            "idle deadline must slide forward; got {expires_at}"
        );
        // Still a valid session after the slide.
        assert_eq!(auth.validate_session("sid").as_deref(), Some("ops"));
    }

    #[test]
    fn refresh_is_capped_by_the_absolute_deadline() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        // Live session, but the absolute ceiling is only 100s away — closer than a
        // full idle window. The slide must clamp to the ceiling, not overshoot it.
        let abs = now + 100;
        auth.sessions.insert_raw("sid", "ops", now + 10, abs);
        let max_age = auth.refresh_session("sid").expect("still live");
        assert!(
            max_age <= 100 && max_age > 90,
            "refresh must clamp Max-Age to the absolute ceiling; got {max_age}"
        );
        let (expires_at, _abs) = auth.sessions.peek("sid").unwrap();
        assert!(
            expires_at <= abs,
            "idle deadline must never exceed the absolute ceiling ({expires_at} > {abs})"
        );
    }

    #[test]
    fn refresh_rejects_an_idle_timed_out_session_and_prunes_it() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        // Idle deadline already in the past (absolute ceiling still ahead).
        auth.sessions
            .insert_raw("sid", "ops", now - 1, now + SESSION_ABSOLUTE_MAX_SECS);
        assert!(
            auth.refresh_session("sid").is_none(),
            "an idle-expired session must not refresh"
        );
        // The dead entry was pruned and no longer validates.
        assert!(auth.sessions.peek("sid").is_none());
        assert!(auth.validate_session("sid").is_none());
    }

    #[test]
    fn refresh_rejects_a_session_past_its_absolute_deadline() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        // Idle window would look open (1000s left) but the absolute ceiling has
        // already passed — the hard cap wins and the session is dead.
        auth.sessions.insert_raw("sid", "ops", now + 1000, now - 1);
        assert!(
            auth.refresh_session("sid").is_none(),
            "the absolute ceiling must force expiry even with idle time left"
        );
        assert!(auth.validate_session("sid").is_none());
    }

    #[test]
    fn session_meta_reports_deadlines_without_sliding() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        let idle = now + 1234;
        let abs = now + SESSION_ABSOLUTE_MAX_SECS;
        auth.sessions.insert_raw("sid", "ops", idle, abs);
        let meta = auth.session_meta("sid").expect("a live session has metadata");
        assert_eq!(meta.username, "ops");
        assert_eq!(meta.idle_expires_at, idle);
        assert_eq!(meta.absolute_expires_at, abs);
        // Reading metadata must NOT slide the idle window (so the Account modal can
        // poll it without keeping an idle console alive).
        let (after, after_abs) = auth.sessions.peek("sid").unwrap();
        assert_eq!(after, idle, "session_meta must not move the idle deadline");
        assert_eq!(after_abs, abs, "session_meta must not move the absolute ceiling");
    }

    #[test]
    fn session_meta_rejects_and_prunes_an_expired_session() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        // Idle deadline already past (absolute ceiling still ahead).
        auth.sessions
            .insert_raw("sid", "ops", now - 1, now + SESSION_ABSOLUTE_MAX_SECS);
        assert!(
            auth.session_meta("sid").is_none(),
            "an idle-expired session exposes no metadata"
        );
        // The dead entry was pruned, exactly like validate.
        assert!(auth.sessions.peek("sid").is_none());
        // An unknown session id is simply None (no panic, no entry created).
        assert!(auth.session_meta("deadbeef").is_none());
    }

    #[test]
    fn validate_does_not_slide_the_idle_window() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        let before = now + 30;
        auth.sessions
            .insert_raw("sid", "ops", before, now + SESSION_ABSOLUTE_MAX_SECS);
        // A plain validate (status poll) admits the session but leaves the idle
        // deadline untouched — only refresh slides it.
        assert_eq!(auth.validate_session("sid").as_deref(), Some("ops"));
        let (after, _abs) = auth.sessions.peek("sid").unwrap();
        assert_eq!(after, before, "validate must not move the idle deadline");
    }

    #[test]
    fn refreshed_session_never_outlives_the_absolute_cap() {
        let (auth, _tmp) = auth();
        let now = now_secs();
        // Real session minted now; its ceiling is now + absolute max.
        let sid = auth.create_session("ops");
        let (_e, abs) = auth.sessions.peek(&sid).unwrap();
        assert!(
            (abs - (now + SESSION_ABSOLUTE_MAX_SECS)).abs() <= 2,
            "absolute deadline is set at creation"
        );
        // Repeated refreshes keep sliding the idle window but the ceiling is fixed.
        for _ in 0..5 {
            auth.refresh_session(&sid).expect("live");
            let (expires_at, abs2) = auth.sessions.peek(&sid).unwrap();
            assert_eq!(abs2, abs, "the absolute ceiling is immutable across refreshes");
            assert!(expires_at <= abs2, "idle deadline stays under the ceiling");
        }
    }

    #[test]
    fn stored_hash_is_argon2id_phc_not_plaintext() {
        let (auth, tmp) = auth();
        auth.create_admin("ops", "hunter2pass").unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("dashboard-admin.json")).unwrap();
        assert!(raw.contains("$argon2id$"), "got: {raw}");
        assert!(
            !raw.contains("hunter2pass"),
            "password must never be stored in plaintext"
        );
    }

    #[test]
    fn admin_path_is_next_to_the_db() {
        let p = admin_path_for_db(Path::new("/x/y/local.db"));
        assert!(p.ends_with("dashboard-admin.json"));
        assert_eq!(p.parent().unwrap(), Path::new("/x/y"));
        // A bare filename (no parent) still resolves to a sane relative path.
        let p2 = admin_path_for_db(Path::new("local.db"));
        assert!(p2.ends_with("dashboard-admin.json"));
    }

    #[test]
    fn reset_changes_password_old_fails_new_works() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("dashboard-admin.json");
        let a1 = DashboardAuth::from_admin_path(&admin);
        a1.create_admin("ops", "oldpassword").unwrap();
        assert_eq!(a1.verify_login("ops", "oldpassword").as_deref(), Some("ops"));
        // Reset keeps the username (read from disk) but sets a new password.
        let user = read_admin_username(&admin).unwrap();
        assert_eq!(user, "ops");
        reset_admin_credential(&admin, &user, "newpassword1").unwrap();
        // A FRESH handle (simulating a serve restart) honors ONLY the new
        // password — the old one is gone.
        let a2 = DashboardAuth::from_admin_path(&admin);
        assert_eq!(a2.verify_login("ops", "newpassword1").as_deref(), Some("ops"));
        assert!(
            a2.verify_login("ops", "oldpassword").is_none(),
            "old password must stop working after reset"
        );
    }

    #[test]
    fn reset_creates_when_absent_and_validates() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("dashboard-admin.json");
        // No admin yet → reset CREATES it with the given username.
        assert!(read_admin_username(&admin).is_none());
        reset_admin_credential(&admin, "newadmin", "secretpass1").unwrap();
        assert_eq!(read_admin_username(&admin).as_deref(), Some("newadmin"));
        // Empty username / short password are refused, never stored as plaintext.
        assert!(reset_admin_credential(&admin, "  ", "longenough").is_err());
        assert!(reset_admin_credential(&admin, "ops", "short").is_err());
        let raw = std::fs::read_to_string(&admin).unwrap();
        assert!(raw.contains("$argon2id$"), "got: {raw}");
        assert!(!raw.contains("secretpass1"));
    }

    #[test]
    fn change_password_wrong_current_is_rejected_and_old_still_works() {
        let (auth, _tmp) = auth();
        auth.create_admin("ops", "oldpassword").unwrap();
        let sid = auth.create_session("ops");
        // A wrong current password is refused with WrongCurrent...
        assert_eq!(
            auth.change_password(&sid, "not-the-current", "brandnewpass"),
            Err(ChangePasswordError::WrongCurrent)
        );
        // ...and nothing changed: the original password still verifies.
        assert_eq!(auth.verify_login("ops", "oldpassword").as_deref(), Some("ops"));
        assert!(auth.verify_login("ops", "brandnewpass").is_none());
    }

    #[test]
    fn change_password_too_short_new_is_rejected() {
        let (auth, _tmp) = auth();
        auth.create_admin("ops", "oldpassword").unwrap();
        let sid = auth.create_session("ops");
        // Correct current password, but the new one is below MIN_PASSWORD_LEN.
        assert_eq!(
            auth.change_password(&sid, "oldpassword", "short"),
            Err(ChangePasswordError::TooShort)
        );
        // The old password is untouched.
        assert_eq!(auth.verify_login("ops", "oldpassword").as_deref(), Some("ops"));
    }

    #[test]
    fn change_password_success_swaps_hash_and_old_password_stops_working() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("dashboard-admin.json");
        let auth = DashboardAuth::from_admin_path(&admin);
        auth.create_admin("ops", "oldpassword").unwrap();
        let before = std::fs::read_to_string(&admin).unwrap();
        let sid = auth.create_session("ops");
        auth.change_password(&sid, "oldpassword", "newpassword1").unwrap();
        // The new password works; the old one no longer does.
        assert_eq!(auth.verify_login("ops", "newpassword1").as_deref(), Some("ops"));
        assert!(auth.verify_login("ops", "oldpassword").is_none());
        // The stored hash actually changed and remains an Argon2id PHC string with
        // neither plaintext written to disk.
        let after = std::fs::read_to_string(&admin).unwrap();
        assert_ne!(before, after, "the stored hash must be rewritten");
        assert!(after.contains("$argon2id$"), "got: {after}");
        assert!(!after.contains("newpassword1"));
        assert!(!after.contains("oldpassword"));
        // A FRESH handle (simulating a serve restart) honors only the new password.
        let reopened = DashboardAuth::from_admin_path(&admin);
        assert_eq!(reopened.verify_login("ops", "newpassword1").as_deref(), Some("ops"));
        assert!(reopened.verify_login("ops", "oldpassword").is_none());
    }

    #[test]
    fn change_password_invalidates_other_sessions_but_keeps_current() {
        let (auth, _tmp) = auth();
        auth.create_admin("ops", "oldpassword").unwrap();
        let current = auth.create_session("ops");
        let other = auth.create_session("ops");
        // Both sessions are valid before the change.
        assert!(auth.validate_session(&current).is_some());
        assert!(auth.validate_session(&other).is_some());
        auth.change_password(&current, "oldpassword", "newpassword1").unwrap();
        // The caller's own session survives; every other session is dropped.
        assert_eq!(auth.validate_session(&current).as_deref(), Some("ops"));
        assert!(
            auth.validate_session(&other).is_none(),
            "other live sessions must be invalidated by a password change"
        );
    }

    #[test]
    fn change_password_no_admin_is_rejected() {
        let (auth, _tmp) = auth();
        // No setup yet → no credential to change.
        assert_eq!(
            auth.change_password("anything", "x", "newpassword1"),
            Err(ChangePasswordError::NoAdmin)
        );
    }

    #[test]
    fn cookie_value_parses_from_header() {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            header::HeaderValue::from_static("foo=bar; relux_session=abc123; baz=1"),
        );
        assert_eq!(session_cookie_from_headers(&headers).as_deref(), Some("abc123"));
        // No cookie header → None (an unauthenticated caller is rejected).
        let empty = header::HeaderMap::new();
        assert!(session_cookie_from_headers(&empty).is_none());
    }

    #[test]
    fn set_cookie_is_httponly_lax_and_clear_expires_it() {
        let set = set_session_cookie("abc123");
        assert!(set.contains("relux_session=abc123"));
        assert!(set.contains("HttpOnly"));
        assert!(set.contains("SameSite=Lax"));
        assert!(set.contains("Path=/"));
        // No Secure attribute (loopback http) — documented honestly.
        assert!(!set.contains("Secure"));
        let clear = clear_session_cookie();
        assert!(clear.contains("Max-Age=0"));
    }

    #[test]
    fn set_cookie_with_max_age_carries_that_window_and_clamps_negatives() {
        // An explicit positive Max-Age is echoed verbatim (used by the sliding
        // refresh to track the remaining server-side lifetime).
        let c = set_session_cookie_with_max_age("abc123", 3600);
        assert!(c.contains("relux_session=abc123"));
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("Path=/"));
        assert!(c.contains("Max-Age=3600"), "got: {c}");
        assert!(!c.contains("Secure"));
        // The login/setup helper still emits the full idle window.
        assert!(set_session_cookie("abc123").contains(&format!("Max-Age={SESSION_TTL_SECS}")));
        // A non-positive Max-Age is clamped to 0 (never a negative attribute).
        let neg = set_session_cookie_with_max_age("abc123", -5);
        assert!(neg.contains("Max-Age=0"), "got: {neg}");
    }
}
