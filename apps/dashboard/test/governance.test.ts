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
  isManagerSubtree,
  managerSubtreePermission,
  pluginWildcardPermission,
  managerSubtreeActions,
  managerGrantAvailability,
  parseTokenTtlSecs,
  agentTokenLooksValid,
  assignTaskFormReason,
  managerGrantFormReason,
  assignTaskCurlSnippet,
  managerGrantCurlSnippet,
  AGENT_SELF_ASSIGN_TASK_ROUTE,
  AGENT_SELF_MANAGER_GRANT_ROUTE,
  type ManagerGrantAgent,
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

test("the manager-subtree scope is accepted; malformed subtree strings are rejected", () => {
  // The accepted advanced/manager scope.
  assert.equal(permissionInvalidReason("agent:lead-1:subtree:grant_permission"), null);
  assert.ok(isValidPermission("agent:lead-1:subtree:grant_permission"));
  assert.ok(isManagerSubtree("agent:lead-1:subtree:grant_permission"));
  // A plain exact `agent:` capability is not a subtree scope (and stays valid).
  assert.ok(!isManagerSubtree("agent:lead-1:configure"));
  assert.equal(permissionInvalidReason("agent:lead-1:configure"), null);

  // Malformed subtree attempts are rejected with the scope-specific reason.
  for (const bad of [
    "agent:lead-1:subtree",
    "agent:lead-1:subtree:",
    "agent::subtree:grant",
    "agent:lead-1:subtree:a:b",
    "agent:subtree:grant",
    "agent:lead-1:subtree:*", // also a wildcard, but the subtree reason is fine too
  ]) {
    assert.ok(permissionInvalidReason(bad), `${bad} must be rejected`);
    assert.equal(isValidPermission(bad), false);
  }
  assert.match(
    permissionInvalidReason("agent:lead-1:subtree")!,
    /manager-subtree scope must be exactly/,
  );
  // The keyword is case-sensitive: `Subtree` stays an opaque valid `agent:` capability.
  assert.equal(permissionInvalidReason("agent:lead-1:Subtree:grant"), null);
});

test("managerSubtreePermission builds the scope from a manager id + action, null when malformed", () => {
  assert.equal(
    managerSubtreePermission("lead-1", "grant_permission"),
    "agent:lead-1:subtree:grant_permission",
  );
  assert.equal(
    managerSubtreePermission("  lead-1  ", "  grant_permission  "),
    "agent:lead-1:subtree:grant_permission",
  );
  assert.equal(managerSubtreePermission("bad id", "grant"), null);
  assert.equal(managerSubtreePermission("lead-1", "bad action"), null);
  assert.equal(managerSubtreePermission("", "grant"), null);
});

test("the manager-subtree scope is an elevated (advanced/manager) capability", () => {
  assert.equal(isElevatedPermission("agent:lead-1:subtree:grant_permission"), true);
});

test("managerSubtreeActions reads own-id subtree scopes only", () => {
  const lead: ManagerGrantAgent = {
    id: "lead",
    permissions: [
      "agent:lead:subtree:grant_permission",
      "agent:lead:subtree:assign_task",
      "agent:other:subtree:grant_permission", // names a DIFFERENT manager → ignored
      "tool:relux-tools-github:read", // not a subtree scope → ignored
    ],
  };
  assert.deepEqual(managerSubtreeActions(lead), ["grant_permission", "assign_task"]);
  assert.deepEqual(managerSubtreeActions({ id: "lead", permissions: [] }), []);
});

test("managerGrantAvailability mirrors the backend authority gate", () => {
  // Topology: director <- lead <- ic ; peer reports to director (lead's sibling).
  const roster: ManagerGrantAgent[] = [
    { id: "director", status: "active" },
    { id: "lead", status: "active", reports_to: "director",
      permissions: ["agent:lead:subtree:grant_permission"] },
    { id: "ic", status: "active", reports_to: "lead" },
    { id: "peer", status: "active", reports_to: "director" },
  ];
  const lead = roster[1];

  // Available: live, scoped, with a non-empty Branch (only ic, not the sibling peer).
  const ok = managerGrantAvailability(lead, roster);
  assert.equal(ok.available, true);
  assert.equal(ok.reason, "");
  assert.deepEqual(ok.targets, ["ic"]);

  // No scope → unavailable with an honest reason (director has a subordinate but no scope).
  const noScope = managerGrantAvailability(roster[0], roster);
  assert.equal(noScope.available, false);
  assert.match(noScope.reason, /No manager-subtree grant scope/);

  // Paused manager → unavailable (a non-Active manager wields no subtree authority).
  const paused = managerGrantAvailability({ ...lead, status: "paused" }, roster);
  assert.equal(paused.available, false);
  assert.match(paused.reason, /Active/);

  // Scoped but empty Branch (a manager whose only report was removed) → unavailable.
  const lonely: ManagerGrantAgent = {
    id: "lonely",
    status: "active",
    permissions: ["agent:lonely:subtree:grant_permission"],
  };
  const empty = managerGrantAvailability(lonely, [lonely]);
  assert.equal(empty.available, false);
  assert.match(empty.reason, /Branch/);
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

test("agentTokenLooksValid accepts the relux_agt_ shape only", () => {
  assert.equal(agentTokenLooksValid("relux_agt_deadbeef01234567"), true);
  assert.equal(agentTokenLooksValid("  relux_agt_abc123  "), true);
  // Wrong prefix / empty / shaped-like-something-else are rejected.
  assert.equal(agentTokenLooksValid("relux_session_abc"), false);
  assert.equal(agentTokenLooksValid("agt_abc"), false);
  assert.equal(agentTokenLooksValid(""), false);
  assert.equal(agentTokenLooksValid("relux_agt_"), false);
});

test("assignTaskFormReason gates the token test form, null when ready", () => {
  // Missing/!shaped token, missing task, missing target each get an honest reason.
  assert.match(assignTaskFormReason("", "task_1", "ic")!, /raw token/);
  assert.match(assignTaskFormReason("not-a-token", "task_1", "ic")!, /relux_agt_/);
  assert.match(assignTaskFormReason("relux_agt_abc", "", "ic")!, /task id/);
  assert.match(assignTaskFormReason("relux_agt_abc", "task_1", "")!, /target/);
  // All present + well-shaped → ready.
  assert.equal(assignTaskFormReason("relux_agt_abc", "task_1", "ic"), null);
});

test("managerGrantFormReason gates the token grant form, validating the permission grammar, null when ready", () => {
  // Missing/!shaped token, missing target each get an honest reason (same as the assign form).
  assert.match(managerGrantFormReason("", "ic", "tool:relux-tools-echo:say")!, /raw token/);
  assert.match(managerGrantFormReason("not-a-token", "ic", "tool:relux-tools-echo:say")!, /relux_agt_/);
  assert.match(managerGrantFormReason("relux_agt_abc", "", "tool:relux-tools-echo:say")!, /target/);
  // The permission is validated against the backend grammar BEFORE the API: blank and
  // malformed strings are rejected with the add-permission form's own reasons.
  assert.match(managerGrantFormReason("relux_agt_abc", "ic", "")!, /Enter a permission/);
  assert.match(managerGrantFormReason("relux_agt_abc", "ic", "not-a-prefix")!, /Must start with/);
  assert.match(managerGrantFormReason("relux_agt_abc", "ic", "tool:*")!, /wildcard/);
  // All present + a well-formed capability → ready.
  assert.equal(managerGrantFormReason("relux_agt_abc", "ic", "tool:relux-tools-echo:say"), null);
});

test("the curl snippets embed NO secret (token is the $RELUX_AGENT_TOKEN var) and hit the real routes", () => {
  const assign = assignTaskCurlSnippet("task_0001", "ic");
  // The real route + body field names, never the operator console.
  assert.ok(assign.includes(AGENT_SELF_ASSIGN_TASK_ROUTE));
  assert.match(assign, /"task_id":"task_0001"/);
  assert.match(assign, /"target_agent_id":"ic"/);
  // The token is a shell variable — never an inlined secret.
  assert.match(assign, /Bearer \$RELUX_AGENT_TOKEN/);
  assert.ok(!assign.includes("relux_agt_"), "snippet must not inline a raw token");

  // Blank ids fall back to clear placeholders (no crash, shape stays obvious).
  const blank = assignTaskCurlSnippet("", "");
  assert.match(blank, /<task_id>/);
  assert.match(blank, /<target_agent_id>/);

  const grant = managerGrantCurlSnippet("ic", "tool:relux-tools-echo:say");
  assert.ok(grant.includes(AGENT_SELF_MANAGER_GRANT_ROUTE));
  assert.match(grant, /"target_id":"ic"/);
  assert.match(grant, /"permission":"tool:relux-tools-echo:say"/);
  assert.match(grant, /Bearer \$RELUX_AGENT_TOKEN/);
  assert.ok(!grant.includes("relux_agt_"), "snippet must not inline a raw token");
});

test("parseTokenTtlSecs converts days→secs and treats blank/invalid as unspecified", () => {
  // Blank / whitespace → undefined (backend applies its default + clamp).
  assert.equal(parseTokenTtlSecs(""), undefined);
  assert.equal(parseTokenTtlSecs("   "), undefined);
  // Non-positive / non-numeric → undefined (never mints a dead or garbage TTL).
  assert.equal(parseTokenTtlSecs("0"), undefined);
  assert.equal(parseTokenTtlSecs("-5"), undefined);
  assert.equal(parseTokenTtlSecs("abc"), undefined);
  // A positive day count converts to seconds.
  assert.equal(parseTokenTtlSecs("7"), 7 * 86400);
  assert.equal(parseTokenTtlSecs("1.5"), Math.round(1.5 * 86400));
});
