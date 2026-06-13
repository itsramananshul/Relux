//! Prime-guided configuration of a governed **command tool** from operator-supplied
//! fields, for an installed plugin that has **no detected runnable candidate**.
//!
//! [`crate::capability_detect::detect_candidates`] turns an imported source into a
//! bounded list of candidates. A repo that ships no `relux-plugin.json` and declares no
//! MCP server / `bin` / console-script / Cargo binary yields an honest `manual`
//! candidate with **no one-click activation** — Relux refuses to infer a command from
//! arbitrary repo content. [`crate::prime_candidate_config`] handles the *detected*
//! case; this module handles the *from-scratch* one: the user names the command
//! themselves ("use npm test from this plugin", "configure this repo as a tool that runs
//! npm test"), and Prime stages that as a reviewable [`crate::PrimeAction::ConfigureCommandTool`]
//! proposal — always behind a human approval.
//!
//! This is a **fallback rail only** (`docs/reference-driven-development.md`, BINDING:
//! keyword rules are never the primary brain). The pure text parser here extracts the
//! plugin selector + the argv recipe; the confirm route re-validates the whole recipe
//! through the UNCHANGED command-tool validator (argv-only, no shell, no danger flag,
//! confined `cwd`, approval always Required) before anything is stored, and nothing runs
//! at configuration time.
//!
//! ## Hard invariants
//!
//! - **Executes nothing.** Parsing only reshapes the user's words into a draft. The
//!   actual configuration flows through [`crate::parse_command_tool_input`] +
//!   [`crate::KernelState::configure_command_tool`], which re-validate the argv safety
//!   contract; the resulting tool stays gated until invoked.
//! - **Never fabricates a command.** When no concrete program can be extracted the parser
//!   returns `None`, so the caller falls through to the detected-candidate path or asks a
//!   clarifying question — Prime never guesses a command from repo content.
//!
//! Reference grounding (`docs/reference-driven-development.md`, BINDING):
//! `reference/hermes-agent-main/hermes_cli/mcp_config.py` (`cmd_mcp_add` — key a
//! `{command,args,env}` server by name; configuration is a separate, confirmed step from
//! running it). We rebuild a `{program,args}` recipe from the user's words and hand it to
//! the existing command-tool validation, never spawning on configuration.

/// A parsed chat request to configure a governed command tool from scratch: a fuzzy
/// plugin selector (re-resolved server-side) and the operator's reviewed argv recipe.
/// Neither is trusted as a final command — the confirm route re-validates everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandToolConfigRequest {
    /// A fuzzy plugin reference parsed from the message ("from acme-tools", "for the repo"),
    /// or empty when none was named — the backend resolves the unique installed plugin.
    pub plugin_selector: String,
    /// A derived, sanitized tool name (e.g. `npm.test`, `cargo.build`). Advisory — the
    /// validator re-sanitizes it.
    pub tool_name: String,
    /// The program (argv[0]) the user named.
    pub program: String,
    /// The fixed args (the remaining tokens of the user's command).
    pub args: Vec<String>,
}

/// Parse a chat message into a [`CommandToolConfigRequest`], or `None` when the message
/// is not an explicit from-scratch command-tool request OR no concrete command could be
/// extracted. Pure text — reads no state and runs nothing.
///
/// Recognised shapes (the user supplies the command themselves):
///   - "configure this repo as a tool that runs npm test"
///   - "make a command tool that runs cargo build for acme-tools"
///   - "add a command tool running ./scripts/serve.sh"
///   - "use npm test from this plugin"
///
/// The caller has already routed the message to
/// [`relux_core::PrimeIntent::PluginConfiguration`]; this only extracts the recipe. A
/// message that names no command (e.g. "configure the command tool from acme-tools",
/// which references a DETECTED candidate) returns `None` so the caller falls through to
/// [`crate::prime_candidate_config::parse_candidate_config_request`].
pub fn parse_command_tool_config_request(message: &str) -> Option<CommandToolConfigRequest> {
    let original = message.trim();
    if original.is_empty() {
        return None;
    }
    let m = original.to_lowercase();

    // Find the command phrase. An explicit "runs/running/that runs <cmd>" cue wins; else
    // a "use <cmd> from/for <plugin>" shape. We work on the ORIGINAL-case message so the
    // extracted program/args keep their case (a path/flag is case-sensitive).
    let command_phrase = extract_command_phrase(original, &m)?;
    // Split off a trailing "from/for <plugin>" reference so it never leaks into the argv.
    let (command_text, plugin_selector) = split_plugin_reference(&command_phrase);

    let mut tokens = command_text.split_whitespace();
    let program = tokens.next()?.to_string();
    // A bare "it"/"this"/"that" is not a program (the user said "use it from the plugin"
    // with no actual command) — refuse rather than fabricate.
    if is_filler_program(&program) {
        return None;
    }
    let args: Vec<String> = tokens.map(|s| s.to_string()).collect();

    let tool_name = derive_tool_name(&program, &args);

    Some(CommandToolConfigRequest {
        plugin_selector,
        tool_name,
        program,
        args,
    })
}

/// Extract the literal command phrase from the message, preserving original case. Returns
/// the substring AFTER the strongest command cue, or `None` when no cue is present.
fn extract_command_phrase(original: &str, lower: &str) -> Option<String> {
    // A leading "use "/"run "/"runs " is a command cue at the very start ("use npm test
    // from this plugin"); a filler program ("use it …") is rejected downstream.
    const LEADING: &[&str] = &["use ", "run ", "runs "];
    for cue in LEADING {
        if let Some(rest) = lower.strip_prefix(cue) {
            if !rest.trim().is_empty() {
                return Some(original[cue.len()..].trim().to_string());
            }
        }
    }
    // Strongest first: an explicit "that run(s)/running" cue, then a bare "run(s)", then a
    // mid-sentence "use ". Each cue must be followed by a real command (caller-checked).
    const CUES: &[&str] = &[
        " that runs ",
        " that run ",
        " which runs ",
        " running ",
        " runs ",
        " run ",
        " use ",
    ];
    for cue in CUES {
        if let Some(idx) = lower.find(cue) {
            let start = idx + cue.len();
            if start >= original.len() {
                continue;
            }
            let phrase = original[start..].trim();
            if !phrase.is_empty() {
                return Some(phrase.to_string());
            }
        }
    }
    None
}

/// Split a command phrase into (command, plugin_selector) by cutting a trailing
/// "from/for/on/in <plugin>" reference. The plugin token is cleaned + filtered against a
/// small stop-word list (so "from this plugin" yields an empty selector, not "this").
fn split_plugin_reference(phrase: &str) -> (String, String) {
    const CUES: &[&str] = &[" from ", " for ", " on ", " in "];
    const STOPWORDS: &[&str] = &[
        "the", "that", "this", "it", "repo", "repository", "plugin", "source", "a", "my", "our",
    ];
    let lower = phrase.to_lowercase();
    // Pick the EARLIEST cue so everything after it is treated as the plugin reference.
    let mut best: Option<(usize, usize)> = None;
    for cue in CUES {
        if let Some(idx) = lower.find(cue) {
            match best {
                Some((b, _)) if b <= idx => {}
                _ => best = Some((idx, cue.len())),
            }
        }
    }
    if let Some((idx, len)) = best {
        let command = phrase[..idx].trim().to_string();
        let rest = &phrase[idx + len..];
        let raw = rest.split_whitespace().next().unwrap_or("");
        let token: String = raw
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
            .trim_end_matches("'s")
            .to_string();
        let selector = if token.is_empty() || STOPWORDS.contains(&token.to_lowercase().as_str()) {
            String::new()
        } else {
            token
        };
        // A command that became empty after the cut (e.g. the cue was the whole phrase)
        // is degenerate — keep the original phrase so the caller's program check rejects it.
        if command.is_empty() {
            return (phrase.trim().to_string(), String::new());
        }
        return (command, selector);
    }
    (phrase.trim().to_string(), String::new())
}

/// Whether a parsed "program" is actually a filler word (a pronoun or article), not a
/// real command. Refuses "use it from the plugin" / "configure a tool to run the build"
/// (no concrete command named) rather than fabricate a program from a stop-word.
fn is_filler_program(program: &str) -> bool {
    let cleaned = program
        .to_lowercase()
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string();
    if cleaned.is_empty() {
        return true;
    }
    matches!(
        cleaned.as_str(),
        "it" | "this"
            | "that"
            | "them"
            | "the"
            | "a"
            | "an"
            | "my"
            | "our"
            | "your"
            | "its"
            | "some"
            | "any"
    )
}

/// Derive a stable, readable tool-name suggestion from the program + first arg, e.g.
/// `npm test` ⇒ `npm.test`, `node ./cli.js` ⇒ `node.run`, `cargo build` ⇒ `cargo.build`.
/// The validator re-sanitizes this; it just needs to be a sensible default.
fn derive_tool_name(program: &str, args: &[String]) -> String {
    let base = sanitize_segment(file_stem(program));
    // The verb is the first word-like subcommand (e.g. `test`, `build`) — skip flags
    // (`--port`), paths (`./x`), and bare numbers (`8080`) so the name reads naturally.
    let verb = args
        .iter()
        .map(|a| a.trim())
        .find(|a| {
            !a.is_empty()
                && !a.starts_with('-')
                && !a.contains('/')
                && !a.contains('\\')
                && !a.contains('.')
                && a.chars().any(|c| c.is_ascii_alphabetic())
        })
        .map(sanitize_segment)
        .filter(|a| !a.is_empty())
        .unwrap_or_default();
    let verb = if verb.is_empty() { "run".to_string() } else { verb };
    if base.is_empty() {
        format!("tool.{verb}")
    } else {
        format!("{base}.{verb}")
    }
}

/// The file stem of a program token (`./scripts/serve.sh` ⇒ `serve`, `npm` ⇒ `npm`).
fn file_stem(program: &str) -> &str {
    let last = program.rsplit(['/', '\\']).next().unwrap_or(program);
    last.split('.').next().unwrap_or(last)
}

/// Lower-case + keep only `[a-z0-9-]`, collapsing anything else away. Bounded so a
/// pathological token never produces a huge name.
fn sanitize_segment(s: &str) -> String {
    let cleaned: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    cleaned.trim_matches('-').chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_configure_as_a_tool_that_runs() {
        let r = parse_command_tool_config_request(
            "configure this repo as a tool that runs npm test",
        )
        .unwrap();
        assert_eq!(r.program, "npm");
        assert_eq!(r.args, vec!["test".to_string()]);
        assert_eq!(r.plugin_selector, "", "'this repo' is not a plugin id");
        assert_eq!(r.tool_name, "npm.test");
    }

    #[test]
    fn parses_command_tool_that_runs_with_plugin() {
        let r = parse_command_tool_config_request(
            "make a command tool that runs cargo build for acme-tools",
        )
        .unwrap();
        assert_eq!(r.program, "cargo");
        assert_eq!(r.args, vec!["build".to_string()]);
        assert_eq!(r.plugin_selector, "acme-tools");
        assert_eq!(r.tool_name, "cargo.build");
    }

    #[test]
    fn parses_use_cmd_from_plugin() {
        let r =
            parse_command_tool_config_request("use npm test from this plugin").unwrap();
        assert_eq!(r.program, "npm");
        assert_eq!(r.args, vec!["test".to_string()]);
        assert_eq!(r.plugin_selector, "");
    }

    #[test]
    fn parses_a_script_path_keeping_case() {
        let r = parse_command_tool_config_request(
            "add a command tool running ./scripts/Serve.sh --port 8080",
        )
        .unwrap();
        assert_eq!(r.program, "./scripts/Serve.sh");
        assert_eq!(r.args, vec!["--port".to_string(), "8080".to_string()]);
        // file stem, lower-cased; the first non-flag/non-path arg is the verb, else "run".
        assert_eq!(r.tool_name, "serve.run");
    }

    #[test]
    fn refuses_when_no_concrete_command() {
        // References a DETECTED candidate, not a from-scratch command — the caller falls
        // through to the candidate path.
        assert!(parse_command_tool_config_request(
            "configure the command tool from acme-tools"
        )
        .is_none());
    }

    #[test]
    fn refuses_a_bare_pronoun_program() {
        assert!(parse_command_tool_config_request("use it from this plugin").is_none());
    }

    #[test]
    fn refuses_an_article_program() {
        // "run the build script" must not be parsed as program="the" — no concrete
        // command was named, so the caller falls through rather than fabricating one.
        assert!(parse_command_tool_config_request(
            "configure a tool to run the build script"
        )
        .is_none());
    }

    #[test]
    fn refuses_a_message_with_no_command_cue() {
        assert!(parse_command_tool_config_request("enable the mcp server").is_none());
    }

    #[test]
    fn plugin_reference_does_not_leak_into_argv() {
        let r = parse_command_tool_config_request(
            "configure a tool that runs ./run.sh for my-plugin",
        )
        .unwrap();
        assert_eq!(r.program, "./run.sh");
        assert!(r.args.is_empty(), "the 'for <plugin>' tail must not become an arg");
        assert_eq!(r.plugin_selector, "my-plugin");
    }
}
