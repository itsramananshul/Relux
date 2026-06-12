// Render verification for the cross-Guild Inbox search box + the search-driven
// results/empty rendering (src/pages/Inbox.tsx, docs/relix-dashboard-design.md
// §6.11 "cross-project search of the queue").
//
// inbox.test.ts pins the pure search/filter logic; this proves the REAL page
// renders the search input (and pre-fills + offers Clear from a `?q=` URL), and
// that the REAL InboxRow renders exactly the rows the real `searchInbox` keeps —
// both a hit (results) and a miss (nothing rendered). It transpiles the real
// components with the esbuild Vite already vendors and server-renders them through
// react-router's StaticRouter (the declarative-router family the app uses), so a
// render-time throw fails here exactly as it would white-screen the page. onChange
// handlers are NOT fired (the URL wiring is covered by reading `?q=` here, and the
// pure search by inbox.test.ts).
//
// Run: `npm test` (auto-discovered) or `node --test test/inbox-search-render.test.mjs`.

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
import { searchInbox } from "../inbox.ts";
function at(el, loc) {
  return renderToStaticMarkup(<StaticRouter location={loc || "/inbox"}>{el}</StaticRouter>);
}
const noop = () => {};

function failedRun(over) {
  return {
    id: "run:1",
    kind: "failed_run",
    severity: "warn",
    title: "Failed run",
    summary: "a run failed",
    task_id: "task_0001",
    run_id: "run_0001",
    failure_class: "adapter_missing",
    age_ticks: 5,
    actions: ["retry", "diagnose", "investigate", "inspect"],
    link: "/work",
    ...over,
  };
}

// Two items with distinct, searchable titles/classes so search can pick one.
const ADAPTER = failedRun({ id: "run:adapter", title: "Adapter went missing", failure_class: "adapter_missing", run_id: "run_adapter" });
const AUTH = failedRun({ id: "run:auth", title: "Login needs sign-in", failure_class: "auth_required", run_id: "run_auth" });
const ITEMS = [ADAPTER, AUTH];

function rows(query) {
  return at(<>{searchInbox(ITEMS, query).map((it) => <InboxRow key={it.id} item={it} onActed={noop} />)}</>);
}

export function renderPage()        { return at(<Inbox />, "/inbox"); }
export function renderPageQueried()  { return at(<Inbox />, "/inbox?q=adapter"); }
export function renderResults()      { return rows("auth"); }   // keeps only AUTH
export function renderAllResults()   { return rows(""); }       // empty query → both
export function renderEmptyResults() { return rows("nonesuch"); } // keeps nothing
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "inbox-search-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-inbox-search-render-"));
  const out = join(tmp, "inbox-search-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the Inbox page renders the search box (labeled input + placeholder), no Clear without a query", () => {
  const html = mod.renderPage();
  assert.match(html, /aria-label="Search the attention queue"/);
  assert.match(html, /placeholder="Search the queue/);
  // No active query → no Clear button.
  assert.doesNotMatch(html, />Clear</);
});

test("a `?q=` URL pre-fills the search box and offers Clear", () => {
  const html = mod.renderPageQueried();
  // The input is controlled from the URL query.
  assert.match(html, /value="adapter"/);
  assert.match(html, />Clear</);
});

test("search renders only the matching rows (results)", () => {
  const html = mod.renderResults();
  assert.match(html, /Login needs sign-in/);
  assert.doesNotMatch(html, /Adapter went missing/);
});

test("an empty query renders every row", () => {
  const html = mod.renderAllResults();
  assert.match(html, /Adapter went missing/);
  assert.match(html, /Login needs sign-in/);
});

test("a non-matching query renders no rows (empty)", () => {
  const html = mod.renderEmptyResults();
  assert.doesNotMatch(html, /Adapter went missing/);
  assert.doesNotMatch(html, /Login needs sign-in/);
});
