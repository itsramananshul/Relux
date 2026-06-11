// Render/DOM verification for the ReadinessGuide component's three honest modes:
// LOADING (no report yet → "Checking readiness…"), DEGRADED (a read failed → an
// explicit "… unavailable" row with a Retry and a "degraded" badge, never a faked
// "operational"), and OPERATIONAL (all reads in → the summary, no checking text).
//
// readiness.test.ts pins the pure derivation; this proves the component actually
// renders the distinction — so a regression that re-collapses "failed" back into
// the indefinite loading text (the bug this slice fixes) fails loudly. It mirrors
// the other render harnesses: transpile the REAL component with the esbuild Vite
// vendors and server-render it under react-router's declarative StaticRouter.
//
// Run: `npm test` (auto-discovered) or `node --test test/readiness-guide-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const componentsDir = join(dashboardRoot, "src", "components");

// Three hand-built reports exercise the three modes the component must render
// differently. Shapes mirror ReadinessReport (only the fields the component reads).
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { ReadinessGuide } from "./ReadinessGuide.tsx";

const firstAction = { label: "Talk to Prime", linkTo: "/prime" };

const degraded = {
  ready: false,
  degraded: true,
  summary: "unused while degraded",
  firstAction,
  blockers: [],
  attention: [],
  items: [
    {
      id: "state-unavailable",
      label: "State unavailable",
      status: "warn",
      description: "Could not read live state from the control plane, so the readiness below is partial.",
      linkTo: "/health",
      cta: "Open Health",
      retry: true,
    },
    {
      id: "prime-brain",
      label: "Connect Prime to a brain",
      status: "link",
      description: "Choose who answers Prime.",
      linkTo: "/health",
    },
  ],
};

const operational = {
  ready: true,
  degraded: false,
  summary: "Brain: Local (deterministic). 0 agents, 0 tools ready. 0 open tasks, 0 running.",
  firstAction,
  blockers: [],
  attention: [],
  items: [
    { id: "prime-brain", label: "Prime brain", status: "done", description: "ok", linkTo: "/health" },
  ],
};

export function renderLoading() {
  return renderToStaticMarkup(
    <StaticRouter location="/"><ReadinessGuide report={null} loading={true} onRefresh={() => {}} /></StaticRouter>
  );
}
export function renderDegraded() {
  return renderToStaticMarkup(
    <StaticRouter location="/"><ReadinessGuide report={degraded} loading={false} onRefresh={() => {}} /></StaticRouter>
  );
}
export function renderOperational() {
  return renderToStaticMarkup(
    <StaticRouter location="/"><ReadinessGuide report={operational} loading={false} onRefresh={() => {}} /></StaticRouter>
  );
}
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: componentsDir,
      loader: "tsx",
      sourcefile: "readiness-guide-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-readiness-guide-render-"));
  const out = join(tmp, "readiness-guide-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("LOADING: no report yet renders the honest checking text, no failure copy", () => {
  const html = mod.renderLoading();
  assert.match(html, /Checking readiness/);
  assert.doesNotMatch(html, /State unavailable/);
  assert.doesNotMatch(html, /degraded/);
});

test("DEGRADED: a failed read renders an explicit unavailable row + Retry + badge", () => {
  const html = mod.renderDegraded();
  // The explicit row, NOT the indefinite checking text.
  assert.doesNotMatch(html, /Checking readiness/);
  assert.match(html, /State unavailable/);
  // A per-row Retry affordance is wired from the row's `retry` flag.
  assert.match(html, /Retry/);
  // The badge is honest — "degraded", never "operational".
  assert.match(html, /degraded/);
  assert.doesNotMatch(html, /operational/);
  // The degraded banner explains the partial state.
  assert.match(html, /Showing what is available/);
});

test("OPERATIONAL: all reads in renders the summary, no checking/degraded text", () => {
  const html = mod.renderOperational();
  assert.doesNotMatch(html, /Checking readiness/);
  assert.doesNotMatch(html, /degraded/);
  assert.match(html, /operational/);
  assert.match(html, /Set up —/);
});
