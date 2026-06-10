//! The local CLI adapter runtime (Adapter Runtime v1).
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.1 (Adapter Plugins) and
//! section 14 (the first plugin-powered run). This module is the one place the
//! kernel spawns a real local process to drive an assigned task: a coding-agent
//! CLI the operator already has installed (Claude CLI, Codex CLI) or a generic
//! local command. The pure config/recognition types live in
//! `relux_core::adapter`; this module holds PATH discovery, argv construction,
//! prompt composition, and a bounded process spawn.
//!
//! Safety properties enforced here (the product safety bar):
//!
//! - **argv only.** Commands are built as an argv array and passed straight to
//!   [`std::process::Command`] - never a shell string, so there is no shell
//!   interpolation/injection surface. The (potentially multi-line) task prompt is
//!   fed on **stdin**, not as an argument, so there is no arg-escaping surface and
//!   it works uniformly across native binaries and Windows `.cmd` shims.
//! - **No bypass flags.** The Claude invocation uses `--permission-mode default`
//!   (a safe, non-bypass mode). Relux never passes
//!   `--dangerously-skip-permissions` or any danger/bypass flag.
//! - **Bounded.** Every run has a wall-clock timeout (the child is killed on
//!   expiry) and a stdout/stderr byte cap. The child's stdin is closed right after
//!   the prompt is written, so it can never block waiting for interactive input.
//! - **Redacted.** Captured stdout/stderr is scrubbed of obvious secrets before
//!   it is returned (and persisted to a transcript).
//!
//! Discovery (`find_on_path`) is read-only: it inspects `PATH` (and `PATHEXT` on
//! Windows) and never executes anything.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use relux_core::{redact_secrets, AdapterKind};

/// The safe, non-bypass permission mode Relux passes to the Claude CLI. This is
/// deliberately NOT `bypassPermissions` and Relux never passes
/// `--dangerously-skip-permissions`.
pub const CLAUDE_PERMISSION_MODE: &str = "default";

/// A fully-resolved spec for one adapter process invocation.
#[derive(Debug, Clone)]
pub struct AdapterCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    /// The composed prompt, written to the child's stdin then closed. Kept off
    /// the argv so a multi-line prompt needs no shell/arg escaping.
    pub stdin: String,
    pub working_dir: Option<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

/// The outcome of running an adapter command. stdout/stderr are already
/// secret-redacted and capped.
#[derive(Debug, Clone)]
pub struct AdapterRunOutcome {
    pub program: String,
    /// The process exit code, when the process exited normally.
    pub exit_code: Option<i32>,
    /// True only on a clean exit with code 0 (and no timeout).
    pub success: bool,
    /// True when the run was killed because it exceeded its timeout.
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Real measured wall-clock duration of the subprocess, in milliseconds. This
    /// is honest process timing (the adapter layer already reads a monotonic
    /// `Instant` for the timeout); it never feeds the kernel's deterministic
    /// logical clock and only exists for real CLI runs.
    pub duration_ms: u64,
}

/// Build the argv (after the program name) for a given adapter kind. The prompt
/// is delivered on stdin (see [`AdapterCommandSpec::stdin`]), NOT as an argument,
/// so these are just the mode/permission flags. Pure and unit-tested.
///
/// - Claude: `-p --permission-mode default --output-format json` (print/
///   non-interactive, safe non-bypass permission mode, structured result
///   envelope; prompt read from stdin). The JSON envelope lets the kernel parse
///   an honest summary + cost/usage (master plan section 9.6) while still storing
///   the raw, redacted output. It is NOT a bypass/danger flag.
/// - Codex: `exec` (the non-interactive subcommand; prompt read from stdin). Left
///   as plain text - its JSONL event stream is a separate, larger parsing job.
/// - Command: no extra args (the operator's binary reads the prompt from stdin).
/// - LocalPrime: never spawned (returns an empty argv defensively).
pub fn build_adapter_args(kind: &AdapterKind) -> Vec<String> {
    match kind {
        AdapterKind::ClaudeCli => vec![
            "-p".to_string(),
            "--permission-mode".to_string(),
            CLAUDE_PERMISSION_MODE.to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ],
        AdapterKind::CodexCli => vec!["exec".to_string()],
        AdapterKind::Command | AdapterKind::LocalPrime => Vec::new(),
    }
}

/// Compose the task prompt handed to a CLI adapter. It states who the agent is
/// (name + persona), the task title and JSON input, and asks the CLI to do the
/// work and report concisely. Kept conservative for v1.
pub fn compose_prompt(
    agent_name: &str,
    persona: Option<&str>,
    task_title: &str,
    task_input: &serde_json::Value,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "You are {agent_name}, an agent operating inside the Relux control plane.\n"
    ));
    if let Some(p) = persona {
        let p = p.trim();
        if !p.is_empty() {
            out.push_str(p);
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(&format!("Task: {task_title}\n"));
    let input_str =
        serde_json::to_string_pretty(task_input).unwrap_or_else(|_| task_input.to_string());
    if input_str.trim() != "null" {
        out.push_str("Task input (JSON):\n");
        out.push_str(&input_str);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(
        "Do the work for this task using your available local tools. \
         When you are done, report concisely what you did and any results. \
         Do not ask for confirmation; if you cannot proceed, explain why.",
    );
    out
}

/// Find an executable named `binary` on the current `PATH`, returning its
/// resolved path. Read-only: it never executes anything.
///
/// If `binary` already contains a path separator, it is treated as a direct path
/// and returned if it exists. On Windows, bare names are probed against the
/// `PATHEXT` extensions (`.exe`, `.cmd`, `.bat`, ...).
pub fn find_on_path(binary: &str) -> Option<PathBuf> {
    let binary = binary.trim();
    if binary.is_empty() {
        return None;
    }

    // A binary given with a path separator (or extension) is used as-is.
    if binary.contains('/') || binary.contains('\\') {
        let p = PathBuf::from(binary);
        return if p.is_file() { Some(p) } else { None };
    }

    let path_var = std::env::var_os("PATH")?;
    let exts = path_extensions();
    // On Windows a bare, extension-less file (e.g. an npm shell shim named
    // `claude` with no extension) is NOT directly executable by CreateProcess - it
    // is a Unix-style script. So when probing a bare name there, prefer a PATHEXT
    // variant (`claude.cmd` / `claude.exe`, which Rust runs correctly, routing
    // `.cmd`/`.bat` through cmd.exe) and only accept the extension-less file when
    // the name already carries an executable extension. On non-Windows `exts` is
    // empty and the bare file is the executable, so behavior is unchanged.
    let bare_is_executable = bare_name_is_executable(binary, &exts);
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if bare_is_executable {
            let candidate = dir.join(binary);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        for ext in &exts {
            let candidate = dir.join(format!("{binary}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether a bare program name is directly runnable as-is. On non-Windows
/// (`exts` empty) it always is. On Windows it is only runnable when it already
/// ends with a known executable extension (e.g. `foo.exe`); a bare `foo` must be
/// resolved to a PATHEXT variant instead.
fn bare_name_is_executable(name: &str, exts: &[String]) -> bool {
    if exts.is_empty() {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    exts.iter().any(|e| lower.ends_with(e.as_str()))
}

/// The executable extensions to probe for a bare binary name. On Windows this is
/// `PATHEXT` (lowercased), falling back to a sane default; elsewhere it is empty
/// (Unix executables carry no extension).
fn path_extensions() -> Vec<String> {
    if cfg!(windows) {
        match std::env::var("PATHEXT") {
            Ok(v) if !v.trim().is_empty() => v
                .split(';')
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
            _ => vec![
                ".com".to_string(),
                ".exe".to_string(),
                ".bat".to_string(),
                ".cmd".to_string(),
            ],
        }
    } else {
        Vec::new()
    }
}

/// Spawn the adapter command, wait up to its timeout, capture and cap
/// stdout/stderr, redact secrets, and report the outcome. The child's stdin is
/// closed so it can never block on interactive input. On timeout the child is
/// killed.
pub fn run_adapter_command(spec: &AdapterCommandSpec) -> std::io::Result<AdapterRunOutcome> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &spec.working_dir {
        if !dir.trim().is_empty() {
            command.current_dir(dir);
        }
    }

    let mut child = command.spawn()?;

    // Feed the prompt on stdin from a dedicated thread, then close stdin (EOF).
    // A dedicated thread avoids a deadlock when the prompt is larger than the
    // pipe buffer and the child starts writing output before draining stdin.
    if let Some(mut stdin) = child.stdin.take() {
        let prompt = spec.stdin.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
            // Dropping `stdin` here closes it, signalling EOF.
        });
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let max = spec.max_output_bytes;
    let out_handle = spawn_capped_reader(stdout, max);
    let err_handle = spawn_capped_reader(stderr, max);

    // Poll for completion until the timeout, then kill. std has no wait-with-
    // timeout, so this is a short-sleep poll loop - cheap and deterministic.
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None => {
                if start.elapsed() >= spec.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    };

    let duration_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    let (stdout_bytes, stdout_truncated) = out_handle.join().unwrap_or_default();
    let (stderr_bytes, stderr_truncated) = err_handle.join().unwrap_or_default();

    let exit_code = status.as_ref().and_then(|s| s.code());
    let success = !timed_out && status.map(|s| s.success()).unwrap_or(false);

    Ok(AdapterRunOutcome {
        program: spec.program.clone(),
        exit_code,
        success,
        timed_out,
        stdout: redact_secrets(&String::from_utf8_lossy(&stdout_bytes)),
        stderr: redact_secrets(&String::from_utf8_lossy(&stderr_bytes)),
        stdout_truncated,
        stderr_truncated,
        duration_ms,
    })
}

/// Read a child pipe to EOF on its own thread, keeping at most `max` bytes and
/// reporting whether more was produced (truncation). The reader always drains to
/// EOF so the child never blocks on a full pipe.
fn spawn_capped_reader<R>(reader: Option<R>, max: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut kept: Vec<u8> = Vec::new();
        let mut truncated = false;
        if let Some(mut reader) = reader {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        if kept.len() < max {
                            let take = (max - kept.len()).min(n);
                            kept.extend_from_slice(&chunk[..take]);
                            if take < n {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        (kept, truncated)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_args_use_safe_non_bypass_mode() {
        let args = build_adapter_args(&AdapterKind::ClaudeCli);
        assert_eq!(
            args,
            vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                "default".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ]
        );
        // The structured-output flag is present, the safe permission mode is kept,
        // and there is never a bypass/danger flag; the prompt is never an argv
        // element.
        assert!(args.windows(2).any(|w| w == ["--output-format", "json"]));
        assert!(args.iter().any(|a| a == "default"));
        assert!(!args.iter().any(|a| a.contains("dangerously")));
        assert!(!args.iter().any(|a| a == "bypassPermissions"));
    }

    #[test]
    fn codex_and_command_args() {
        assert_eq!(
            build_adapter_args(&AdapterKind::CodexCli),
            vec!["exec".to_string()]
        );
        assert!(build_adapter_args(&AdapterKind::Command).is_empty());
        assert!(build_adapter_args(&AdapterKind::LocalPrime).is_empty());
    }

    #[test]
    fn prompt_includes_persona_title_and_input() {
        let prompt = compose_prompt(
            "code-agent",
            Some("You write careful code."),
            "Fix the failing test",
            &serde_json::json!({ "repo": "acme/api" }),
        );
        assert!(prompt.contains("code-agent"));
        assert!(prompt.contains("You write careful code."));
        assert!(prompt.contains("Fix the failing test"));
        assert!(prompt.contains("acme/api"));
        assert!(prompt.contains("report concisely"));
    }

    #[test]
    fn prompt_omits_null_input_block() {
        let prompt = compose_prompt("a", None, "t", &serde_json::Value::Null);
        assert!(!prompt.contains("Task input"));
    }

    #[test]
    fn find_on_path_returns_none_for_missing_binary() {
        assert!(find_on_path("relux-definitely-not-a-real-binary-xyz").is_none());
        assert!(find_on_path("").is_none());
    }

    #[test]
    fn bare_name_executability_is_windows_aware() {
        // Non-Windows (no PATHEXT entries): a bare name is the executable.
        assert!(bare_name_is_executable("claude", &[]));
        // Windows (PATHEXT present): a bare, extension-less name is NOT directly
        // runnable - it must resolve to a `.cmd`/`.exe` variant first.
        let exts = vec![".com".to_string(), ".exe".to_string(), ".cmd".to_string()];
        assert!(!bare_name_is_executable("claude", &exts));
        assert!(bare_name_is_executable("claude.cmd", &exts));
        assert!(bare_name_is_executable("CLAUDE.EXE", &exts));
    }

    // --- Fake-binary spawn tests (no real Claude/Codex) --------------------

    /// Write a fake CLI into `dir` that prints `output` to stdout and exits 0.
    /// Cross-platform: a `.cmd` on Windows, an executable `.sh` elsewhere.
    fn write_fake_cli(dir: &std::path::Path, name: &str, output: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(&path, format!("@echo off\r\necho {output}\r\n")).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\necho '{output}'\n")).unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    /// Write a fake CLI that exits with a non-zero code.
    fn write_failing_cli(dir: &std::path::Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(&path, "@echo off\r\necho boom 1>&2\r\nexit /b 3\r\n").unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\necho boom 1>&2\nexit 3\n").unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    #[test]
    fn runs_fake_cli_and_captures_output() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_fake_cli(dir.path(), "fake-agent", "DETERMINISTIC_OUTPUT");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec!["ignored-arg".to_string()],
            stdin: "prompt on stdin".to_string(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            max_output_bytes: 1024,
        };
        let outcome = run_adapter_command(&spec).expect("spawn ok");
        assert!(outcome.success, "stderr: {}", outcome.stderr);
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout.contains("DETERMINISTIC_OUTPUT"));
        assert!(!outcome.timed_out);
    }

    #[test]
    fn nonzero_exit_is_reported_as_failure() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_failing_cli(dir.path(), "fail-agent");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            max_output_bytes: 1024,
        };
        let outcome = run_adapter_command(&spec).expect("spawn ok");
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(3));
    }

    #[test]
    fn output_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        // The fake prints a ~20-char line; cap to 4 bytes so it must truncate.
        let bin = write_fake_cli(dir.path(), "verbose-agent", "AAAAAAAAAAAAAAAAAAAA");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            max_output_bytes: 4,
        };
        let outcome = run_adapter_command(&spec).expect("spawn ok");
        assert!(outcome.stdout.len() <= 4, "stdout not capped: {:?}", outcome.stdout);
        assert!(outcome.stdout_truncated);
    }

    #[test]
    fn missing_program_is_an_io_error_not_a_panic() {
        let spec = AdapterCommandSpec {
            program: "relux-definitely-not-a-real-binary-xyz".to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        };
        assert!(run_adapter_command(&spec).is_err());
    }
}
