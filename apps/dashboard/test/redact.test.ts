import { test } from "node:test";
import assert from "node:assert/strict";
import { redactSecrets, REDACTION_PLACEHOLDER } from "../src/redact.ts";

// Build key-shaped tokens at runtime so no literal secret appears in source (keeps secret
// scanners + reviewers calm), mirroring the relux_core::redact test convention.
const tok = (prefix: string, body: string) => `${prefix}${body}`;

test("redactSecrets masks known token prefixes", () => {
  const sk = tok("sk-ant-", "0123456789abcdef0123");
  const gh = tok("ghp_", "0123456789abcdef0123");
  const out = redactSecrets(`using ${sk} and ${gh} now`);
  assert.ok(!out.includes(sk), `anthropic token leaked: ${out}`);
  assert.ok(!out.includes(gh), `github token leaked: ${out}`);
  assert.ok(out.startsWith("using "));
  assert.ok(out.endsWith(" now"));
  assert.equal(out.split(REDACTION_PLACEHOLDER).length - 1, 2);
});

test("redactSecrets masks key=value and key: value secret pairs", () => {
  const secret = tok("", "supersecretvalue123");
  const env = redactSecrets(`API_KEY=${secret}`);
  assert.ok(env.startsWith("API_KEY="));
  assert.ok(!env.includes(secret));
  assert.ok(env.includes(REDACTION_PLACEHOLDER));

  const json = redactSecrets(`"auth_token": "${secret}"`);
  assert.ok(!json.includes(secret), `json secret leaked: ${json}`);
  assert.ok(json.includes(REDACTION_PLACEHOLDER));
});

test("redactSecrets leaves ordinary text and URLs untouched, and is idempotent", () => {
  const plain = "Ran 3 tests, 0 failures. Updated README.md and src/main.rs.";
  assert.equal(redactSecrets(plain), plain);
  const url = "Listening on http://127.0.0.1:8080/healthz";
  assert.equal(redactSecrets(url), url);
  // Re-running over already-redacted output is a no-op (so dashboard re-scrub matches the kernel).
  const once = redactSecrets(`token ${tok("sk-", "0123456789abcdef0123")} here`);
  assert.equal(redactSecrets(once), once);
});
