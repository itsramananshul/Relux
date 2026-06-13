// Contract tests for the backend-governed Prime capability-activation client
// (`reluxPrime.configureCandidate` → POST /v1/relux/prime/actions/configure-candidate)
// and the `configurePluginCandidateAction` chat-action parser. This is the SINGLE
// chokepoint the Prime chat's "Configure with Prime" button calls — it must send the
// exact plugin + candidate ids (and optional approval) the kernel re-resolves +
// re-validates server-side. The tests pin the HTTP method + path + body so the route
// can never silently drift, exercise the defensive action parser, and confirm the
// committed bundle ships the new route (catches a stale dist). Run: `npm test` or
// `node --test test/prime-configure-candidate.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { readdirSync, readFileSync } from "node:fs";
import { reluxPrime } from "../src/api.ts";
import { configurePluginCandidateAction } from "../src/prime.ts";

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
  candidate_id: "cli-bin-cool",
  kind: "cli_command",
  activation: "command_tool",
  tool_name: "cool.run",
  next_step: "Configured command tool \"cool.run\". It is gated (needs approval) — ask me to run the cool.run tool and I'll stage the approval before it runs.",
  no_code_executed: true,
  approval_closed: true,
};

test("configureCandidate POSTs the plugin id + candidate id + approval id", async () => {
  stubFetch(SAMPLE);
  await reluxPrime.configureCandidate("relux-plugin-repo", "cli-bin-cool", "appr_1");
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/prime/actions/configure-candidate");
  assert.deepEqual(captured!.body, {
    plugin_id: "relux-plugin-repo",
    candidate_id: "cli-bin-cool",
    approval_id: "appr_1",
  });
});

test("configureCandidate omits an absent approval id", async () => {
  stubFetch(SAMPLE);
  await reluxPrime.configureCandidate("relux-plugin-repo", "mcp");
  assert.deepEqual(captured!.body, {
    plugin_id: "relux-plugin-repo",
    candidate_id: "mcp",
  });
});

test("configureCandidate returns the structured activation envelope", async () => {
  stubFetch(SAMPLE);
  const r = await reluxPrime.configureCandidate("relux-plugin-repo", "cli-bin-cool");
  assert.equal(r.activation, "command_tool");
  assert.equal(r.tool_name, "cool.run");
  assert.equal(r.no_code_executed, true);
});

test("configurePluginCandidateAction parses a well-formed action", () => {
  const parsed = configurePluginCandidateAction({
    type: "configure_plugin_candidate",
    plugin_id: "relux-plugin-repo",
    candidate_id: "mcp",
  });
  assert.deepEqual(parsed, { pluginId: "relux-plugin-repo", candidateId: "mcp" });
});

test("configurePluginCandidateAction tolerates an empty plugin selector", () => {
  // "configure the first candidate" carries no plugin — the backend resolves it.
  const parsed = configurePluginCandidateAction({
    type: "configure_plugin_candidate",
    plugin_id: "",
    candidate_id: "first",
  });
  assert.deepEqual(parsed, { pluginId: "", candidateId: "first" });
});

test("configurePluginCandidateAction rejects other / malformed actions", () => {
  assert.equal(configurePluginCandidateAction(null), null);
  assert.equal(configurePluginCandidateAction(undefined), null);
  assert.equal(
    configurePluginCandidateAction({ type: "install_plugin_from_github", repo_url: "x" }),
    null,
  );
  // Missing the candidate selector ⇒ unshaped ⇒ null (never trusted).
  assert.equal(
    configurePluginCandidateAction({ type: "configure_plugin_candidate", plugin_id: "x" }),
    null,
  );
});

test("the committed dashboard bundle ships the backend configure-candidate route", () => {
  const assetsDir = join(distDir, "assets");
  const files = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const hit = files.some((f) =>
    readFileSync(join(assetsDir, f), "utf8").includes("/v1/relux/prime/actions/configure-candidate"),
  );
  assert.ok(hit, "expected the rebuilt dist to reference the new configure-candidate action route");
});
