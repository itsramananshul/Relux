// Render verification for the cross-Guild Inbox ageing / SLA badges + the filter bar
// (src/pages/Inbox.tsx, docs/relix-dashboard-design.md §6.11 "triage SLAs / ageing").
//
// inbox.test.ts pins the pure bucket/filter logic; this proves the REAL InboxRow
// renders the right age badge per band (and the honest "age unavailable" when an item
// carries no anchor), and that the Inbox page renders its filter chips. It transpiles
// the real components with the esbuild Vite already vendors and server-renders them
// through react-router's StaticRouter (the declarative-router family the app uses), so
// a render-time throw fails here exactly as it would white-screen the page. onClick
// handlers are NOT fired (the wiring is covered by the unit + backend route tests).
//
// Run: `npm test` (auto-discovered) or `node --test test/inbox-age-render.test.mjs`.

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
import { InboxRow, Inbox } from "./Inbox.tsx";
function at(el, loc) {
  return renderToStaticMarkup(<StaticRouter location={loc || "/inbox"}>{el}</StaticRouter>);
}
const noop = () => {};

function failedRun(ageTicks) {
  return {
    id: "run:1",
    kind: "failed_run",
    severity: "warn",
    title: "Failed run run_0001",
    summary: "adapter_missing: the adapter is not installed",
    task_id: "task_0001",
    run_id: "run_0001",
    failure_class: "adapter_missing",
    attention_since: ageTicks == null ? undefined : "2026-06-08T00:00:00Z",
    age_ticks: ageTicks,
    actions: ["retry", "diagnose", "investigate", "inspect"],
    link: "/work",
  };
}

export function renderOverdue() { return at(<InboxRow item={failedRun(900)} onActed={noop} />); }
export function renderStale()   { return at(<InboxRow item={failedRun(300)} onActed={noop} />); }
export function renderFresh()   { return at(<InboxRow item={failedRun(5)} onActed={noop} />); }
export function renderNoAge()   { return at(<InboxRow item={failedRun(undefined)} onActed={noop} />); }
export function renderPage()    { return at(<Inbox />, "/inbox"); }
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "inbox-age-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-inbox-age-render-"));
  const out = join(tmp, "inbox-age-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("an overdue item renders the Overdue band with a tick count (not wall-clock)", () => {
  const html = mod.renderOverdue();
  assert.match(html, /Overdue/);
  assert.match(html, /900 ticks/);
  // The tooltip is explicit that this is logical-clock, never real time.
  assert.match(html, /logical clock/);
  assert.doesNotMatch(html, /seconds since|minutes ago/);
});

test("a stale item renders the Stale band", () => {
  assert.match(mod.renderStale(), /Stale/);
});

test("a fresh item renders the Fresh band", () => {
  const html = mod.renderFresh();
  assert.match(html, /Fresh/);
  assert.match(html, /5 ticks/);
});

test("an item with no anchor renders 'age unavailable', never a fabricated age", () => {
  const html = mod.renderNoAge();
  assert.match(html, /age unavailable/i);
  // No invented band words for an item we can't age.
  assert.doesNotMatch(html, /Overdue|Stale|\bFresh\b/);
});

test("the Inbox page renders the filter chips (All / Approvals / Overdue)", () => {
  const html = mod.renderPage();
  assert.match(html, /All/);
  assert.match(html, /Approvals/);
  assert.match(html, /Failed runs/);
  assert.match(html, /Overdue/);
});
