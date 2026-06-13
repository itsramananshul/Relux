// Contract tests for the backend-governed from-scratch command-tool client
// (`reluxPrime.configureCommandTool` → POST /v1/relux/prime/actions/configure-command-tool)
// and the `configureCommandToolAction` chat-action parser. This is the bridge a
// source-only plugin (no relux-plugin.json, no detected candidate) uses to become usable
// without hand-editing JSON. The tests pin the HTTP method + path + body so the route can
// never silently drift, exercise the defensive action parser, and confirm the committed
// bundle ships the new route (catches a stale dist). Run: `npm test` or
// `node --test test/prime-configure-command-tool.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { readdirSync, readFileSync } from "node:fs";
import { reluxPrime } from "../src/api.ts";
import { configureCommandToolAction } from "../src/prime.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

type Captured = { url: string; method: string; body: unknown };
let captured: Captured | null = null;
const realFetch = globalThis.fetch;

function stubFetch(responseBody: unknown) {
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    captured = {
      url: String(url),
      method: init?.method ?? "GET",
      body: init?.body ? JSON.parse(String(init.body)) : undefined,
    };
    return {
      ok: true,
      status: 200,
      text: async () => JSON.stringify(responseBody),
    } as Response;
  }) as typeof fetch;
}

afterEach(() => {
  globalThis.fetch = realFetch;
  captured = null;
});

const SAMPLE = {
  plugin_id: "relux-plugin-repo",
  plugin_name: "repo",
  tool_name: "repo.build",
  permission: "tool:relux-plugin-repo:build",
  gated: true,
  next_step: 'Configured command tool "repo.build". Ask me to use repo.build.',
  no_code_executed: true,
  catalog_refresh: true,
  approval_closed: true,
};

test("configureCommandTool POSTs the recipe + approval id to the governed route", async () => {
  stubFetch(SAMPLE);
  await reluxPrime.configureCommandTool(
    {
      plugin_id: "relux-plugin-repo",
      name: "repo.build",
      program: "cargo",
      args: ["build"],
    },
    "appr_1",
  );
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/prime/actions/configure-command-tool");
  assert.deepEqual(captured!.body, {
    plugin_id: "relux-plugin-repo",
    name: "repo.build",
    program: "cargo",
    args: ["build"],
    approval_id: "appr_1",
  });
});

test("configureCommandTool omits an absent approval id", async () => {
  stubFetch(SAMPLE);
  await reluxPrime.configureCommandTool({
    plugin_id: "relux-plugin-repo",
    name: "repo.build",
    program: "cargo",
    args: ["build"],
  });
  assert.deepEqual(captured!.body, {
    plugin_id: "relux-plugin-repo",
    name: "repo.build",
    program: "cargo",
    args: ["build"],
  });
});

test("configureCommandTool returns the structured envelope (gated + catalog refresh)", async () => {
  stubFetch(SAMPLE);
  const r = await reluxPrime.configureCommandTool({
    plugin_id: "relux-plugin-repo",
    name: "repo.build",
    program: "cargo",
  });
  assert.equal(r.tool_name, "repo.build");
  assert.equal(r.gated, true);
  assert.equal(r.no_code_executed, true);
  assert.equal(r.catalog_refresh, true);
  assert.equal(r.permission, "tool:relux-plugin-repo:build");
});

test("configureCommandToolAction parses a well-formed action", () => {
  const parsed = configureCommandToolAction({
    type: "configure_command_tool",
    plugin_id: "acme-tools",
    tool_name: "cargo.build",
    program: "cargo",
    args: ["build"],
    cwd: "",
  });
  assert.deepEqual(parsed, {
    pluginId: "acme-tools",
    toolName: "cargo.build",
    program: "cargo",
    args: ["build"],
    cwd: "",
  });
});

test("configureCommandToolAction filters non-string args and tolerates an empty plugin", () => {
  const parsed = configureCommandToolAction({
    type: "configure_command_tool",
    plugin_id: "",
    tool_name: "npm.test",
    program: "npm",
    args: ["test", 5, null],
  });
  assert.deepEqual(parsed, {
    pluginId: "",
    toolName: "npm.test",
    program: "npm",
    args: ["test"],
    cwd: "",
  });
});

test("configureCommandToolAction rejects other / malformed actions", () => {
  assert.equal(configureCommandToolAction(null), null);
  assert.equal(configureCommandToolAction(undefined), null);
  assert.equal(
    configureCommandToolAction({ type: "configure_plugin_candidate", candidate_id: "mcp" }),
    null,
  );
  // Missing the program ⇒ unshaped ⇒ null (never trusted).
  assert.equal(
    configureCommandToolAction({ type: "configure_command_tool", plugin_id: "x" }),
    null,
  );
});

test("the committed dashboard bundle ships the backend configure-command-tool route", () => {
  const assetsDir = join(distDir, "assets");
  const files = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const hit = files.some((f) =>
    readFileSync(join(assetsDir, f), "utf8").includes(
      "/v1/relux/prime/actions/configure-command-tool",
    ),
  );
  assert.ok(hit, "expected the rebuilt dist to reference the new configure-command-tool route");
});
