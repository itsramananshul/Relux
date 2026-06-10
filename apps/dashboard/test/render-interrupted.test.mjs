// Render/DOM verification for the interrupted-orchestration UX
// (RELUX_MASTER_PLAN Sec 15). The pure helpers in `orchestration.ts` are unit
// tested, but those tests never prove the React component actually RENDERS the
// interrupted callout + Continue button — the bug a user sees is a blank/broken
// dashboard or a missing callout after a restart, which a pure-function test
// cannot catch.
//
// This harness closes that gap WITHOUT a browser and WITHOUT new dependencies:
//   1. Render path — it transpiles the REAL `OrchestrationRow` with the esbuild
//      already vendored by Vite, then server-renders it through react-dom/server
//      + react-router's StaticRouter (both already present). It exercises the
//      genuine JSX conditional, so a regression that hides the callout, drops the
//      Continue button, or shows the callout for a non-interrupted run fails here.
//   2. Shipped-bundle path — it reads the COMMITTED bundle the kernel actually
//      serves (`crates/relix-web-bridge/dashboard-dist`) and asserts the
//      index.html asset wiring is intact and the JS bundle carries the callout
//      copy. This catches a "blank/broken dashboard" (broken asset refs) and a
//      STALE dist (source changed, bundle not rebuilt → served UI missing the
//      callout) — the exact restart-time failure, in the artifact that ships.
//
// Run: `npm test` (auto-discovered) or `node --test test/render-interrupted.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const componentsDir = join(dashboardRoot, "src", "components");
const repoRoot = resolve(dashboardRoot, "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

// ── Render path: transpile + server-render the real component ───────────────

// A tiny entry that renders OrchestrationRow inside a StaticRouter (the row uses
// <Link>, so it needs a router context) and returns the static markup. esbuild
// bundles the real component graph; nothing is mocked except the click handlers.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { OrchestrationRow } from "./OrchestrationPanel.tsx";
const noop = () => {};
export function render(o, job) {
  return renderToStaticMarkup(
    <StaticRouter location="/prime">
      <OrchestrationRow o={o} job={job} onRun={noop} onCancel={noop} />
    </StaticRouter>
  );
}
`;

let tmp = null;
let render = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: componentsDir,
      loader: "tsx",
      sourcefile: "render-entry.tsx",
    },
    bundle: true,
    // CJS so the fully-bundled output keeps native `require` for node builtins
    // (react-dom's server entry dynamic-requires "stream"); a single bundled React
    // copy is shared by the component, react-dom/server, and react-router, so
    // hooks stay consistent.
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  // Fully self-contained (no external imports), so it can live in the OS tmpdir
  // and never touch the repo tree even if a run is interrupted before cleanup.
  tmp = mkdtempSync(join(tmpdir(), "relux-render-"));
  const out = join(tmp, "render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

// Fixtures mirror the reconstructed-interrupted shape pinned in
// orchestration.test.ts: a durable: id, no live worker, completed + pending mix.
function step(taskId, agentId, outcome) {
  return { task_id: taskId, agent_id: agentId, role: "implementation", title: `Brief ${taskId}`, outcome };
}
function orch(id, status, steps) {
  return {
    id, goal: "research the options and document the findings", created_by: "founder",
    namespace_id: "workspace", status, steps, notes: [], created_at: "t0", updated_at: "t0",
  };
}
function jobStep(taskId, outcome) {
  return { task_id: taskId, agent_id: "prime", title: `Brief ${taskId}`, outcome };
}
function baseJob(state, extra = {}) {
  return {
    id: "job_0001", orchestration_id: "orch_0001", state, max: 25, concurrency: 2,
    current_round: 0, ran: 0, completed: 0, failed: 0, blocked: 0, steps: [], ...extra,
  };
}

// The partially-run record + reconstructed interrupted job a poll-after-restart
// produces: 2 of 4 briefs done, 2 pending, synthetic durable: id, no live worker.
const partiallyRun = orch("orch_0001", "running", [
  step("task_0001", "research-agent", "completed"),
  step("task_0002", "research-agent", "completed"),
  step("task_0003", "doc-agent", "pending"),
  step("task_0004", "doc-agent", "pending"),
]);
const interruptedJob = baseJob("interrupted", {
  id: "durable:orch_0001", ran: 2, completed: 2, current_round: 1,
  steps: [
    jobStep("task_0001", "completed"), jobStep("task_0002", "completed"),
    jobStep("task_0003", "pending"), jobStep("task_0004", "pending"),
  ],
});

test("an interrupted run RENDERS the callout, durable progress, and a Continue button", () => {
  const html = render(partiallyRun, interruptedJob);
  // The honest, cause-neutral callout headline + reconstructed-record disclaimer.
  assert.match(html, /Run interrupted/);
  assert.match(html, /no live worker/);
  assert.match(html, /Reconstructed from the durable record/);
  // The durable progress and the count a Continue run would resume.
  assert.match(html, /2\/4 briefs run/);
  assert.match(html, /2 pending/);
  assert.match(html, /Continue starts a fresh run that resumes the 2 pending/);
  // The actionable control is a real, labelled Continue button.
  assert.match(html, /<button[^>]*>Continue<\/button>/);
});

test("a fresh planned orchestration shows Run, and NOT the interrupted callout", () => {
  const planned = orch("orch_0002", "planned", [step("task_0001", "a", "pending")]);
  const html = render(planned, null);
  assert.match(html, /<button[^>]*>Run orchestration<\/button>/);
  // The callout is conditional — it must not leak onto a plan that never ran.
  assert.doesNotMatch(html, /Run interrupted/);
});

test("a live running job shows the live phase banner, NOT the interrupted callout", () => {
  // A real in-process worker: process-local id (not durable:), state running.
  const liveJob = baseJob("running", {
    current_round: 2,
    steps: [jobStep("task_0001", "completed"), jobStep("task_0002", "running")],
    ran: 1, completed: 1,
  });
  const html = render(partiallyRun, liveJob);
  assert.match(html, /Running — round 2/);
  assert.doesNotMatch(html, /Run interrupted/);
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

test("the committed dashboard bundle has intact index.html asset wiring", () => {
  const indexHtml = readFileSync(join(distDir, "index.html"), "utf8");
  // The SPA mount point — without it the dashboard renders blank.
  assert.match(indexHtml, /<div id="root">/);
  // The hashed JS + CSS are referenced under the /dashboard/ base the kernel
  // serves (a wrong base or a missing ref is the classic "blank dashboard").
  const js = indexHtml.match(/\/dashboard\/assets\/([\w.-]+\.js)/);
  const css = indexHtml.match(/\/dashboard\/assets\/([\w.-]+\.css)/);
  assert.ok(js, "index.html must reference a /dashboard/assets/*.js bundle");
  assert.ok(css, "index.html must reference a /dashboard/assets/*.css bundle");
  // The referenced asset files actually exist on disk and are non-empty.
  for (const name of [js[1], css[1]]) {
    const bytes = readFileSync(join(distDir, "assets", name));
    assert.ok(bytes.length > 0, `${name} must be non-empty`);
  }
});

test("the shipped JS bundle carries the interrupted callout copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII-only fragments survive minification; if the source gained the callout
  // but the committed bundle was never rebuilt, these are absent → fail.
  assert.match(bundle, /Run interrupted/);
  assert.match(bundle, /no live worker/);
  assert.match(bundle, /Reconstructed from the durable record/);
  assert.match(bundle, /Continue starts a fresh run that resumes the/);
});
