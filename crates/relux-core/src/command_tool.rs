//! The **governed command tool** config — the safe, argv-only data model behind
//! turning a detected repo script/binary (a `cli_command` capability candidate) into
//! a real, gated Relux tool.
//!
//! ## Why this exists
//!
//! `crate::mcp` activates an MCP source in one click. But an ordinary repo that only
//! ships a CLI / script / Cargo binary had **no** governed activation path — the
//! capability candidate was an honest but dead-end `manual` record
//! (`crates/relux-kernel/src/capability_detect.rs`). This module is the smallest
//! production-shaped data model that lets an operator configure such a candidate into
//! a callable tool **safely**: the command is stored as an `argv` program + fixed
//! args + an optional `cwd` confined to the plugin's own install directory, never a
//! shell string. It carries no secrets and executes nothing itself — the kernel
//! (`crates/relux-kernel/src/command_exec.rs`) runs it argv-only, behind the same
//! permission + approval + audit gates as any other tool, only after an explicit,
//! gated invocation.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! Read before writing this module:
//!
//! - **Hermes** `reference/hermes-agent-main/tools/environments/local.py`
//!   (`subprocess.Popen(args, cwd=...)`, L522-534): commands run as an **argv array**
//!   with a validated working directory — never a shell string. We mirror it: the
//!   program + args are individual `argv` elements (re-validated by
//!   [`crate::mcp::validate_stdio_command`] — no shell metacharacters, no danger
//!   flags), and a `cwd` is confined to a safe root.
//! - **openclaw** `src/agents/bash-tools.exec-types.ts` (`commandArgv?: string[]`
//!   distinct from a display `command` string): execution binds an argv array, never a
//!   re-parsed string. We keep ONLY the argv form (there is no shell-string form to
//!   misinterpret).
//! - **openclaw** `src/infra/exec-approvals.ts` (`SystemRunApprovalBinding` binds
//!   `{argv, cwd}`): the unit of approval is a concrete `(argv, cwd)`. Here the stored
//!   config IS that binding; invocation appends only validated, declared input args.
//!
//! ## The safety contract (binding)
//!
//! - **argv only, never a shell.** `program` + `args` are validated with the SAME
//!   [`crate::mcp::validate_stdio_command`] the managed-stdio MCP transport uses (a
//!   single bounded program token with no shell metacharacters, bounded control-char
//!   free args, no `--yolo`/bypass danger flag). There is no shell-string field, so
//!   there is no metacharacter-injection surface.
//! - **cwd is confined.** A `cwd`, when set, is a relative path validated for SHAPE
//!   here (non-empty, bounded, no control char, no `..` traversal) and re-validated at
//!   spawn time by the kernel to canonicalize INSIDE the plugin's install directory
//!   (blocking a symlink escape). `None` ⇒ the install directory root.
//! - **Optional input args are positional + validated.** A caller may supply values
//!   for the declared [`CommandInputArg`]s; each value is bounded, control-char free,
//!   and appended as a SINGLE `argv` element in declared order — never shell-split,
//!   never able to introduce a new flag the operator did not declare.
//! - **No secrets.** The config carries no env/secret material. (Env injection is a
//!   future extension; v1 inherits the parent environment only.)
//!
//! This module performs NO I/O and holds no kernel state; it is the pure data model +
//! validators the kernel calls before storing a config or spawning a process.

use serde::{Deserialize, Serialize};

use crate::mcp::{validate_stdio_command, validate_stdio_cwd_shape, McpConfigError};

/// Default per-invocation timeout for a governed command tool (ms). A command tool
/// can legitimately run longer than an MCP request (a build, a batch job), so the
/// ceiling is higher than the MCP timeout — but still bounded.
pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 30_000;
/// Smallest accepted command-tool timeout (ms).
pub const MIN_COMMAND_TIMEOUT_MS: u64 = 500;
/// Largest accepted command-tool timeout (ms) — 5 minutes, matching the plugin
/// manifest's `invoke_timeout_secs` ceiling (`docs/plugins.md`).
pub const MAX_COMMAND_TIMEOUT_MS: u64 = 300_000;

/// Most declared input args one command tool may carry.
pub const MAX_COMMAND_INPUT_ARGS: usize = 16;
/// Max characters of one caller-supplied input-arg VALUE (bounded so a hostile caller
/// cannot append a megabyte argv element).
pub const MAX_COMMAND_INPUT_VALUE_CHARS: usize = 4096;

/// One declared, optional input argument a caller may supply at invocation time. The
/// value is appended to the command's fixed args as a single `argv` element (in the
/// order the inputs are declared) — positional, never a flag, never shell-split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInputArg {
    /// The key the caller supplies in the invocation `input` JSON object. A safe
    /// POSIX-style identifier ([`crate::is_valid_env_var_name`]) so it can never carry
    /// an injection and reads cleanly in the form/schema.
    pub name: String,
    /// A short human description shown in the tool's input schema.
    #[serde(default)]
    pub description: String,
    /// Whether the caller MUST supply this arg (a missing required arg fails the
    /// invocation, fail-closed, before anything spawns).
    #[serde(default)]
    pub required: bool,
}

/// A durable, operator-configured governed command tool.
///
/// Persisted locally alongside the rest of the control plane, keyed by its owning
/// `(plugin_id, tool_name)`. Carries no secrets. The matching manifest
/// [`crate::ToolDefinition`] (added at configure time) is what surfaces it in the
/// Tools list and gates it; this is the execution recipe the kernel runs argv-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandToolConfig {
    /// The installed plugin this command tool belongs to.
    pub plugin_id: String,
    /// The manifest tool name this config backs (e.g. `repo.build`).
    pub tool_name: String,
    /// The program to run — `argv[0]`. A single bounded token (a launcher such as
    /// `node` / `python` / `cargo` / `cmd`, or an explicit path), never a shell line.
    pub program: String,
    /// Fixed args, each a single `argv` element passed verbatim (never shell-split).
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional working directory, relative to (and confined within) the plugin's
    /// install directory. `None` ⇒ the install directory root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Declared optional input args appended (validated) at invocation time.
    #[serde(default)]
    pub input_args: Vec<CommandInputArg>,
    /// Per-invocation timeout in milliseconds (already clamped by [`clamp_command_timeout`]).
    pub timeout_ms: u64,
    /// Whether this command tool is enabled. A disabled config is honestly refused at
    /// invocation (not silently run).
    pub enabled: bool,
}

/// Why a governed command tool config / invocation was refused. Fail-closed: a config
/// that fails validation is never stored, and an invocation that fails arg-building is
/// never spawned.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandToolError {
    /// The program / fixed args failed the argv safety contract (no shell, no danger
    /// flag, bounded). Wraps the shared [`McpConfigError`] so the messages match the
    /// managed-stdio transport's.
    #[error("invalid command: {0}")]
    InvalidCommand(#[from] McpConfigError),
    /// More declared input args than [`MAX_COMMAND_INPUT_ARGS`].
    #[error("too many input args (max {MAX_COMMAND_INPUT_ARGS})")]
    TooManyInputArgs,
    /// A declared input-arg name is not a safe identifier.
    #[error("input arg name '{0}' is not a valid identifier (letters, digits, '_')")]
    InvalidInputArgName(String),
    /// Two declared input args share a name.
    #[error("duplicate input arg name '{0}'")]
    DuplicateInputArgName(String),
    /// A required input arg was not supplied at invocation time.
    #[error("missing required input arg '{0}'")]
    MissingRequiredInput(String),
    /// A supplied input-arg value was not a JSON string.
    #[error("input arg '{0}' must be a string")]
    InputNotAString(String),
    /// A supplied input-arg value was too long.
    #[error("input arg '{0}' is too long (max {MAX_COMMAND_INPUT_VALUE_CHARS} chars)")]
    InputTooLong(String),
    /// A supplied input-arg value carried a control character.
    #[error("input arg '{0}' must not contain control characters")]
    InputHasControlChar(String),
}

/// Clamp a requested timeout (ms) into the accepted command-tool range.
pub fn clamp_command_timeout(timeout_ms: u64) -> u64 {
    timeout_ms.clamp(MIN_COMMAND_TIMEOUT_MS, MAX_COMMAND_TIMEOUT_MS)
}

/// Validate a [`CommandToolConfig`] against the safety contract (SHAPE only; the deep
/// cwd-containment check is the kernel's, at spawn time). Fail-closed: a failure means
/// the config is never stored.
///
/// - `program` + `args` pass [`validate_stdio_command`] (argv-only, no shell
///   metacharacters in the program token, bounded control-char-free args, no danger
///   flag);
/// - a `cwd`, when present, passes [`validate_stdio_cwd_shape`] (non-empty, bounded,
///   no control char, no `..` traversal);
/// - `input_args` are bounded in count, each a valid identifier, and uniquely named.
pub fn validate_command_tool_config(config: &CommandToolConfig) -> Result<(), CommandToolError> {
    validate_stdio_command(&config.program, &config.args)?;
    if let Some(cwd) = &config.cwd {
        validate_stdio_cwd_shape(cwd)?;
    }
    if config.input_args.len() > MAX_COMMAND_INPUT_ARGS {
        return Err(CommandToolError::TooManyInputArgs);
    }
    let mut seen: Vec<&str> = Vec::with_capacity(config.input_args.len());
    for arg in &config.input_args {
        if !crate::is_valid_env_var_name(&arg.name) {
            return Err(CommandToolError::InvalidInputArgName(arg.name.clone()));
        }
        if seen.contains(&arg.name.as_str()) {
            return Err(CommandToolError::DuplicateInputArgName(arg.name.clone()));
        }
        seen.push(&arg.name);
    }
    Ok(())
}

/// Build the full argv tail (fixed args + validated declared input args) for ONE
/// invocation. Does NOT include `argv[0]` (the program) — the caller spawns
/// `config.program` with this as its args.
///
/// `input` is the caller's invocation JSON. For each declared [`CommandInputArg`], in
/// declared order: a present value must be a bounded, control-char-free string and is
/// appended as a single argv element; a missing REQUIRED arg fails closed; a missing
/// optional arg is skipped. Any key in `input` that is not a declared input arg is
/// IGNORED (it can never reach argv), so a caller can never smuggle an extra flag.
pub fn build_command_argv(
    config: &CommandToolConfig,
    input: &serde_json::Value,
) -> Result<Vec<String>, CommandToolError> {
    // Re-validate the stored command on every build (defense in depth).
    validate_stdio_command(&config.program, &config.args)?;
    let obj = input.as_object();
    let mut argv: Vec<String> = config.args.clone();
    for declared in &config.input_args {
        match obj.and_then(|o| o.get(&declared.name)) {
            None | Some(serde_json::Value::Null) => {
                if declared.required {
                    return Err(CommandToolError::MissingRequiredInput(declared.name.clone()));
                }
            }
            Some(serde_json::Value::String(s)) => {
                if s.chars().count() > MAX_COMMAND_INPUT_VALUE_CHARS {
                    return Err(CommandToolError::InputTooLong(declared.name.clone()));
                }
                if s.chars().any(|c| c.is_control()) {
                    return Err(CommandToolError::InputHasControlChar(declared.name.clone()));
                }
                argv.push(s.clone());
            }
            Some(_) => return Err(CommandToolError::InputNotAString(declared.name.clone())),
        }
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(program: &str, args: &[&str]) -> CommandToolConfig {
        CommandToolConfig {
            plugin_id: "relux-plugin-x".to_string(),
            tool_name: "repo.run".to_string(),
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            input_args: Vec::new(),
            timeout_ms: DEFAULT_COMMAND_TIMEOUT_MS,
            enabled: true,
        }
    }

    #[test]
    fn a_clean_argv_config_validates() {
        assert!(validate_command_tool_config(&cfg("node", &["./dist/server.js"])).is_ok());
    }

    #[test]
    fn a_shell_metacharacter_in_the_program_is_refused() {
        // The whole point: no shell-string program. A metacharacter signals a pasted
        // shell line and is rejected (mirrors the managed-stdio transport).
        let c = cfg("rm -rf / && curl evil", &[]);
        assert!(matches!(
            validate_command_tool_config(&c),
            Err(CommandToolError::InvalidCommand(_))
        ));
    }

    #[test]
    fn a_danger_flag_arg_is_refused() {
        let c = cfg("claude", &["--dangerously-skip-permissions"]);
        assert!(matches!(
            validate_command_tool_config(&c),
            Err(CommandToolError::InvalidCommand(_))
        ));
    }

    #[test]
    fn a_parent_traversal_cwd_is_refused() {
        let mut c = cfg("node", &["x.js"]);
        c.cwd = Some("../../etc".to_string());
        assert!(matches!(
            validate_command_tool_config(&c),
            Err(CommandToolError::InvalidCommand(_))
        ));
    }

    #[test]
    fn input_args_are_bounded_named_and_unique() {
        let mut c = cfg("node", &["run.js"]);
        c.input_args = vec![
            CommandInputArg { name: "FILE".into(), description: String::new(), required: true },
            CommandInputArg { name: "FILE".into(), description: String::new(), required: false },
        ];
        assert!(matches!(
            validate_command_tool_config(&c),
            Err(CommandToolError::DuplicateInputArgName(_))
        ));

        c.input_args = vec![CommandInputArg {
            name: "not a name".into(),
            description: String::new(),
            required: false,
        }];
        assert!(matches!(
            validate_command_tool_config(&c),
            Err(CommandToolError::InvalidInputArgName(_))
        ));
    }

    #[test]
    fn build_argv_appends_only_declared_inputs_in_order() {
        let mut c = cfg("node", &["run.js", "--mode", "fast"]);
        c.input_args = vec![
            CommandInputArg { name: "FILE".into(), description: String::new(), required: true },
            CommandInputArg { name: "TAG".into(), description: String::new(), required: false },
        ];
        let input = serde_json::json!({ "FILE": "report.csv", "TAG": "v2", "EXTRA": "ignored" });
        let argv = build_command_argv(&c, &input).unwrap();
        // Fixed args first, then declared inputs in declared order. EXTRA is dropped.
        assert_eq!(argv, vec!["run.js", "--mode", "fast", "report.csv", "v2"]);
    }

    #[test]
    fn build_argv_fails_closed_on_a_missing_required_input() {
        let mut c = cfg("node", &["run.js"]);
        c.input_args = vec![CommandInputArg {
            name: "FILE".into(),
            description: String::new(),
            required: true,
        }];
        let err = build_command_argv(&c, &serde_json::json!({})).unwrap_err();
        assert!(matches!(err, CommandToolError::MissingRequiredInput(n) if n == "FILE"));
    }

    #[test]
    fn build_argv_rejects_a_non_string_or_control_char_value() {
        let mut c = cfg("node", &["run.js"]);
        c.input_args = vec![CommandInputArg {
            name: "FILE".into(),
            description: String::new(),
            required: false,
        }];
        assert!(matches!(
            build_command_argv(&c, &serde_json::json!({ "FILE": 7 })),
            Err(CommandToolError::InputNotAString(_))
        ));
        assert!(matches!(
            build_command_argv(&c, &serde_json::json!({ "FILE": "a\nb" })),
            Err(CommandToolError::InputHasControlChar(_))
        ));
    }

    #[test]
    fn an_optional_missing_input_is_skipped_not_appended() {
        let mut c = cfg("node", &["run.js"]);
        c.input_args = vec![CommandInputArg {
            name: "TAG".into(),
            description: String::new(),
            required: false,
        }];
        let argv = build_command_argv(&c, &serde_json::json!({})).unwrap();
        assert_eq!(argv, vec!["run.js"]);
    }

    #[test]
    fn timeout_clamps_into_range() {
        assert_eq!(clamp_command_timeout(0), MIN_COMMAND_TIMEOUT_MS);
        assert_eq!(clamp_command_timeout(10_000_000), MAX_COMMAND_TIMEOUT_MS);
        assert_eq!(clamp_command_timeout(45_000), 45_000);
    }
}
