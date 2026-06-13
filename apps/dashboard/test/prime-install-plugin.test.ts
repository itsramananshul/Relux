// Contract tests for the backend-governed Prime GitHub plugin-install action client
// (`reluxPrime.installPluginFromGithub` → POST /v1/relux/prime/actions/install-plugin).
// This is the SINGLE chokepoint the Prime chat card's "Confirm import" button now calls
// instead of chaining install-github + hints + approvals.decide client-side. The tests
// pin the HTTP method + path + body the dashboard sends so the kernel route can never
// silently drift, and confirm the committed bundle ships the new route (catches a stale
// dist). Run: `npm test` or `node --test test/prime-install-plugin.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { readdirSync, readFileSync } from "node:fs";
import { reluxPrime } from "../src/api.ts";

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

test("installPluginFromGithub POSTs the repo_url + proposed id + approval id", async () => {
  stubFetch({
    plugin: { id: "relux-plugin-repo", enabled: false, generated: true },
    source: "https://github.com/owner/repo",
    generated: true,
    scanned: true,
    candidate_count: 0,
    candidates: [],
    next_actions: ["Open the Plugins page to enable or configure this plugin's tools."],
    no_code_executed: true,
    approval_closed: true,
  });
  await reluxPrime.installPluginFromGithub(
    "https://github.com/owner/repo",
    "relux-plugin-repo",
    "appr_1",
  );
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/prime/actions/install-plugin");
  assert.deepEqual(captured!.body, {
    repo_url: "https://github.com/owner/repo",
    plugin_id: "relux-plugin-repo",
    approval_id: "appr_1",
  });
});

test("installPluginFromGithub omits absent optional fields", async () => {
  stubFetch({
    plugin: { id: "relux-plugin-repo", enabled: false, generated: false },
    source: "https://github.com/owner/repo",
    generated: false,
    scanned: true,
    candidate_count: 1,
    candidates: [],
    next_actions: [],
    no_code_executed: true,
    approval_closed: false,
  });
  // No proposed id, no approval id — the body carries only the required repo_url.
  await reluxPrime.installPluginFromGithub("https://github.com/owner/repo");
  assert.deepEqual(captured!.body, { repo_url: "https://github.com/owner/repo" });
});

test("installPluginFromGithub returns the structured result envelope", async () => {
  const envelope = {
    plugin: { id: "relux-plugin-repo", enabled: false, generated: true },
    source: "https://github.com/owner/repo",
    generated: true,
    scanned: true,
    candidate_count: 2,
    candidates: [{ id: "c1" }, { id: "c2" }],
    next_actions: ["Review 2 detected capability candidates on the Plugins page."],
    no_code_executed: true,
    approval_id: "appr_1",
    approval_closed: true,
  };
  stubFetch(envelope);
  const result = await reluxPrime.installPluginFromGithub(
    "https://github.com/owner/repo",
    "relux-plugin-repo",
    "appr_1",
  );
  assert.equal(result.candidate_count, 2);
  assert.equal(result.no_code_executed, true);
  assert.equal(result.approval_closed, true);
  assert.equal(result.plugin.id, "relux-plugin-repo");
});

test("the committed dashboard bundle ships the backend install-plugin action route", () => {
  // The bundle hashes its filenames, so scan the Prime page chunk(s) for the route.
  const assetsDir = join(distDir, "assets");
  const files = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const hit = files.some((f) =>
    readFileSync(join(assetsDir, f), "utf8").includes("/v1/relux/prime/actions/install-plugin"),
  );
  assert.ok(hit, "expected the rebuilt dist to reference the new install-plugin action route");
});
