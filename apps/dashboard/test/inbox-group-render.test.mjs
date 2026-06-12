// Render verification for the cross-Guild Inbox cross-item GROUPING (src/pages/Inbox.tsx
// + the pure src/inbox.ts, docs/relix-dashboard-design.md §6.11 "cross-item grouping —
// collapsing a whole stalled subtree into one card").
//
// inbox.test.ts pins the pure grouping logic; this proves the REAL components render:
//   - the page exposes the Group toggle (default grouped);
//   - the real SubtreeGroupCard collapses a stalled subtree into one header (root title,
//     worst-severity badge, oldest-age badge, per-kind counts) and, when expanded, lays
//     out each member's row WITH its actions (actions are never hidden permanently);
//   - a standalone (unrelated) item renders as its own row, never folded into a group.
// It transpiles the real components with the esbuild Vite vendors and server-renders them
// through react-router's StaticRouter, so a render-time throw fails here as it would
// white-screen the page.
//
// Run: `npm test` (auto-discovered) or `node --test test/inbox-group-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const pagesDir = join(dashboardRoot, "src", "pages");

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { Inbox, SubtreeGroupCard, InboxRow } from "./Inbox.tsx";
import { buildInboxGroups } from "../inbox.ts";
function at(el, loc) {
  return renderToStaticMarkup(<StaticRouter location={loc || "/inbox"}>{el}</StaticRouter>);
}
const noop = () => {};

// A stalled subtree: a blocked root, a CRITICAL failed-run child (worst severity), and a
// blocked child that has waited the longest (oldest age) — all under tk-root.
const ROOT = {
  id: "task:tk-root", kind: "blocked_task", severity: "warn", title: "Blocked: Ship the launch",
  summary: "On hold — reopen to put it back in the run lifecycle.",
  task_id: "tk-root", age_ticks: 50, actions: ["reopen_and_run", "reopen", "investigate", "inspect"], link: "/work",
};
const CHILD_FAILED = {
  id: "run:1", kind: "failed_run", severity: "critical", title: "Failed run 1",
  summary: "adapter_missing: the CLI is not installed", task_id: "tk-child-a", parent_task: "tk-root",
  failure_class: "adapter_missing", age_ticks: 10, actions: ["retry", "diagnose", "investigate", "inspect"], link: "/work",
};
const CHILD_BLOCKED = {
  id: "task:tk-child-b", kind: "blocked_task", severity: "warn", title: "Blocked: Wire the gateway",
  summary: "On hold — last run failed (auth_required).", task_id: "tk-child-b", parent_task: "tk-root",
  run_id: "run_2", failure_class: "auth_required", age_ticks: 999,
  actions: ["reopen_and_run", "reopen", "diagnose", "investigate", "inspect"], link: "/work",
};
const UNRELATED = {
  id: "approval:1", kind: "pending_approval", severity: "info", title: "Approval: promote to prod",
  summary: "needs sign-off", approval_id: "appr_1", age_ticks: 5, actions: ["open_approval"], link: "/approvals",
};

const SUBTREE = buildInboxGroups([ROOT, CHILD_FAILED, CHILD_BLOCKED])[0];

export function renderPage()           { return at(<Inbox />, "/inbox"); }
export function renderCollapsed()      { return at(<SubtreeGroupCard group={SUBTREE} onActed={noop} />); }
export function renderExpanded()       { return at(<SubtreeGroupCard group={SUBTREE} onActed={noop} defaultOpen={true} />); }
export function renderStandalone()     { return at(<InboxRow item={UNRELATED} onActed={noop} />); }
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "inbox-group-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-inbox-group-render-"));
  const out = join(tmp, "inbox-group-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the Inbox page renders the Group toggle (grouping is the default)", () => {
  const html = mod.renderPage();
  // The default-on toggle reads "Grouped"; aria-pressed marks it active.
  assert.match(html, />Grouped</);
  assert.match(html, /aria-pressed="true"/);
});

test("a collapsed subtree card summarizes the whole subtree without listing members", () => {
  const html = mod.renderCollapsed();
  // The card is titled from the root task (Blocked: prefix stripped).
  assert.match(html, /Ship the launch/);
  // The WORST member severity (critical) leads the header, not the root's own warn.
  assert.match(html, /Critical/);
  // Per-kind rollup counts are shown (2 blocked + 1 failed).
  assert.match(html, /2 blocked work/);
  assert.match(html, /1 failed runs/);
  // The member rows are NOT rendered while collapsed (their summaries are hidden).
  assert.doesNotMatch(html, /adapter_missing: the CLI is not installed/);
  // The header is an expandable control (collapsed → aria-expanded false).
  assert.match(html, /aria-expanded="false"/);
});

test("expanding the subtree lays out every member row WITH its actions", () => {
  const html = mod.renderExpanded();
  assert.match(html, /aria-expanded="true"/);
  // Each member's row + summary is now present.
  assert.match(html, /adapter_missing: the CLI is not installed/);
  assert.match(html, /Wire the gateway/);
  // The members' action buttons are present — actions are never hidden permanently.
  assert.match(html, />Retry</);
  assert.match(html, />Analyze failure</);
  assert.match(html, />Reopen &amp; run</); // ampersand is HTML-escaped in the markup
});

test("an unrelated item renders as its own standalone row, never folded into a group", () => {
  const html = mod.renderStandalone();
  assert.match(html, /promote to prod/);
  // It is a plain attention row (its open-approval affordance), not a subtree header.
  assert.doesNotMatch(html, /aria-expanded/);
});
