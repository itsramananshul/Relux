// Contract tests for the persistent allow-always grant client + the "Allow always"
// approval action (docs/HERMES_OPENCLAW_DEEP_AUDIT.md §5 P2 / §23). They pin the
// HTTP method + path + body the dashboard sends so the kernel routes can never
// silently drift, and confirm the committed bundle ships the new Approvals UI copy
// (catches a stale dist). Run: `npm test` or `node --test test/grants.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { readdirSync, readFileSync } from "node:fs";
import { reluxGrants, reluxApprovals } from "../src/api.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

// Capture the last fetch call and reply with a canned 200 JSON body.
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

test("reluxGrants.list GETs the grants collection", async () => {
  stubFetch([]);
  await reluxGrants.list();
  assert.equal(captured!.method, "GET");
  assert.equal(captured!.url, "/v1/relux/grants");
});

test("reluxGrants.create POSTs the plugin/tool (and optional agent) body", async () => {
  stubFetch({ id: "grant_0001" });
  await reluxGrants.create({
    plugin_id: "relux-plugin-my-repo",
    tool_name: "deploy.run",
    agent_id: "prime",
  });
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/grants");
  assert.deepEqual(captured!.body, {
    plugin_id: "relux-plugin-my-repo",
    tool_name: "deploy.run",
    agent_id: "prime",
  });
});

test("reluxGrants.revoke DELETEs the grant by id (url-encoded)", async () => {
  stubFetch({ revoked: true });
  await reluxGrants.revoke("grant_0001");
  assert.equal(captured!.method, "DELETE");
  assert.equal(captured!.url, "/v1/relux/grants/grant_0001");
});

test("reluxApprovals.allowAlways POSTs to the approval's allow-always route", async () => {
  stubFetch({ id: "appr_0001", status: "approved" });
  await reluxApprovals.allowAlways("appr_0001");
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/approvals/appr_0001/allow-always");
});

test("the committed dashboard bundle ships the allow-always Approvals copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // The narrow "Allow always" action + the grants panel + the "Approve once" relabel.
  assert.match(bundle, /Allow always/);
  assert.match(bundle, /Allow-always grants/);
  assert.match(bundle, /Approve once/);
});
