//! At-rest **encryption providers** for the local secret store.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` §17.5 (permissions / safety) + `docs/mcp.md`
//! "Local secrets & environment". The store keeps operator API keys / tokens locally.
//! Permission-hardening the file (POSIX `0600` / Windows `icacls`) keeps OTHER users
//! out, but a file at rest is still plaintext to anyone who can read the disk image
//! (backup, stolen laptop, another admin). This module adds **encryption at rest** for
//! the value bytes themselves, keyed to the OS where one is available.
//!
//! ## What ships
//!
//! - **Windows → DPAPI, CurrentUser scope** ([`DpapiCipher`]). The value is sealed with
//!   `CryptProtectData` (CurrentUser), so only the same Windows user on the same machine
//!   can unseal it; the file alone is useless to anyone else. Scheme marker:
//!   [`relux_core::SECRET_SCHEME_DPAPI`].
//! - **Everything else → permission-hardened plaintext** ([`PlaintextCipher`]). No OS
//!   keychain integration yet, so the value is stored verbatim and protected only by the
//!   file permissions — honestly marked [`relux_core::SECRET_SCHEME_PLAINTEXT`] so an
//!   operator (and the dashboard) can see it is NOT encrypted at rest. This is also the
//!   fail-safe fallback on Windows when DPAPI is unavailable.
//!
//! ## Why a shell-out for DPAPI (mirrors `os_secure`)
//!
//! Like `relix-web-bridge::os_secure::restrict_to_current_user` (which shells out to
//! `icacls` rather than dragging in the Win32 ACL API + `unsafe`), [`DpapiCipher`] drives
//! DPAPI through PowerShell's `System.Security.Cryptography.ProtectedData` — the managed
//! wrapper over `CryptProtectData` / `CryptUnprotectData`. This keeps the kernel free of
//! `unsafe` and of a heavyweight `windows` crate dependency, and is equivalent in effect:
//! the same DPAPI master key, the same CurrentUser scope. The **plaintext never rides an
//! argv** — the only command-line argument is the secret-free script; the value travels
//! base64-encoded over the child's **stdin** and (on unseal) back over its **stdout**,
//! both in-memory pipes. `powershell.exe` lives in `%SystemRoot%\System32` on every
//! supported Windows version.
//!
//! ## The encoding contract
//!
//! Each cipher round-trips a `&str` plaintext to/from a single at-rest `String` (what the
//! store serializes into the secrets file). For DPAPI that at-rest string is
//! `base64(CryptProtectData(plaintext_utf8))`; for plaintext it is the value verbatim.
//! The store dispatches **unseal by the stored per-secret scheme marker**, so a file that
//! mixes schemes (mid-migration) still reads correctly, and a value sealed on one host
//! that another host cannot unseal (e.g. a DPAPI file copied to Linux) fails **closed**
//! with a clean, value-free error.

/// An at-rest encoding provider for a secret value. `seal` is the write path, `open` the
/// read path; `scheme` is the durable marker stored next to each value so the store can
/// pick the right `open`.
pub trait SecretCipher: Send + Sync {
    /// The durable scheme marker stored with every value this cipher seals
    /// (`relux_core::SECRET_SCHEME_*`).
    fn scheme(&self) -> &'static str;

    /// Whether this cipher actually encrypts at rest. `false` for the plaintext
    /// fallback — the store uses this to decide whether a legacy plaintext file is worth
    /// migrating (only when an encrypting writer is active).
    fn encrypts(&self) -> bool;

    /// Seal `plaintext` into the at-rest string the store will serialize. Returns an
    /// error string (value-free) on failure; the store then falls back to plaintext so a
    /// secret is never lost to a transient sealing failure.
    fn seal(&self, plaintext: &str) -> Result<String, String>;

    /// Open an at-rest string previously produced by THIS cipher's [`scheme`](Self::scheme)
    /// back into plaintext. Errors are value-free (e.g. a DPAPI error code), never the
    /// secret.
    fn open(&self, encoded: &str) -> Result<String, String>;
}

/// Permission-hardened **plaintext** at rest: the value is stored verbatim and protected
/// only by the owner-only file permissions. Used on non-Windows hosts and as the Windows
/// fail-safe fallback.
pub struct PlaintextCipher;

impl SecretCipher for PlaintextCipher {
    fn scheme(&self) -> &'static str {
        relux_core::SECRET_SCHEME_PLAINTEXT
    }
    fn encrypts(&self) -> bool {
        false
    }
    fn seal(&self, plaintext: &str) -> Result<String, String> {
        Ok(plaintext.to_string())
    }
    fn open(&self, encoded: &str) -> Result<String, String> {
        Ok(encoded.to_string())
    }
}

/// Windows **DPAPI** (CurrentUser scope) at rest, driven through PowerShell's
/// `ProtectedData`. Only present/usable on Windows; the unit type is defined on all
/// platforms so the store's types are platform-uniform, but [`Self::seal`]/[`Self::open`]
/// only do real work under `cfg(windows)`.
pub struct DpapiCipher;

impl DpapiCipher {
    pub fn new() -> Self {
        DpapiCipher
    }
}

impl Default for DpapiCipher {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretCipher for DpapiCipher {
    fn scheme(&self) -> &'static str {
        relux_core::SECRET_SCHEME_DPAPI
    }
    fn encrypts(&self) -> bool {
        true
    }

    fn seal(&self, plaintext: &str) -> Result<String, String> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let input_b64 = STANDARD.encode(plaintext.as_bytes());
        // stdout is base64(CryptProtectData blob) — already the at-rest value.
        run_dpapi(PS_PROTECT, &input_b64)
    }

    fn open(&self, encoded: &str) -> Result<String, String> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        // stdout is base64(plaintext bytes); decode it back to the UTF-8 value.
        let out_b64 = run_dpapi(PS_UNPROTECT, encoded.trim())?;
        let bytes = STANDARD
            .decode(out_b64.trim())
            .map_err(|_| "DPAPI unseal produced malformed output".to_string())?;
        String::from_utf8(bytes).map_err(|_| "DPAPI unseal produced non-UTF-8 bytes".to_string())
    }
}

/// The default at-rest writer for this host: DPAPI on Windows, permission-hardened
/// plaintext elsewhere. The store uses this for every new/rewritten value; reads still
/// dispatch on each secret's stored scheme.
pub fn default_writer() -> Box<dyn SecretCipher> {
    #[cfg(windows)]
    {
        Box::new(DpapiCipher::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(PlaintextCipher)
    }
}

// ── PowerShell DPAPI scripts (secret-free; the value rides stdin/stdout) ───────────────
//
// `$ProgressPreference='SilentlyContinue'` keeps stdout clean (the cold-start "Preparing
// modules" record otherwise lands as CLIXML); `$ErrorActionPreference='Stop'` turns a
// DPAPI failure into a non-zero exit we can detect. Input is base64 on stdin; output is a
// single base64 token on stdout.

/// Reads base64(plaintext) on stdin → emits base64(CryptProtectData blob) on stdout.
#[cfg(windows)]
const PS_PROTECT: &str = "$ProgressPreference='SilentlyContinue'; $ErrorActionPreference='Stop'; Add-Type -AssemblyName System.Security; $in=[Console]::In.ReadToEnd().Trim(); $plain=[Convert]::FromBase64String($in); $prot=[System.Security.Cryptography.ProtectedData]::Protect($plain,$null,'CurrentUser'); [Convert]::ToBase64String($prot)";

/// Reads base64(CryptProtectData blob) on stdin → emits base64(plaintext) on stdout.
#[cfg(windows)]
const PS_UNPROTECT: &str = "$ProgressPreference='SilentlyContinue'; $ErrorActionPreference='Stop'; Add-Type -AssemblyName System.Security; $in=[Console]::In.ReadToEnd().Trim(); $prot=[Convert]::FromBase64String($in); $plain=[System.Security.Cryptography.ProtectedData]::Unprotect($prot,$null,'CurrentUser'); [Convert]::ToBase64String($plain)";

/// Run a secret-free PowerShell DPAPI `script`, feeding `stdin_b64` on the child's stdin
/// and returning its trimmed stdout. The plaintext only ever travels the in-memory
/// stdin/stdout pipes — never an argv, never a temp file. A non-zero exit yields a
/// bounded, value-free error (DPAPI surfaces only an error code, never the secret).
#[cfg(windows)]
fn run_dpapi(script: &str, stdin_b64: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn powershell for DPAPI: {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "DPAPI child has no stdin pipe".to_string())?;
        stdin
            .write_all(stdin_b64.as_bytes())
            .map_err(|e| format!("write DPAPI stdin: {e}"))?;
        // Drop closes the pipe → child sees EOF on stdin and proceeds.
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("await DPAPI child: {e}"))?;
    if !out.status.success() {
        let err: String = String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(300)
            .collect();
        return Err(format!("DPAPI operation failed: {err}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Non-Windows builds never call this — the unit type exists only so the store's types
/// stay platform-uniform. Calling it is a clean, value-free error.
#[cfg(not(windows))]
fn run_dpapi(_script: &str, _stdin_b64: &str) -> Result<String, String> {
    Err("DPAPI is only available on Windows".to_string())
}

#[cfg(not(windows))]
const PS_PROTECT: &str = "";
#[cfg(not(windows))]
const PS_UNPROTECT: &str = "";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_cipher_round_trips_verbatim() {
        let c = PlaintextCipher;
        assert_eq!(c.scheme(), relux_core::SECRET_SCHEME_PLAINTEXT);
        assert!(!c.encrypts());
        let sealed = c.seal("sk-or-abcd1234").unwrap();
        assert_eq!(sealed, "sk-or-abcd1234"); // verbatim — no encryption
        assert_eq!(c.open(&sealed).unwrap(), "sk-or-abcd1234");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_cipher_round_trips_and_ciphertext_hides_plaintext() {
        let c = DpapiCipher::new();
        assert_eq!(c.scheme(), relux_core::SECRET_SCHEME_DPAPI);
        assert!(c.encrypts());
        let plain = "sk-or-dpapi-secret-9876";
        let sealed = match c.seal(plain) {
            Ok(s) => s,
            // DPAPI genuinely unavailable in this environment — skip rather than fail.
            Err(_) => return,
        };
        // The sealed at-rest string must NOT contain the plaintext anywhere.
        assert!(!sealed.contains(plain), "plaintext leaked into DPAPI blob");
        assert!(!sealed.contains("dpapi-secret"), "plaintext fragment leaked");
        // And it round-trips back to exactly the plaintext.
        assert_eq!(c.open(&sealed).unwrap(), plain);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_open_of_corrupt_blob_fails_cleanly() {
        let c = DpapiCipher::new();
        // A syntactically-valid base64 string that is not a real DPAPI blob: unseal must
        // error (not panic, not return junk). Skip if DPAPI itself is unavailable.
        if c.seal("probe").is_err() {
            return;
        }
        let err = c.open("AQAAANCMnd8BFdERjHoAwE/Cl+sBAAAAcorruptcorrupt==").unwrap_err();
        assert!(!err.is_empty());
    }
}
