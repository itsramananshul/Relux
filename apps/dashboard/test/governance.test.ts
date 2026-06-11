// Pure-helper tests for the Crew permission governance surface. These pin the
// client-side validation + risk classification so the form rejects a malformed
// permission BEFORE the API does, and flags control-plane prefixes for confirmation.
// Run: `npm test` (auto-discovered) or `node --test test/governance.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  VALID_PERMISSION_PREFIXES,
  permissionPrefix,
  isValidPermission,
  permissionInvalidReason,
  permissionRisk,
  isElevatedPermission,
  isScopedWildcard,
  pluginWildcardPermission,
} from "../src/governance.ts";

test("permissionPrefix extracts the prefix incl. the colon", () => {
  assert.equal(permissionPrefix("tool:relux-tools-github:read"), "tool:");
  assert.equal(permissionPrefix("exec:shell:run"), "exec:");
  assert.equal(permissionPrefix("garbage"), "");
});

test("isValidPermission mirrors the backend prefix allowlist", () => {
  for (const prefix of VALID_PERMISSION_PREFIXES) {
    assert.ok(isValidPermission(`${prefix}resource:action`), `expected ok for ${prefix}`);
  }
  assert.equal(isValidPermission(""), false);
  assert.equal(isValidPermission("   "), false);
  assert.equal(isValidPermission("fs:some:action"), false);
  assert.equal(isValidPermission("not-a-prefix"), false);
});

test("permissionInvalidReason explains an empty or malformed string, null when valid", () => {
  assert.match(permissionInvalidReason("")!, /Enter a permission/);
  assert.match(permissionInvalidReason("nope")!, /Must start with/);
  assert.equal(permissionInvalidReason("tool:relux-tools-echo:say"), null);
});

test("the scoped tool-plugin wildcard is accepted; broader/partial globs are rejected", () => {
  // The one accepted scope.
  assert.equal(permissionInvalidReason("tool:relux-tools-github:*"), null);
  assert.ok(isValidPermission("tool:relux-tools-github:*"));
  assert.ok(isScopedWildcard("tool:relux-tools-github:*"));
  assert.ok(!isScopedWildcard("tool:relux-tools-github:create_pr"));

  // Broad / partial / non-tool wildcards are rejected with a scope-specific reason.
  for (const bad of ["*", "tool:*", "tool:*:*", "tool:relux-tools-github:cre*", "agent:bot:*"]) {
    const reason = permissionInvalidReason(bad);
    assert.ok(reason, `${bad} must be rejected`);
    assert.equal(isValidPermission(bad), false);
  }
  assert.match(permissionInvalidReason("tool:relux-tools-github:cre*")!, /Only `tool:<plugin-id>:\*`/);
});

test("path-like / injection strings are rejected", () => {
  for (const bad of [
    "tool:relux-tools-github:../etc",
    "tool:relux-tools-github:read write",
    "tool:relux/tools:read",
  ]) {
    assert.match(permissionInvalidReason(bad)!, /Remove spaces, slashes/);
  }
});

test("pluginWildcardPermission builds the scope from a plugin id, null when malformed", () => {
  assert.equal(pluginWildcardPermission("relux-tools-github"), "tool:relux-tools-github:*");
  assert.equal(pluginWildcardPermission("  relux-tools-github  "), "tool:relux-tools-github:*");
  assert.equal(pluginWildcardPermission("bad id"), null);
  assert.equal(pluginWildcardPermission("relux/tools"), null);
  assert.equal(pluginWildcardPermission(""), null);
});

test("control-plane prefixes are elevated; tool/task/audit are standard", () => {
  for (const p of [
    "adapter:relux-adapter-claude-cli:run",
    "provider:openai:chat",
    "exec:shell:run",
    "plugin:relux-tools-github:install",
    "agent:research-bot:configure",
    "approval:hire:decide",
  ]) {
    assert.equal(permissionRisk(p), "elevated", `${p} should be elevated`);
    assert.equal(isElevatedPermission(p), true);
  }
  for (const p of [
    "tool:relux-tools-github:read",
    "task:task_0001:update",
    "audit:events:read",
  ]) {
    assert.equal(permissionRisk(p), "standard", `${p} should be standard`);
    assert.equal(isElevatedPermission(p), false);
  }
});
