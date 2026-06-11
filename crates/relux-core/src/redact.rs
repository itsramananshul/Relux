//! Best-effort secret redaction for captured process output.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 17.5 (permissions/safety) and
//! the product safety bar for the adapter runtime: when Relux spawns a local CLI
//! and captures its stdout/stderr into a run transcript, it scrubs obvious
//! key-shaped tokens first so a leaked credential is not persisted verbatim.
//!
//! This is a conservative, dependency-free scrubber, NOT a security boundary. It
//! masks well-known token prefixes (OpenAI/Anthropic `sk-...`, GitHub `ghp_...`,
//! Slack `xox...`, AWS `AKIA...`, Google `AIza.../ya29....`, GitLab `glpat-...`)
//! and `key=value` / `key: value` pairs whose key names a secret. It is meant to
//! reduce accidental credential persistence, not to guarantee none slips through.

/// The text substituted in place of a redacted secret.
pub const REDACTION_PLACEHOLDER: &str = "***REDACTED***";

/// Known high-signal secret token prefixes. A whitespace-delimited token that
/// starts with one of these (and is long enough) is masked.
const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-",
    "sk-",
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "xapp-",
    "glpat-",
    "ya29.",
    "AKIA",
    "ASIA",
    "AIza",
    // Relux per-agent access token (crate::agent_auth). Defence-in-depth: a token
    // should never reach a transcript/log, but mask its prefix if one ever does.
    "relux_agt_",
];

/// Key-name fragments that mark the right-hand side of a `key=value` /
/// `key: value` pair as a secret to mask.
const SECRET_KEY_FRAGMENTS: &[&str] =
    &["key", "token", "secret", "password", "passwd", "auth", "credential"];

/// The wrapper characters stripped from a token before pattern-matching and
/// re-applied around the placeholder so surrounding quotes/punctuation survive.
const WRAPPERS: &[char] = &[
    '"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', ';', '.', '<', '>',
];

/// Redact obvious secrets from `input`, preserving whitespace and structure.
pub fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for (i, line) in input.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    out
}

/// Redact one line, masking each whitespace-delimited word in place while
/// keeping the original inter-word whitespace. A one-word lookback handles the
/// split `key: value` form (e.g. JSON `"auth_token": "secret"`), where the key
/// and value are separate words.
fn redact_line(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut word = String::new();
    // Set when the previous word was a secret key awaiting its value word.
    let mut value_pending = false;
    let flush = |word: &str, value_pending: &mut bool, result: &mut String| {
        if *value_pending {
            result.push_str(&redact_pending_value(word));
            *value_pending = false;
        } else {
            result.push_str(&redact_word(word));
        }
        *value_pending = is_secret_key_marker(word);
    };
    for ch in line.chars() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                flush(&word, &mut value_pending, &mut result);
                word.clear();
            }
            result.push(ch);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        flush(&word, &mut value_pending, &mut result);
    }
    result
}

/// Redact a single whitespace-delimited word: first the `key=value`/`key: value`
/// form, then a bare secret token.
fn redact_word(word: &str) -> String {
    if let Some(redacted) = redact_key_value(word) {
        return redacted;
    }
    let (lead, core, trail) = strip_wrappers(word);
    if looks_like_secret_token(core) {
        format!("{lead}{REDACTION_PLACEHOLDER}{trail}")
    } else {
        word.to_string()
    }
}

/// True when `word` is a bare secret key awaiting a value on the next word, e.g.
/// `"auth_token":` or `password=`. The trailing separator is what distinguishes
/// a key marker from a mere mention of the word in prose.
fn is_secret_key_marker(word: &str) -> bool {
    let key = match word.strip_suffix([':', '=']) {
        Some(k) => k,
        None => return false,
    };
    let (_, core, _) = strip_wrappers(key);
    let norm = core.to_lowercase();
    !norm.is_empty()
        && SECRET_KEY_FRAGMENTS
            .iter()
            .any(|frag| norm.contains(frag))
}

/// Redact the value word that follows a secret key marker. The key already told
/// us this is a credential, so any sufficiently long value core is masked
/// (regardless of prefix), with wrappers preserved.
fn redact_pending_value(word: &str) -> String {
    let (lead, core, trail) = strip_wrappers(word);
    if core.len() >= 6 {
        format!("{lead}{REDACTION_PLACEHOLDER}{trail}")
    } else {
        word.to_string()
    }
}

/// Mask the value of a `key=value` or `key: value` token whose key names a
/// secret. Returns `None` when the word is not such a pair.
fn redact_key_value(word: &str) -> Option<String> {
    for sep in ['=', ':'] {
        if let Some(idx) = word.find(sep) {
            let (key_raw, rest) = word.split_at(idx);
            let value = &rest[sep.len_utf8()..];
            // Skip URL-ish `scheme://...` (the ':' belongs to the scheme, the
            // value would start with '//') and empty values.
            if value.starts_with('/') || value.is_empty() {
                continue;
            }
            let key_norm: String = key_raw
                .trim_matches(|c: char| WRAPPERS.contains(&c) || c.is_whitespace())
                .to_lowercase();
            if key_norm.is_empty() {
                continue;
            }
            let names_secret = SECRET_KEY_FRAGMENTS
                .iter()
                .any(|frag| key_norm.contains(frag));
            // Only redact a value with enough length to plausibly be a credential.
            let (vlead, vcore, vtrail) = strip_wrappers(value);
            if names_secret && vcore.len() >= 6 {
                return Some(format!(
                    "{key_raw}{sep}{vlead}{REDACTION_PLACEHOLDER}{vtrail}"
                ));
            }
        }
    }
    None
}

/// True when a bare token looks like a known credential.
fn looks_like_secret_token(token: &str) -> bool {
    if token.len() < 12 {
        // Every supported prefix yields a token well over this length when a real
        // secret body is present; this keeps short words (e.g. "sk-1") safe.
        return false;
    }
    SECRET_PREFIXES.iter().any(|p| token.starts_with(p))
}

/// Split a token into `(leading wrappers, core, trailing wrappers)` so the core
/// can be matched while quotes/punctuation are preserved around the placeholder.
fn strip_wrappers(token: &str) -> (&str, &str, &str) {
    let trimmed_start = token.trim_start_matches(WRAPPERS);
    let lead = &token[..token.len() - trimmed_start.len()];
    let core = trimmed_start.trim_end_matches(WRAPPERS);
    let trail = &trimmed_start[core.len()..];
    (lead, core, trail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a realistic-looking token at runtime so no contiguous key-shaped
    /// literal appears in source (keeps secret scanners and reviewers calm).
    fn token(prefix: &str, body: &str) -> String {
        format!("{prefix}{body}")
    }

    #[test]
    fn masks_known_prefixes() {
        let sk = token("sk-ant-", "0123456789abcdef0123");
        let gh = token("ghp_", "0123456789abcdef0123");
        let input = format!("using {sk} and {gh} now");
        let out = redact_secrets(&input);
        assert!(!out.contains(&sk), "anthropic token leaked: {out}");
        assert!(!out.contains(&gh), "github token leaked: {out}");
        assert_eq!(out.matches(REDACTION_PLACEHOLDER).count(), 2);
        assert!(out.starts_with("using "));
        assert!(out.ends_with(" now"));
    }

    #[test]
    fn masks_key_value_pairs() {
        let secret = token("", "supersecretvalue123");
        let line = format!("API_KEY={secret}");
        let out = redact_secrets(&line);
        assert!(out.starts_with("API_KEY="));
        assert!(!out.contains(&secret));
        assert!(out.contains(REDACTION_PLACEHOLDER));

        let json = format!("\"auth_token\": \"{secret}\"");
        let out = redact_secrets(&json);
        assert!(!out.contains(&secret), "json secret leaked: {out}");
        assert!(out.contains(REDACTION_PLACEHOLDER));
    }

    #[test]
    fn preserves_wrappers_around_token() {
        let body = token("sk-", "0123456789abcdef0123");
        let quoted = format!("(\"{body}\")");
        let out = redact_secrets(&quoted);
        assert!(out.starts_with("(\""));
        assert!(out.ends_with("\")"));
        assert!(out.contains(REDACTION_PLACEHOLDER));
    }

    #[test]
    fn masks_relux_agent_token_prefix() {
        // A leaked per-agent access token (crate::agent_auth) is masked by its prefix.
        let agt = token("relux_agt_", "0123456789abcdef0123456789abcdef");
        let input = format!("Authorization: Bearer {agt}");
        let out = redact_secrets(&input);
        assert!(!out.contains(&agt), "agent token leaked: {out}");
        assert!(out.contains(REDACTION_PLACEHOLDER));
    }

    #[test]
    fn leaves_ordinary_text_untouched() {
        let input = "Ran 3 tests, 0 failures. Updated README.md and src/main.rs.";
        assert_eq!(redact_secrets(input), input);
        // A URL with a port must not be treated as a key:value secret.
        let url = "Listening on http://127.0.0.1:8080/healthz";
        assert_eq!(redact_secrets(url), url);
    }

    #[test]
    fn preserves_newlines_and_layout() {
        let body = token("sk-", "0123456789abcdef0123");
        let input = format!("line one\nkey = {body}\nline three");
        let out = redact_secrets(&input);
        assert_eq!(out.lines().count(), 3);
        assert!(out.contains("line one"));
        assert!(out.contains("line three"));
        assert!(!out.contains(&body));
    }
}
