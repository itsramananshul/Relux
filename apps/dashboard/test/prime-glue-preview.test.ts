// Contract test for the tool-glue preview client (`reluxPrime.previewGlue` →
// POST /v1/relux/prime/glue/preview; RELUX_MASTER_PLAN §23, the execute_code foundation).
// It pins the HTTP method + path + body the dashboard sends so the inert grounding route can
// never silently drift, returns the proposal envelope verbatim, and confirms the committed
// bundle ships the route (catches a stale dist). Run: `npm test` or
// `node --test test/prime-glue-preview.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { readdirSync, readFileSync } from "node:fs";
import { reluxPrime, type ReluxPrimeToolPlanProposal } from "../src/api.ts";

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

const PROPOSAL: ReluxPrimeToolPlanProposal = {
  goal: "inspect then summarise",
  summary: "2 steps",
  steps: [
    { index: 1, plugin: "acme", tool: "build", args: {}, readiness: "ready" },
    { index: 2, plugin: "ghost", tool: "missing", args: {}, readiness: "unknown" },
  ],
  ready_to_create: false,
  issues: ["step 2: tool ghost/missing is not in the catalog"],
};

test("previewGlue POSTs { goal, steps, extended } to the glue-preview route", async () => {
  stubFetch(PROPOSAL);
  await reluxPrime.previewGlue(
    "inspect then summarise",
    [
      { plugin: "acme", tool: "build", args: { x: 1 } },
      { plugin: "ghost", tool: "missing" },
    ],
    true,
  );
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/prime/glue/preview");
  assert.deepEqual(captured!.body, {
    goal: "inspect then summarise",
    steps: [
      { plugin: "acme", tool: "build", args: { x: 1 } },
      { plugin: "ghost", tool: "missing" },
    ],
    extended: true,
  });
});

test("previewGlue defaults extended to false", async () => {
  stubFetch(PROPOSAL);
  await reluxPrime.previewGlue("g", [{ plugin: "acme", tool: "build" }]);
  assert.equal((captured!.body as { extended: boolean }).extended, false);
});

test("previewGlue returns the proposal envelope verbatim (unknown step preserved)", async () => {
  stubFetch(PROPOSAL);
  const result = await reluxPrime.previewGlue("g", [{ plugin: "acme", tool: "build" }]);
  assert.equal(result.ready_to_create, false);
  assert.equal(result.steps.length, 2);
  assert.equal(result.steps[1].readiness, "unknown");
  assert.ok(result.issues && result.issues.length === 1);
});

test("the committed dashboard bundle ships the glue-preview route", () => {
  const assetsDir = join(distDir, "assets");
  const files = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const hit = files.some((f) =>
    readFileSync(join(assetsDir, f), "utf8").includes("/v1/relux/prime/glue/preview"),
  );
  assert.ok(hit, "expected the rebuilt dist to reference the glue-preview route");
});
