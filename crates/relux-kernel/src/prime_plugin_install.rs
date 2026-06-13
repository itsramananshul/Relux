//! Parse a free-form Prime chat request into a structured GitHub plugin-install.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` §8 (Plugin Model), §10.1 (Intent Layer),
//! §10.2 (Action Layer); `docs/plugins.md` (GitHub import); `docs/prime-tool-use.md`
//! ("Importing a plugin from GitHub").
//!
//! ## Why this exists
//!
//! "Clone nousresearch/hermes-agent and import it as a plugin" used to be misread as
//! a generic work task or a free-form local-prime run that got stuck/blocked. That is
//! a plugin-import intent: Prime should recognize it, parse the repo, and stage the
//! SAFE manifestless install behind a human confirmation — then surface what was
//! installed and the detected capability candidates. This module is the deterministic
//! parser that turns the natural-language message into a canonical, credential-free
//! GitHub reference; everything downstream (the approval gate, the existing
//! `install_from_github` clone, the `/hints` candidate scan) is unchanged.
//!
//! ## Safety by construction
//!
//! - The output `repo_url` is ALWAYS rebuilt as `https://github.com/<owner>/<repo>`
//!   from the parsed owner/repo, so anything before `github.com/` (including embedded
//!   `user:token@` credentials) is dropped — the parser can never carry a credential
//!   or rewrite a scheme into an accepted one. The kernel's authoritative
//!   `validate_github_url` stays the real gate at install time.
//! - GitHub-only: a non-GitHub host, an ssh/git URL, or junk yields `None` (Prime then
//!   asks the operator to clarify the repo) — never a silent guess.
//! - Pure and side-effect-free: it clones nothing and reads no filesystem.
//!
//! Reference: this mirrors Hermes `hermes_cli/plugins_cmd.py::_resolve_git_url`
//! (owner/repo shorthand or full URL → cloneable URL, GitHub default) and openclaw's
//! single-classifier discipline (`reference/openclaw-main/src/acp/approval-classifier.ts`):
//! one deterministic function decides, and the risky path is always confirmation-gated.

/// A validated GitHub plugin-install request parsed out of a chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGithubPluginInstall {
    /// Canonical, credential-free clone URL: `https://github.com/<owner>/<repo>`.
    pub repo_url: String,
    /// The repository owner (org/user) segment.
    pub owner: String,
    /// The repository name segment (without a `.git` suffix).
    pub repo: String,
    /// The PROPOSED local plugin id (`relux-plugin-<sanitized repo>`). Advisory: the
    /// installer finalizes the real id from the repo's manifest, or scaffolds this
    /// exact shape when the source has no `relux-plugin.json`.
    pub proposed_plugin_id: String,
}

/// True when a name segment is a legal GitHub owner/repo fragment: starts with an
/// ASCII alphanumeric and otherwise contains only `A-Za-z0-9._-`. Mirrors the
/// dashboard's `normalizeGithubUrl` shorthand pattern so chat and the Plugins page
/// agree on what `owner/repo` means.
fn is_name_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    seg.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Strip surrounding punctuation/quotes a chat token may carry (`"owner/repo",`,
/// `(https://github.com/o/r)`, a trailing `.`/`/`) without touching inner characters.
fn trim_token(tok: &str) -> &str {
    tok.trim_matches(|c: char| {
        matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '!' | '?' | '<' | '>')
    })
    .trim_end_matches('.')
    .trim_end_matches('/')
}

/// Build the parsed result from a validated owner/repo pair.
fn build(owner: &str, repo: &str) -> ParsedGithubPluginInstall {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let repo_url = format!("https://github.com/{owner}/{repo}");
    let proposed_plugin_id = format!(
        "relux-plugin-{}",
        crate::plugin_install::sanitize_seed(repo)
    );
    ParsedGithubPluginInstall {
        repo_url,
        owner: owner.to_string(),
        repo: repo.to_string(),
        proposed_plugin_id,
    }
}

/// Extract the owner/repo from a token that contains `github.com/...`, ignoring
/// anything before the host (scheme, `www.`, credentials) and after the repo
/// (`/tree/main`, `.git`, query, fragment). Returns `None` if two path segments
/// can't be read or either is not a legal name.
fn parse_github_url_token(tok: &str) -> Option<ParsedGithubPluginInstall> {
    // Locate the host anchor; everything before it (scheme + optional creds) is dropped.
    let lower = tok.to_ascii_lowercase();
    let idx = lower.find("github.com/")?;
    let after = &tok[idx + "github.com/".len()..];
    // Split off any query/fragment, then take the first two path segments.
    let path = after.split(['?', '#']).next().unwrap_or(after);
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let owner = segs.next()?;
    let repo = segs.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if is_name_segment(owner) && is_name_segment(repo) {
        Some(build(owner, repo))
    } else {
        None
    }
}

/// Treat a token as the bare `owner/repo[.git]` shorthand: EXACTLY two slash-separated
/// legal name segments and nothing else (no scheme, no extra slashes, no `@`).
fn parse_shorthand_token(tok: &str) -> Option<ParsedGithubPluginInstall> {
    if tok.contains("://") || tok.contains('@') || tok.contains('\\') {
        return None;
    }
    let parts: Vec<&str> = tok.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let owner = parts[0];
    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if is_name_segment(owner) && is_name_segment(repo) {
        Some(build(owner, repo))
    } else {
        None
    }
}

/// Parse a free-form chat message into a [`ParsedGithubPluginInstall`], or `None`
/// when no GitHub repo reference is present.
///
/// A full `github.com/...` URL anywhere in the message wins over a bare shorthand
/// (so "import https://github.com/owner/repo, not foo/bar" imports the URL). When no
/// URL is present, the first bare `owner/repo` token is used. Pure; reads nothing.
pub fn parse_github_plugin_install(message: &str) -> Option<ParsedGithubPluginInstall> {
    let tokens: Vec<&str> = message.split_whitespace().map(trim_token).collect();
    // Prefer an explicit github.com URL token.
    if let Some(found) = tokens
        .iter()
        .find_map(|t| parse_github_url_token(t))
    {
        return Some(found);
    }
    // Otherwise the first bare owner/repo shorthand.
    tokens.iter().find_map(|t| parse_shorthand_token(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo_shorthand() {
        let p = parse_github_plugin_install("install nousresearch/hermes-agent as a plugin")
            .expect("shorthand parses");
        assert_eq!(p.repo_url, "https://github.com/nousresearch/hermes-agent");
        assert_eq!(p.owner, "nousresearch");
        assert_eq!(p.repo, "hermes-agent");
        assert_eq!(p.proposed_plugin_id, "relux-plugin-hermes-agent");
    }

    #[test]
    fn parses_full_https_url() {
        let p = parse_github_plugin_install("import https://github.com/owner/repo as plugin")
            .expect("url parses");
        assert_eq!(p.repo_url, "https://github.com/owner/repo");
        assert_eq!(p.proposed_plugin_id, "relux-plugin-repo");
    }

    #[test]
    fn parses_clone_phrasing_with_git_suffix() {
        let p = parse_github_plugin_install(
            "clone nousresearch/hermes-agent.git and import it as a plugin",
        )
        .expect("clone phrasing parses");
        assert_eq!(p.repo_url, "https://github.com/nousresearch/hermes-agent");
        assert_eq!(p.repo, "hermes-agent");
    }

    #[test]
    fn url_with_extra_path_and_query_takes_owner_repo() {
        let p = parse_github_plugin_install(
            "import https://github.com/owner/repo/tree/main?foo=bar as a plugin",
        )
        .expect("deep url parses");
        assert_eq!(p.repo_url, "https://github.com/owner/repo");
    }

    #[test]
    fn drops_embedded_credentials() {
        // Anything before github.com/ is dropped: the canonical URL is credential-free.
        let p = parse_github_plugin_install(
            "install https://user:token@github.com/owner/repo as a plugin",
        )
        .expect("credentialed url is sanitized, not rejected");
        assert_eq!(p.repo_url, "https://github.com/owner/repo");
        assert!(!p.repo_url.contains('@'));
        assert!(!p.repo_url.contains("token"));
    }

    #[test]
    fn url_wins_over_shorthand() {
        let p = parse_github_plugin_install(
            "import https://github.com/real/one not other/two as plugin",
        )
        .expect("url present");
        assert_eq!(p.repo_url, "https://github.com/real/one");
    }

    #[test]
    fn non_github_url_is_not_parsed() {
        assert!(parse_github_plugin_install("install https://gitlab.com/owner/repo").is_none());
    }

    #[test]
    fn casual_mention_without_repo_is_none() {
        assert!(parse_github_plugin_install("what if I made a plugin system?").is_none());
        assert!(parse_github_plugin_install("install the plugin relux-tools-echo").is_none());
        assert!(parse_github_plugin_install("can you add a plugin for me").is_none());
    }

    #[test]
    fn three_segment_path_is_not_shorthand() {
        // A bare `a/b/c` is not owner/repo (too many segments) and must not match.
        assert!(parse_github_plugin_install("install a/b/c").is_none());
    }

    #[test]
    fn windows_path_is_not_shorthand() {
        assert!(parse_github_plugin_install("install C:\\repos\\thing as plugin").is_none());
    }
}
