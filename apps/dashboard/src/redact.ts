// Best-effort, dependency-free secret redaction for user-visible tool output in the chat.
//
// Redaction parity (docs/RELUX_MASTER_PLAN.md §11.1): Prime must never splash a credential into
// chat. The kernel already scrubs the shaped Plugin Lens result and the deterministic reply
// (crates/relux-core/src/redact.rs `redact_secrets` / `redact_json`, crates/relux-kernel
// `shape_result` / `natural_tool_reply`), but the dashboard is the LAST surface before a human
// reads the text — and an MCP/tool `structuredContent` can still reach `formatToolDetails`
// unredacted. So this layer re-scrubs the rendered text/details as defence in depth.
//
// This mirrors `relux_core::redact_secrets` byte-for-byte (same prefixes, same key=value /
// key: value lookback) so the kernel reply and the dashboard's deduplicated result block agree on
// the same redacted body. It is a conservative scrubber, NOT a security boundary — it masks
// well-known token prefixes and the right-hand side of secret-named key/value pairs. Reference:
// Hermes `agent/redact.py` and OpenClaw `redactStringsDeep` / `sanitizeToolResult`
// (reference/openclaw-main/src/agents/pi-embedded-subscribe.tools.ts).

export const REDACTION_PLACEHOLDER = "***REDACTED***";

// Known high-signal secret token prefixes — kept in sync with relux_core::redact::SECRET_PREFIXES.
const SECRET_PREFIXES = [
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
  "relux_agt_",
];

// Key-name fragments that mark the right-hand side of a `key=value` / `key: value` pair as secret.
const SECRET_KEY_FRAGMENTS = ["key", "token", "secret", "password", "passwd", "auth", "credential"];

// Wrapper characters stripped from a token before matching and re-applied around the placeholder.
const WRAPPERS = new Set(['"', "'", "`", "(", ")", "[", "]", "{", "}", ",", ";", ".", "<", ">"]);

function stripWrappers(token: string): [string, string, string] {
  let start = 0;
  while (start < token.length && WRAPPERS.has(token[start]!)) start++;
  let end = token.length;
  while (end > start && WRAPPERS.has(token[end - 1]!)) end--;
  return [token.slice(0, start), token.slice(start, end), token.slice(end)];
}

function looksLikeSecretToken(token: string): boolean {
  // Every supported prefix yields a token well over this length when a real secret body is
  // present; this keeps short words (e.g. "sk-1") safe.
  if (token.length < 12) return false;
  return SECRET_PREFIXES.some((p) => token.startsWith(p));
}

function keyNamesSecret(key: string): boolean {
  const [, core] = stripWrappers(key);
  const norm = core.toLowerCase();
  return norm.length > 0 && SECRET_KEY_FRAGMENTS.some((f) => norm.includes(f));
}

// True when `word` is a bare secret key awaiting its value on the next word (`"auth_token":`).
function isSecretKeyMarker(word: string): boolean {
  if (!word.endsWith(":") && !word.endsWith("=")) return false;
  return keyNamesSecret(word.slice(0, -1));
}

// Mask the value of a `key=value` / `key: value` token whose key names a secret; else null.
function redactKeyValue(word: string): string | null {
  for (const sep of ["=", ":"]) {
    const idx = word.indexOf(sep);
    if (idx < 0) continue;
    const keyRaw = word.slice(0, idx);
    const value = word.slice(idx + 1);
    // Skip URL-ish `scheme://...` (the value would start with '//') and empty values.
    if (value.startsWith("/") || value.length === 0) continue;
    const [, keyCore] = stripWrappers(keyRaw.trim());
    const keyNorm = keyCore.toLowerCase();
    if (!keyNorm) continue;
    const namesSecret = SECRET_KEY_FRAGMENTS.some((f) => keyNorm.includes(f));
    const [vlead, vcore, vtrail] = stripWrappers(value);
    if (namesSecret && vcore.length >= 6) {
      return `${keyRaw}${sep}${vlead}${REDACTION_PLACEHOLDER}${vtrail}`;
    }
  }
  return null;
}

function redactWord(word: string): string {
  const kv = redactKeyValue(word);
  if (kv !== null) return kv;
  const [lead, core, trail] = stripWrappers(word);
  if (looksLikeSecretToken(core)) return `${lead}${REDACTION_PLACEHOLDER}${trail}`;
  return word;
}

// Redact the value word that follows a secret key marker — the key already told us it's a secret.
function redactPendingValue(word: string): string {
  const [lead, core, trail] = stripWrappers(word);
  if (core.length >= 6) return `${lead}${REDACTION_PLACEHOLDER}${trail}`;
  return word;
}

function redactLine(line: string): string {
  let result = "";
  let word = "";
  let valuePending = false;
  const flush = () => {
    result += valuePending ? redactPendingValue(word) : redactWord(word);
    valuePending = isSecretKeyMarker(word);
  };
  for (const ch of line) {
    if (/\s/.test(ch)) {
      if (word) {
        flush();
        word = "";
      }
      result += ch;
    } else {
      word += ch;
    }
  }
  if (word) flush();
  return result;
}

// Redact obvious secrets from `input`, preserving whitespace, newlines, and structure. Idempotent:
// re-running over already-redacted text leaves the placeholder unchanged.
export function redactSecrets(input: string): string {
  if (!input) return input;
  return input.split("\n").map(redactLine).join("\n");
}
