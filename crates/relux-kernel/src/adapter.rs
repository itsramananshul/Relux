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

use relux_core::{redact_secrets, AdapterKind, RunLogSource};

use crate::live_run_log::RunLogSink;
use crate::run_cancel::CancelToken;

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
    /// True when the run was killed because an operator requested cancellation
    /// mid-flight (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26). Distinct from
    /// `timed_out`: a cancel is intentional and terminal (classified
    /// [`relux_core::RunFailureClass::Cancelled`]), never auto-retried.
    pub cancelled: bool,
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

/// Build the argv for a **resume** of a prior provider session: the same safe,
/// non-bypass invocation as [`build_adapter_args`] plus `--resume <session_id>`,
/// which continues the recorded Claude CLI session non-interactively
/// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3 — OpenClaw `runCliWithSession`). The
/// session id is the already-sanitized, argv-safe value from the run's captured
/// [`relux_core::RunSession`]; it is passed as a single positional argv element
/// (no shell), so there is no injection surface.
///
/// Only the Claude CLI supports this here ([`AdapterKind::resume_supported`]); for
/// any other kind we fall back to the fresh-run args — the kernel never reaches
/// this for an unsupported kind because `run.resume` is refused upstream, but the
/// fallback keeps the function total and honest (a "resume" of a non-resumable
/// adapter is just a fresh run, never a faked continuation).
pub fn build_resume_adapter_args(kind: &AdapterKind, session_id: &str) -> Vec<String> {
    match kind {
        AdapterKind::ClaudeCli => vec![
            "-p".to_string(),
            "--resume".to_string(),
            session_id.to_string(),
            "--permission-mode".to_string(),
            CLAUDE_PERMISSION_MODE.to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ],
        _ => build_adapter_args(kind),
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
    run_adapter_command_streaming_cancellable(spec, None, None)
}

/// The result of a safe, read-only CLI probe (see [`probe_cli_version`]).
///
/// This is a *liveness* check: it proves the binary is installed and runnable,
/// not that the operator is signed in (login is verified on the first real chat
/// turn). It is deliberately scoped that way so the probe stays safe — it never
/// runs an agent turn, never crosses a permission boundary, and never bills.
#[derive(Debug, Clone)]
pub struct CliVersionProbe {
    /// Whether the process was actually spawned (false when the binary was not
    /// found on PATH, so nothing ran).
    pub ran: bool,
    /// True only when the probe spawned, exited cleanly (code 0), and did not
    /// time out.
    pub ok: bool,
    /// The reported version line (first non-empty line of stdout, else stderr),
    /// trimmed and clamped. `None` when the binary printed nothing usable.
    pub version: Option<String>,
    /// A short, secret-free human explanation of the outcome.
    pub detail: String,
}

/// Safely probe a CLI by running `<binary> --version`.
///
/// This reuses the exact safe-spawn contract of the adapter runtime
/// (`docs/RELUX_MASTER_PLAN.md` §14 — "Probe/test environment: is the agent
/// installed?"; `docs/relix-agent-adapters.md` §2): argv-only (no shell), an
/// empty stdin closed immediately, a short wall-clock timeout, a small output
/// cap, secret redaction, and **no bypass/danger flag** (the only argument is
/// the universally read-only `--version`). The binary is resolved on PATH first
/// so a Windows `.cmd`/`.exe` shim is spawnable.
pub fn probe_cli_version(binary: &str) -> CliVersionProbe {
    let program = match find_on_path(binary) {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            return CliVersionProbe {
                ran: false,
                ok: false,
                version: None,
                detail: format!("`{binary}` was not found on PATH."),
            };
        }
    };
    let spec = AdapterCommandSpec {
        program,
        args: vec!["--version".to_string()],
        stdin: String::new(),
        working_dir: None,
        timeout: Duration::from_secs(10),
        max_output_bytes: 8 * 1024,
    };
    match run_adapter_command(&spec) {
        Ok(out) => {
            let version = first_nonempty_line(&out.stdout)
                .or_else(|| first_nonempty_line(&out.stderr));
            if out.success {
                CliVersionProbe {
                    ran: true,
                    ok: true,
                    version,
                    detail: format!("`{binary}` is installed and runnable."),
                }
            } else if out.timed_out {
                CliVersionProbe {
                    ran: true,
                    ok: false,
                    version,
                    detail: format!("`{binary} --version` did not respond within 10s."),
                }
            } else {
                let code = out
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                CliVersionProbe {
                    ran: true,
                    ok: false,
                    version,
                    detail: format!("`{binary} --version` exited with status {code}."),
                }
            }
        }
        Err(e) => CliVersionProbe {
            ran: false,
            ok: false,
            version: None,
            detail: format!("could not run `{binary}`: {e}"),
        },
    }
}

/// The first non-empty, trimmed line of `text`, clamped to a sane length so a
/// chatty CLI cannot blow up the status payload.
fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            let mut s = l.to_string();
            if s.chars().count() > 200 {
                s = s.chars().take(200).collect::<String>();
                s.push('…');
            }
            s
        })
}

/// Like [`run_adapter_command`] but additionally **streams** each stdout/stderr
/// chunk to an optional live [`RunLogSink`] as it is read, so a poll of
/// `GET /v1/relux/runs/:id/logs` can show lines BEFORE the run finalizes
/// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10 — the LIVE run-log seam mirroring
/// Paperclip's `runChildProcess(..., { onLog })`).
///
/// The sink is fed the SAME bytes that are kept for the final capture (up to the
/// byte cap), classified by source and line-buffered + re-redacted + clamped inside
/// the sink's [`relux_core::StreamingRunLog`]. The returned [`AdapterRunOutcome`]
/// is byte-for-byte what the non-streaming path produces (the final, redacted,
/// capped stdout/stderr) — streaming is strictly additive and never alters the
/// captured result. With `sink: None` this is exactly the original behaviour.
///
/// The final, canonical `RunLog` is still built by the kernel at finalize from this
/// outcome; the live sink is a DURING-run view that the kernel drops once the
/// durable log exists, so a line is never double-counted within one log.
pub fn run_adapter_command_streaming(
    spec: &AdapterCommandSpec,
    sink: Option<RunLogSink>,
) -> std::io::Result<AdapterRunOutcome> {
    run_adapter_command_streaming_cancellable(spec, sink, None)
}

/// Like [`run_adapter_command_streaming`] but additionally honours an optional
/// [`CancelToken`]: between its existing poll ticks the spawn checks
/// [`CancelToken::is_cancelled`] and, when an operator has requested cancellation,
/// kills the child (best-effort process tree on Windows; the immediate child
/// otherwise), records a `system` cancellation line on the live sink, and returns
/// an outcome with `cancelled: true` (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26 —
/// the mid-run cancellation seam mirroring OpenClaw's `AbortSignal` + Paperclip's
/// timeout/grace kill in `runChildProcess`).
///
/// This reuses the SAME kill path the timeout already uses; the only new input is
/// the operator cancel flag. With `cancel: None` this is exactly the streaming
/// behaviour, and with `sink: None` too it is the original non-streaming behaviour.
pub fn run_adapter_command_streaming_cancellable(
    spec: &AdapterCommandSpec,
    sink: Option<RunLogSink>,
    cancel: Option<CancelToken>,
) -> std::io::Result<AdapterRunOutcome> {
    if let Some(s) = &sink {
        s.system(format!("spawned adapter '{}'", spec.program));
    }
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
    // Record the child pid so a cancel request can target the process tree (the
    // immediate child is always killed via the owned handle regardless).
    if let Some(c) = &cancel {
        c.set_pid(child.id());
    }

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
    // Each reader thread gets its OWN sink clone so it can append its stream's
    // chunks concurrently (the sink serializes appends internally). The sink also
    // carries the source so the two streams stay classified.
    let out_handle = spawn_capped_reader(
        stdout,
        max,
        sink.as_ref().map(|s| (RunLogSource::Stdout, s.clone())),
    );
    let err_handle = spawn_capped_reader(
        stderr,
        max,
        sink.as_ref().map(|s| (RunLogSource::Stderr, s.clone())),
    );

    // Poll for completion until the timeout (or an operator cancel), then kill.
    // std has no wait-with-timeout, so this is a short-sleep poll loop - cheap and
    // deterministic. The cancel flag is checked on the SAME tick as the deadline,
    // so an operator request is honoured within ~40ms.
    let start = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None => {
                // Operator-requested cancellation: kill the child (best-effort
                // tree) and stop. Intentional + terminal — never auto-retried.
                if cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                    if let Some(s) = &sink {
                        s.system("cancellation requested by operator; terminating adapter");
                    }
                    kill_child_tree(&mut child);
                    let _ = child.wait();
                    cancelled = true;
                    break None;
                }
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

    // Flush any held partial (newline-less) trailing line into the live tail so the
    // last line of a process that didn't end in a newline still streams.
    if let Some(s) = &sink {
        s.flush();
    }

    let exit_code = status.as_ref().and_then(|s| s.code());
    let success = !timed_out && !cancelled && status.map(|s| s.success()).unwrap_or(false);

    Ok(AdapterRunOutcome {
        program: spec.program.clone(),
        exit_code,
        success,
        timed_out,
        cancelled,
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
///
/// When `stream_sink` is set, each chunk that is KEPT (i.e. within the byte cap) is
/// also fed to the live [`RunLogSink`] under its source as it is read — the LIVE
/// streaming seam. The sink is fed exactly the bytes that land in the final capture
/// (never beyond the cap), so the live tail and the finalized log stay consistent.
/// When the cap is first hit, the sink is marked truncated for that source so the
/// in-flight UI shows an honest byte-capped marker.
fn spawn_capped_reader<R>(
    reader: Option<R>,
    max: usize,
    stream_sink: Option<(RunLogSource, RunLogSink)>,
) -> std::thread::JoinHandle<(Vec<u8>, bool)>
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
                            // Stream the kept slice live (lossy UTF-8 per chunk; the
                            // sink line-buffers, so a line split across reads is
                            // reassembled before it is emitted/redacted).
                            if let Some((source, s)) = &stream_sink {
                                s.append(*source, &String::from_utf8_lossy(&chunk[..take]));
                            }
                            if take < n {
                                truncated = true;
                                if let Some((source, s)) = &stream_sink {
                                    s.mark_source_truncation(*source);
                                }
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

/// Kill a child process, best-effort including its descendants.
///
/// On Windows a CLI shim (`claude.cmd` → `node` → …) spawns a tree of grandchild
/// processes; [`std::process::Child::kill`] alone (TerminateProcess on the one pid)
/// would orphan them. So we first ask `taskkill /T /F /PID <pid>` to terminate the
/// whole tree, then always call `kill()` on the owned handle as the guaranteed
/// fallback for the immediate child. On non-Windows we kill the immediate child
/// (descendant cleanup would need a process group; the immediate adapter process is
/// what holds the work, and an orphaned helper exits when its pipes close).
fn kill_child_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let pid = child.id();
        // Best-effort tree kill; ignore failure (the direct kill below is the
        // guaranteed fallback). No shell: argv only, output suppressed.
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
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
    fn resume_args_thread_session_id_through_safe_mode() {
        let args = build_resume_adapter_args(&AdapterKind::ClaudeCli, "sess-abc-123");
        // `--resume <id>` is present, the safe permission mode + JSON envelope are
        // kept, and there is never a bypass/danger flag. The session id is a single
        // positional argv element (no shell), so there is no injection surface.
        assert!(args.windows(2).any(|w| w == ["--resume", "sess-abc-123"]));
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "default"]));
        assert!(args.windows(2).any(|w| w == ["--output-format", "json"]));
        assert!(!args.iter().any(|a| a.contains("dangerously")));
        // A non-Claude kind has no resume here — it degrades to the fresh args.
        assert_eq!(
            build_resume_adapter_args(&AdapterKind::CodexCli, "sess-x"),
            build_adapter_args(&AdapterKind::CodexCli)
        );
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
    fn probe_cli_version_missing_binary_did_not_run() {
        let probe = probe_cli_version("relux-definitely-not-a-real-binary-xyz");
        assert!(!probe.ran, "a missing binary must not spawn anything");
        assert!(!probe.ok);
        assert!(probe.version.is_none());
        assert!(probe.detail.contains("PATH"));
    }

    #[test]
    fn probe_cli_version_runs_and_reports_version() {
        let dir = tempfile::tempdir().unwrap();
        // The fake CLI ignores args and prints a version line, so a `--version`
        // probe sees a clean exit and a captured line — exactly the Available case.
        let bin = write_fake_cli(dir.path(), "fake-brain", "fake-brain 9.9.9");
        let probe = probe_cli_version(&bin.to_string_lossy());
        assert!(probe.ran);
        assert!(probe.ok, "detail: {}", probe.detail);
        assert_eq!(probe.version.as_deref(), Some("fake-brain 9.9.9"));
    }

    #[test]
    fn probe_cli_version_nonzero_exit_is_not_ok() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_failing_cli(dir.path(), "fail-brain");
        let probe = probe_cli_version(&bin.to_string_lossy());
        assert!(probe.ran, "the binary exists, so it was spawned");
        assert!(!probe.ok, "a non-zero exit is not a healthy brain");
        assert!(probe.detail.contains("exited"));
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

    // --- LIVE streaming (the `onLog`-style sink) ---------------------------

    /// Write a fake CLI that prints one stdout line, then one stderr line, then
    /// exits 0 — so a streaming run produces both classified sources.
    fn write_two_stream_cli(dir: &std::path::Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(
                &path,
                "@echo off\r\necho OUT_LINE\r\necho ERR_LINE 1>&2\r\n",
            )
            .unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\necho OUT_LINE\necho ERR_LINE 1>&2\n").unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    /// Write a fake CLI that prints LINE_ONE, sleeps ~1s, then prints LINE_TWO and
    /// exits 0 — used to prove a poll sees LINE_ONE BEFORE the run finalizes.
    fn write_slow_cli(dir: &std::path::Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            // `ping -n 2 localhost` waits ~1s with no extra deps; output is
            // suppressed so only our two lines stream.
            std::fs::write(
                &path,
                "@echo off\r\necho LINE_ONE\r\nping -n 2 127.0.0.1 >NUL\r\necho LINE_TWO\r\n",
            )
            .unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\necho LINE_ONE\nsleep 1\necho LINE_TWO\n").unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    #[test]
    fn streaming_sink_captures_classified_lines_and_system_framing() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_two_stream_cli(dir.path(), "stream-agent");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            max_output_bytes: 4096,
        };
        let live = crate::live_run_log::LiveRunLogs::new();
        let run_id = relux_core::RunId::new("run_stream_1");
        let sink = live.begin(&run_id);
        let outcome = run_adapter_command_streaming(&spec, Some(sink)).expect("spawn ok");
        assert!(outcome.success, "stderr: {}", outcome.stderr);
        // The final captured output is unchanged by streaming.
        assert!(outcome.stdout.contains("OUT_LINE"));
        assert!(outcome.stderr.contains("ERR_LINE"));
        // The live buffer carries the system spawn line + both classified streams.
        let snap = live.snapshot(&run_id, None).expect("live buffer");
        assert!(
            snap.lines.iter().any(|l| l.source == RunLogSource::System
                && l.text.contains("spawned adapter")),
            "system spawn line missing: {:?}",
            snap.lines
        );
        assert!(
            snap.lines
                .iter()
                .any(|l| l.source == RunLogSource::Stdout && l.text.contains("OUT_LINE")),
            "stdout line missing: {:?}",
            snap.lines
        );
        assert!(
            snap.lines
                .iter()
                .any(|l| l.source == RunLogSource::Stderr && l.text.contains("ERR_LINE")),
            "stderr line missing: {:?}",
            snap.lines
        );
    }

    #[test]
    fn streaming_lines_are_visible_before_the_run_finalizes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_slow_cli(dir.path(), "slow-agent");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(20),
            max_output_bytes: 4096,
        };
        let live = crate::live_run_log::LiveRunLogs::new();
        let run_id = relux_core::RunId::new("run_stream_slow");
        let sink = live.begin(&run_id);

        // Run the (slow) process on a worker thread; the main thread polls the live
        // tail. The fake prints LINE_ONE, then sleeps ~1s, then LINE_TWO — so the
        // poll observes LINE_ONE well before the worker returns.
        let worker = std::thread::spawn(move || run_adapter_command_streaming(&spec, Some(sink)));

        let mut saw_line_one_live = false;
        // Poll up to ~6s; bail as soon as LINE_ONE appears while the worker runs.
        for _ in 0..120 {
            if worker.is_finished() {
                break;
            }
            if let Some(snap) = live.snapshot(&run_id, None) {
                if snap
                    .lines
                    .iter()
                    .any(|l| l.source == RunLogSource::Stdout && l.text.contains("LINE_ONE"))
                {
                    // LINE_ONE is live AND the process hasn't finished (it is mid
                    // sleep) — LINE_TWO must not be present yet.
                    saw_line_one_live = !worker.is_finished();
                    if saw_line_one_live {
                        assert!(
                            !snap.lines.iter().any(|l| l.text.contains("LINE_TWO")),
                            "LINE_TWO appeared before the sleep elapsed: {:?}",
                            snap.lines
                        );
                    }
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let outcome = worker.join().expect("worker joined").expect("spawn ok");
        assert!(outcome.success, "stderr: {}", outcome.stderr);
        assert!(
            saw_line_one_live,
            "LINE_ONE was not observed via the live tail before the run finalized"
        );
        // After completion the live buffer holds both lines (flush emitted any tail).
        let snap = live.snapshot(&run_id, None).expect("live buffer");
        assert!(snap.lines.iter().any(|l| l.text.contains("LINE_ONE")));
        assert!(snap.lines.iter().any(|l| l.text.contains("LINE_TWO")));
    }

    #[test]
    fn non_streaming_run_is_unchanged_with_no_sink() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_fake_cli(dir.path(), "plain-agent", "PLAIN_OUT");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            max_output_bytes: 1024,
        };
        // `run_adapter_command` (no sink) and the streaming variant with `None`
        // produce the same successful capture.
        let a = run_adapter_command(&spec).expect("spawn ok");
        let b = run_adapter_command_streaming(&spec, None).expect("spawn ok");
        assert!(a.success && b.success);
        assert!(a.stdout.contains("PLAIN_OUT") && b.stdout.contains("PLAIN_OUT"));
        // A successful run is never marked cancelled.
        assert!(!a.cancelled && !b.cancelled);
    }

    // --- Mid-run cancellation (the operator AbortSignal) -------------------

    /// Write a fake CLI that loops for a long time (~10s) so a cancel can kill it
    /// well before it would finish on its own. Cross-platform.
    fn write_long_running_cli(dir: &std::path::Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            // ~10s wait (ping -n 11 ≈ 10s); prints a line first so a live tail can
            // confirm it started.
            std::fs::write(
                &path,
                "@echo off\r\necho STARTED\r\nping -n 11 127.0.0.1 >NUL\r\necho DONE\r\n",
            )
            .unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\necho STARTED\nsleep 10\necho DONE\n").unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    #[test]
    fn cancellation_kills_a_running_adapter_and_marks_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_long_running_cli(dir.path(), "long-agent");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            // A long timeout so the run can only end via cancellation, not the
            // deadline — proving the cancel path, not the timeout path.
            timeout: Duration::from_secs(120),
            max_output_bytes: 4096,
        };
        let live = crate::live_run_log::LiveRunLogs::new();
        let cancels = crate::run_cancel::RunCancellations::new();
        let run_id = relux_core::RunId::new("run_cancel_adapter");
        let sink = live.begin(&run_id);
        let token = cancels.begin(&run_id);

        // Run the long process on a worker; the main thread waits until it has
        // started (the live STARTED line) then requests cancellation.
        let worker = std::thread::spawn(move || {
            run_adapter_command_streaming_cancellable(&spec, Some(sink), Some(token))
        });
        let started = Instant::now();
        loop {
            if live
                .snapshot(&run_id, None)
                .map(|s| s.lines.iter().any(|l| l.text.contains("STARTED")))
                .unwrap_or(false)
            {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(20), "process never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(cancels.request(&run_id), crate::run_cancel::CancelOutcome::Requested);

        // The worker must return promptly (well under the 120s timeout) BECAUSE it
        // was cancelled, not because it finished or timed out.
        let outcome = worker.join().expect("worker joined").expect("spawn ok");
        assert!(outcome.cancelled, "outcome must be marked cancelled");
        assert!(!outcome.success, "a cancelled run is not a success");
        assert!(!outcome.timed_out, "a cancel is distinct from a timeout");
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "cancel did not stop the run promptly (elapsed {:?})",
            started.elapsed()
        );
        // The cancellation system line is on the live tail; DONE never printed.
        let snap = live.snapshot(&run_id, None).expect("live buffer");
        assert!(
            snap.lines.iter().any(|l| l.source == RunLogSource::System
                && l.text.contains("cancellation requested")),
            "missing cancellation system line: {:?}",
            snap.lines
        );
        assert!(
            !snap.lines.iter().any(|l| l.text.contains("DONE")),
            "the process was not actually killed (DONE printed): {:?}",
            snap.lines
        );
    }

    #[test]
    fn no_cancel_request_runs_to_completion() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_fake_cli(dir.path(), "uncancelled-agent", "FINISHED_OK");
        let spec = AdapterCommandSpec {
            program: bin.to_string_lossy().to_string(),
            args: vec![],
            stdin: String::new(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            max_output_bytes: 1024,
        };
        let cancels = crate::run_cancel::RunCancellations::new();
        let token = cancels.begin(&relux_core::RunId::new("run_uncancelled"));
        // A token is present but never requested → the run completes normally.
        let outcome =
            run_adapter_command_streaming_cancellable(&spec, None, Some(token)).expect("spawn ok");
        assert!(outcome.success);
        assert!(!outcome.cancelled);
        assert!(outcome.stdout.contains("FINISHED_OK"));
    }
}
