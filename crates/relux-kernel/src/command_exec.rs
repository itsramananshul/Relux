//! The **governed command-tool executor** — runs an operator-configured
//! [`relux_core::CommandToolConfig`] argv-only, confined to the owning plugin's
//! install directory, with a hard timeout and bounded, secret-redacted output.
//!
//! Companion to the managed-stdio MCP client ([`crate::mcp_stdio`]) — it shares that
//! module's spawn posture (argv only, never a shell; bounded capture; honest failures)
//! but speaks no protocol: it runs one process to completion (or kills it on timeout)
//! and returns its exit status + captured streams as a shaped JSON value the tool
//! invoke path returns to the operator.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! - **Hermes** `reference/hermes-agent-main/tools/environments/local.py`
//!   (`subprocess.Popen(args, cwd=_popen_cwd, ...)` L522-534, `_resolve_safe_cwd`
//!   L41-71): an argv array spawned in a **validated** working directory, output piped
//!   and captured, a default 120s timeout that kills the process group on expiry. We
//!   mirror the argv+cwd+timeout+capture shape; we go stricter on the cwd (it must
//!   canonicalize INSIDE the plugin's install dir — a symlink escape is refused).
//! - **openclaw** `src/agents/bash-tools.exec-runtime.ts` (`DEFAULT_MAX_OUTPUT`
//!   clamp, L112-129): captured output is bounded to a fixed ceiling. We mirror it:
//!   each stream is capped at [`MAX_CAPTURE_BYTES`] and flagged `truncated`.
//!
//! ## Safety contract (binding)
//!
//! - **argv only.** `program` + `args` are re-validated with
//!   [`relux_core::validate_stdio_command`] on every run (defense in depth) and handed
//!   to [`std::process::Command`] as individual argv elements. No shell is ever
//!   invoked, so there is no metacharacter-injection surface.
//! - **cwd confined.** The spawn directory is the plugin's install dir, or a `cwd`
//!   validated to canonicalize INSIDE it ([`crate::secret_store::validate_managed_cwd`]).
//! - **bounded + redacted.** stdout/stderr are each captured up to
//!   [`MAX_CAPTURE_BYTES`], then secret-redacted ([`relux_core::redact_secrets`]).
//! - **timed.** The child is killed if it exceeds the configured timeout; the outcome
//!   is flagged `timed_out` (an honest failure, never a fabricated success).
//! - **no inherited danger.** v1 inherits the parent environment unchanged and injects
//!   nothing; the config carries no secrets.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use relux_core::CommandToolConfig;

/// Max bytes captured from EACH of the child's stdout / stderr. A runaway process that
/// floods a stream is bounded here instead of growing memory without limit.
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// How often the wait loop polls the child while enforcing the timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Why a governed command-tool execution could not be carried out (a setup failure
/// distinct from the command itself exiting non-zero, which is a successful run with a
/// non-zero `exit_code`). Fail-closed and value-free.
#[derive(Debug, thiserror::Error)]
pub enum CommandExecError {
    /// The stored command failed the argv safety contract at run time.
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    /// The configured `cwd` could not be resolved inside the install dir.
    #[error("invalid working directory: {0}")]
    InvalidCwd(String),
    /// The install directory itself is missing / unreadable.
    #[error("plugin install directory is unavailable: {0}")]
    InstallDirUnavailable(String),
    /// Building the invocation argv from the input failed (e.g. a missing required arg).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The process could not be spawned (missing program, permission, …).
    #[error("failed to spawn '{program}': {message}")]
    Spawn { program: String, message: String },
    /// An OS error occurred while waiting on / reading from the child.
    #[error("command execution failed: {0}")]
    Io(String),
}

/// The result of running a governed command tool to completion (or to a timeout kill).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRunOutcome {
    /// The process exit code, when the OS reported one (`None` on a signal/timeout kill).
    pub exit_code: Option<i32>,
    /// True iff the process exited with status 0 (the canonical command success).
    pub success: bool,
    /// True iff the child was killed because it exceeded the configured timeout.
    pub timed_out: bool,
    /// Captured stdout (bounded + secret-redacted).
    pub stdout: String,
    /// Captured stderr (bounded + secret-redacted).
    pub stderr: String,
    /// True iff stdout hit [`MAX_CAPTURE_BYTES`] and was truncated.
    pub stdout_truncated: bool,
    /// True iff stderr hit [`MAX_CAPTURE_BYTES`] and was truncated.
    pub stderr_truncated: bool,
    /// Wall-clock duration of the run in milliseconds.
    pub duration_ms: u64,
}

/// Resolve the spawn directory for a command tool: the configured `cwd` validated to
/// canonicalize INSIDE `install_dir`, or `install_dir` itself when no `cwd` is set.
fn resolve_cwd(config: &CommandToolConfig, install_dir: &Path) -> Result<PathBuf, CommandExecError> {
    match &config.cwd {
        Some(cwd) => crate::secret_store::validate_managed_cwd(cwd, install_dir)
            .map_err(CommandExecError::InvalidCwd),
        None => {
            let canon = install_dir
                .canonicalize()
                .map_err(|e| CommandExecError::InstallDirUnavailable(e.to_string()))?;
            if !canon.is_dir() {
                return Err(CommandExecError::InstallDirUnavailable(format!(
                    "{} is not a directory",
                    canon.display()
                )));
            }
            Ok(canon)
        }
    }
}

/// Execute a governed command tool for ONE invocation and shape the result as JSON.
///
/// Resolves the spawn directory inside `install_dir`, builds the invocation argv from
/// `config` + `input` (only declared input args are appended), runs the command
/// argv-only with the config's timeout, and returns a shaped, bounded, redacted
/// `{ exit_code, success, timed_out, stdout, stderr, duration_ms, … }` value. A
/// non-zero exit is a successful *run* (`success: false`) — never an error; only a
/// setup failure (bad cwd, spawn failure) is an [`CommandExecError`].
pub fn execute_command_tool(
    config: &CommandToolConfig,
    install_dir: &str,
    input: &serde_json::Value,
) -> Result<serde_json::Value, CommandExecError> {
    let argv = relux_core::build_command_argv(config, input)
        .map_err(|e| CommandExecError::InvalidInput(e.to_string()))?;
    let cwd = resolve_cwd(config, Path::new(install_dir))?;
    let timeout = Duration::from_millis(relux_core::clamp_command_timeout(config.timeout_ms));
    let outcome = run_command(&config.program, &argv, &cwd, timeout)?;
    Ok(shape_outcome(&outcome))
}

/// Spawn `program` with `args` (argv only — never a shell) in `cwd`, capture bounded
/// stdout/stderr, enforce `timeout` (killing the child on expiry), and return the
/// [`CommandRunOutcome`]. The command is re-validated before spawn (defense in depth).
pub fn run_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<CommandRunOutcome, CommandExecError> {
    relux_core::validate_stdio_command(program, args)
        .map_err(|e| CommandExecError::InvalidCommand(e.to_string()))?;

    let started = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CommandExecError::Spawn {
            program: program.to_string(),
            message: e.to_string(),
        })?;

    // Drain each stream on its own thread so a full pipe can never deadlock the wait.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    if let Some(mut s) = stdout {
        thread::spawn(move || {
            let _ = out_tx.send(read_capped(&mut s));
        });
    } else {
        let _ = out_tx.send((Vec::new(), false));
    }
    if let Some(mut s) = stderr {
        thread::spawn(move || {
            let _ = err_tx.send(read_capped(&mut s));
        });
    } else {
        let _ = err_tx.send((Vec::new(), false));
    }

    // Poll for completion until the deadline; kill on timeout.
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(CommandExecError::Io(e.to_string())),
        }
    };

    // The reader threads end at EOF (the child's streams close on exit / kill).
    let (out_bytes, out_trunc) = out_rx.recv().unwrap_or_default();
    let (err_bytes, err_trunc) = err_rx.recv().unwrap_or_default();
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    let exit_code = status.and_then(|s| s.code());
    let success = !timed_out && status.map(|s| s.success()).unwrap_or(false);

    Ok(CommandRunOutcome {
        exit_code,
        success,
        timed_out,
        stdout: redact_bounded(&out_bytes),
        stderr: redact_bounded(&err_bytes),
        stdout_truncated: out_trunc,
        stderr_truncated: err_trunc,
        duration_ms,
    })
}

/// Read up to [`MAX_CAPTURE_BYTES`] from a stream, returning `(bytes, truncated)`.
/// Reads (and discards) the remainder so the child never blocks on a full pipe.
fn read_capped<R: Read>(reader: &mut R) -> (Vec<u8>, bool) {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < MAX_CAPTURE_BYTES {
                    let room = MAX_CAPTURE_BYTES - buf.len();
                    let take = room.min(n);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (buf, truncated)
}

/// Decode (lossy UTF-8) and secret-redact a captured stream.
fn redact_bounded(bytes: &[u8]) -> String {
    relux_core::redact_secrets(&String::from_utf8_lossy(bytes))
}

/// Shape a [`CommandRunOutcome`] into the JSON value the tool invoke path returns.
fn shape_outcome(o: &CommandRunOutcome) -> serde_json::Value {
    serde_json::json!({
        "exit_code": o.exit_code,
        "success": o.success,
        "timed_out": o.timed_out,
        "stdout": o.stdout,
        "stderr": o.stderr,
        "stdout_truncated": o.stdout_truncated,
        "stderr_truncated": o.stderr_truncated,
        "duration_ms": o.duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::CommandInputArg;

    /// A portable `(program, args)` that prints `text` to stdout and exits 0.
    fn echo(text: &str) -> (String, Vec<String>) {
        if cfg!(windows) {
            ("cmd".into(), vec!["/C".into(), "echo".into(), text.into()])
        } else {
            ("printf".into(), vec!["%s".into(), text.into()])
        }
    }

    fn install_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn cfg(program: &str, args: &[&str]) -> CommandToolConfig {
        CommandToolConfig {
            plugin_id: "relux-plugin-x".into(),
            tool_name: "repo.run".into(),
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            input_args: Vec::new(),
            timeout_ms: relux_core::DEFAULT_COMMAND_TIMEOUT_MS,
            enabled: true,
        }
    }

    #[test]
    fn runs_a_safe_fixture_command_and_captures_stdout() {
        let dir = install_dir();
        let (program, args) = echo("relux-ok");
        let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = execute_command_tool(
            &cfg(&program, &argv),
            dir.path().to_str().unwrap(),
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(out["success"], serde_json::json!(true));
        assert_eq!(out["exit_code"], serde_json::json!(0));
        assert!(out["stdout"].as_str().unwrap().contains("relux-ok"));
        assert_eq!(out["timed_out"], serde_json::json!(false));
    }

    #[test]
    fn a_nonzero_exit_is_a_successful_run_with_success_false() {
        let dir = install_dir();
        let c = if cfg!(windows) {
            cfg("cmd", &["/C", "exit", "3"])
        } else {
            cfg("sh", &["-c", "exit 3"])
        };
        let out = execute_command_tool(&c, dir.path().to_str().unwrap(), &serde_json::json!({}))
            .unwrap();
        assert_eq!(out["success"], serde_json::json!(false));
        assert_eq!(out["exit_code"], serde_json::json!(3));
    }

    #[test]
    fn a_long_command_is_killed_on_timeout() {
        let dir = install_dir();
        // A ~3s sleeper, with a 200ms timeout.
        let mut c = if cfg!(windows) {
            cfg("cmd", &["/C", "ping", "-n", "4", "127.0.0.1"])
        } else {
            cfg("sleep", &["3"])
        };
        c.timeout_ms = 200;
        let out = execute_command_tool(&c, dir.path().to_str().unwrap(), &serde_json::json!({}))
            .unwrap();
        assert_eq!(out["timed_out"], serde_json::json!(true));
        assert_eq!(out["success"], serde_json::json!(false));
    }

    #[test]
    fn a_cwd_escaping_the_install_dir_is_refused() {
        let dir = install_dir();
        let mut c = cfg("cmd", &["/C", "echo", "x"]);
        c.cwd = Some("../../".to_string());
        let err = execute_command_tool(&c, dir.path().to_str().unwrap(), &serde_json::json!({}));
        assert!(matches!(err, Err(CommandExecError::InvalidCwd(_))));
    }

    #[test]
    fn a_missing_required_input_fails_closed_before_spawn() {
        let dir = install_dir();
        let mut c = cfg("cmd", &["/C", "echo"]);
        c.input_args = vec![CommandInputArg {
            name: "FILE".into(),
            description: String::new(),
            required: true,
        }];
        let err = execute_command_tool(&c, dir.path().to_str().unwrap(), &serde_json::json!({}));
        assert!(matches!(err, Err(CommandExecError::InvalidInput(_))));
    }

    #[test]
    fn output_is_secret_redacted() {
        let dir = install_dir();
        // Echo a token-shaped string; redaction must mask it.
        let secret = "sk-ant-abcdefghij1234567890";
        let (program, args) = echo(secret);
        let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = execute_command_tool(
            &cfg(&program, &argv),
            dir.path().to_str().unwrap(),
            &serde_json::json!({}),
        )
        .unwrap();
        let stdout = out["stdout"].as_str().unwrap();
        assert!(!stdout.contains(secret), "a secret-shaped token must be redacted: {stdout}");
    }
}
