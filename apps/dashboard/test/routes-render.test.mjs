// Render/DOM smoke for the Relux shell routes that had NO render test:
// ReluxHome (/), Prime (/prime), and ReluxApprovals (/approvals).
//
// Why this exists: a pure-function test cannot catch a render-time throw (a
// data-router-only hook, a bad destructure on first paint, a missing context).
// Only actually server-rendering the component under the SAME declarative router
// the app uses (StaticRouter — the family of <BrowserRouter>, NOT a data router)
// reproduces the reported blank pages. useEffect does not fire under
// renderToStaticMarkup, so `useAsync`/fetch never runs; the first synchronous
// render (loading/empty state) is what we assert — exactly what a user sees on
// first paint before any data arrives.
//
// This also pins the ReluxApprovals B&W-design-system fix: the page was written
// entirely in Tailwind utility classes (bg-gray-800 / text-white / text-gray-*),
// which this project does not ship — so it rendered unstyled and off-aesthetic.
// The render here asserts the page now uses the shared `card` chrome and carries
// no stray Tailwind class.
//
// Run: `npm test` (auto-discovered) or `node --test test/routes-render.test.mjs`.

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

// Each page is server-rendered inside a declarative StaticRouter at its real
// path. ReluxApprovals is a default export; ReluxHome and Prime are named.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { ReluxHome } from "./ReluxHome.tsx";
import { Prime } from "./Prime.tsx";
import ReluxApprovals from "./ReluxApprovals.tsx";
function at(loc, el) {
  return renderToStaticMarkup(<StaticRouter location={loc}>{el}</StaticRouter>);
}
export function renderHome() { return at("/", <ReluxHome />); }
export function renderPrime() { return at("/prime", <Prime />); }
export function renderApprovals() { return at("/approvals", <ReluxApprovals />); }
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "routes-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-routes-render-"));
  const out = join(tmp, "routes-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("ReluxHome renders under a declarative router (no blank page)", () => {
  const html = mod.renderHome();
  assert.match(html, /local control plane/);
  // The primary calls-to-action are present on first paint.
  assert.match(html, /Talk to Prime/);
});

test("Prime renders under a declarative router (chat-first, no blank page)", () => {
  const html = mod.renderPrime();
  // The greeting and input placeholder paint immediately (chat is the page).
  assert.match(html, /chat-log/);
  // The advanced controls are collapsed in a <details> BELOW the input, never
  // pushing the chat down (RELUX_MASTER_PLAN §11.1).
  assert.match(html, /Advanced/);
  assert.match(html, /<details/);
  // The "Prime abilities" inventory panel paints on first render (open, lazy-loaded)
  // so installed/MCP tools are a visible capability (docs/prime-tool-use.md).
  assert.match(html, /Prime abilities/);
});

test("ReluxApprovals renders and uses the B&W design system, not Tailwind", () => {
  const html = mod.renderApprovals();
  assert.match(html, /Approvals &amp; Permissions|Approvals/);
  // It must render the shared `card` chrome (the design system), proving the
  // first synchronous (loading) paint does not throw.
  assert.match(html, /class="card"/);
  // Regression guard: no stray Tailwind utility classes leak back in (the bug
  // that left this page unstyled). A few representative tokens are enough.
  assert.doesNotMatch(html, /bg-gray-\d/);
  assert.doesNotMatch(html, /text-gray-\d/);
  assert.doesNotMatch(html, /rounded-lg/);
});
