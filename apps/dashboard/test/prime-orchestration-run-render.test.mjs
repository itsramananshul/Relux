// Render/DOM verification for the ORCHESTRATION result card as a real run control
// (PrimeTurnCard → OrchestrationResultCard; RELUX_MASTER_PLAN §10.4, §11.1, §17.1).
//
// The product contract this pins: when a Prime turn carries an orchestration, the card
// is the PRIMARY run path — it renders an explicit "Run orchestration" button that starts
// the existing non-blocking run-async job, deep-links each run/brief into a populated Work
// surface, and never auto-runs on render. The redundant "Run this orchestration"
// conversational chip is filtered out of the generic suggestion row so there is no
// confusing second run path.
//
// It transpiles the REAL component from Prime.tsx with the esbuild Vite already vendors and
// server-renders it through react-dom/server + react-router's StaticRouter — so a
// render-time throw fails here exactly as it would white-screen the chat. Under
// renderToStaticMarkup, useEffect never fires, so the mount-reconnect / poll make NO network
// calls and NOTHING is started — which is precisely the no-auto-run guarantee (§17.1): the
// resting card shows the Run button and "Nothing is running yet", never a live run. The
// live phase / terminal / refusal banners are driven by the pure `orchestration.ts` job
// helpers (jobPhaseLabel / jobProgressLabel / runButtonLabel), unit-tested in
// orchestration.test.ts; the run/cancel/reconnect branching (409 → reconnect, 429 → refuse)
// is logic this static harness cannot drive, so it is covered structurally + in the shipped
// bundle freshness check below.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-orchestration-run-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const pagesDir = join(dashboardRoot, "src", "pages");
const repoRoot = resolve(dashboardRoot, "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { PrimeTurnCard, OrchestrationResultCard } from "./Prime.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
const noop = () => {};

function step(taskId, agentId, role, outcome, runId) {
  return { task_id: taskId, agent_id: agentId, role, title: "Brief " + taskId, outcome, run_id: runId ?? null };
}
function orch(id, status, steps, notes) {
  return {
    id, goal: "research the options, build a prototype, and write the docs",
    created_by: "founder", namespace_id: "workspace", status, steps, notes: notes ?? [],
    created_at: "t0", updated_at: "t0",
  };
}

// A freshly-created (planned) orchestration: nothing has run, one brief fell back to Prime.
const PLANNED = orch("orch_0001", "planned", [
  step("task_0001", "research-agent", "research", "pending"),
  step("task_0002", "prime", "documentation", "pending"),
], ["No documentation agent on the roster; assigning to Prime. Hire one for a specialist."]);

// A turn that produced the planned orchestration AND carries the kernel's default
// suggested actions: the redundant "Run this orchestration" chip + a "Hire a …" pre-fill.
const TURN = {
  intent: "orchestrate_goal",
  reply: "I created orchestration orch_0001 with 2 briefs.",
  disposition: "executed",
  ai_mode: "deterministic",
  orchestration: PLANNED,
  suggested_actions: [
    { label: "Run this orchestration", message: "run orchestration orch_0001", send: true },
    { label: "Hire a documentation agent", message: "create a documentation agent", send: false },
  ],
  state: {},
};

// A partially-run record (a completed brief carries a real run id) — proves the per-brief
// run deep-link renders a populated /work?run=<id> target (contract D).
const PARTIAL = orch("orch_0002", "running", [
  step("task_0010", "research-agent", "research", "completed", "run_0007"),
  step("task_0011", "prime", "documentation", "pending"),
]);

export function renderTurn() {
  return at(<PrimeTurnCard turn={TURN} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderPlannedCard() {
  return at(<OrchestrationResultCard orchestration={PLANNED} />);
}
export function renderPartialCard() {
  return at(<OrchestrationResultCard orchestration={PARTIAL} />);
}
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "prime-orchestration-run-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-orch-run-render-"));
  const out = join(tmp, "prime-orchestration-run-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the result card renders an explicit Run orchestration button (the primary run path)", () => {
  const html = mod.renderPlannedCard();
  // The real, governed run control — a labelled button, not a text prompt (contract A).
  assert.match(html, /<button[^>]*>Run orchestration<\/button>/);
  // The honest resting state: nothing has started on render (contract E / §17.1).
  assert.match(html, /Nothing is running yet/);
  // It must NOT claim a live run before any click.
  assert.doesNotMatch(html, /Running —/);
});

test("the card deep-links briefs and runs into a populated Work surface (contract D)", () => {
  const html = mod.renderPartialCard();
  // A brief that produced a run deep-links to that run's detail on the Work board.
  assert.match(html, /\/work\?run=run_0007/);
  // Every brief deep-links to its task detail (never a blank /work).
  assert.match(html, /\/work\?task=task_0010/);
  // A "Track on Work board" link is always offered.
  assert.match(html, /Track on Work board/);
});

test("a pending-only planned card shows NO fabricated run link", () => {
  const html = mod.renderPlannedCard();
  // No brief has run yet, so there is no run deep-link to fabricate.
  assert.doesNotMatch(html, /\/work\?run=/);
});

test("the turn filters the redundant 'Run this orchestration' chip but keeps the Hire chip", () => {
  const html = mod.renderTurn();
  // The card owns the run, so the conversational duplicate chip is gone from the row.
  assert.doesNotMatch(html, /Run this orchestration/);
  // But the card's own primary button is present.
  assert.match(html, /Run orchestration/);
  // The non-run suggestion (hire a specialist) is untouched.
  assert.match(html, /Hire a documentation agent/);
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

test("the shipped JS bundle carries the orchestration run control (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // The run button copy + the honest resting footer + the over-cap refusal copy survive
  // minification (ASCII fragments). Their absence means the source gained the run control
  // but the committed bundle was never rebuilt — the exact "served UI is stale" failure.
  assert.match(bundle, /Run orchestration/);
  assert.match(bundle, /Nothing is running yet/);
  assert.match(bundle, /Too many orchestration runs are in flight/);
  assert.match(bundle, /Track on Work board/);
});
