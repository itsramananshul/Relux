// Render/DOM verification for the Work page (the reported blank-page bug).
//
// Work was reported blank like Crew was. Work.tsx does NOT use a data-router-only
// hook, but a render-time throw anywhere in the page (or a child panel) would
// white-screen the /work route just the same. A pure-function test cannot catch
// that; only actually rendering the component does.
//
// This mirrors crew-render.test.mjs: it transpiles the REAL `Work` with the
// esbuild Vite already vendors, then server-renders it through react-dom/server +
// react-router's StaticRouter — the SAME declarative-router context main.tsx uses.
// If Work (or anything it renders synchronously) ever throws on mount, this render
// throws and the test fails — exactly the blank page. The shipped-bundle path then
// asserts the committed dist actually carries the Work copy (catches a stale dist).
//
// Run: `npm test` (auto-discovered) or `node --test test/work-render.test.mjs`.

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

// Render <Work/> inside a StaticRouter (Work uses <Link> / router hooks, so it
// needs router context). The router is the DECLARATIVE StaticRouter — the same
// family as the app's <BrowserRouter> — so any render-time throw here mirrors
// production. useEffect does not fire under renderToStaticMarkup, so `useAsync`
// never calls fetch; the first synchronous render (the loading/empty state) is
// what we assert.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { Work } from "./Work.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <Work />
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
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "work-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-render-"));
  const out = join(tmp, "work-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("Work RENDERS under a declarative router (no render-time throw)", () => {
  // If Work or any child panel throws on mount, this call throws and the test
  // fails — exactly the reported blank page. It must render real markup.
  const html = render();
  // The page header and create affordance are always present (outside the
  // error/loading branches), so the surface is useful even with zero tasks/runs.
  assert.match(html, /Work/);
  assert.match(html, /Create a new task/);
  // The board and the runs section render in the same view.
  assert.match(html, /Open/);
  assert.match(html, /Recent Runs/);
  // Board Oversight v1: the oversight strip and the now-rendered Blocked/Failed
  // column are part of the first paint (the strip shows its loading state until
  // the effect fetches, which never fires under renderToStaticMarkup).
  assert.match(html, /Oversight/);
  assert.match(html, /Blocked \/ Failed/);
  // Work hierarchy/progress v1: the Work groups section renders on first paint.
  // No effect fires under renderToStaticMarkup, so the orchestration read is still
  // null with no error → the honest loading state (never a fabricated group). The
  // populated parent+children render is asserted in work-hierarchy-render.test.mjs.
  assert.match(html, /Work groups/);
  assert.match(html, /Loading work groups/);
});

test("the committed dashboard bundle carries the Work copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII fragments survive minification; their absence means the source has the
  // Work page but the committed bundle was never rebuilt.
  assert.match(bundle, /Create a new task/);
  assert.match(bundle, /Recent Runs/);
});

test("the committed bundle points the rail's Work entry at /work", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  assert.match(bundle, /"\/work"[^}]*"Work"|"Work"[^}]*"\/work"/);
});
