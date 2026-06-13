// Render/DOM verification for the Crew page (the reported blank-page bug).
//
// Root cause: Crew.tsx called react-router's `useLoaderData()`, but the SPA
// mounts under a plain <BrowserRouter> (a DECLARATIVE router, not a data router
// built with createBrowserRouter). `useLoaderData()` outside a data router throws
// "useLoaderData must be used within a data router" on mount — an uncaught render
// error that white-screened the entire /crew route. A pure-function test cannot
// catch that; only actually rendering the component does.
//
// This harness closes the gap WITHOUT a browser and WITHOUT new dependencies:
//   1. Render path — it transpiles the REAL `Crew` with the esbuild already
//      vendored by Vite, then server-renders it through react-dom/server +
//      react-router's StaticRouter (the SAME declarative-router context main.tsx
//      uses, NOT a data router). If Crew ever reintroduces a data-router-only
//      hook, this render throws and the test fails — exactly the blank page.
//   2. Shipped-bundle path — it reads the COMMITTED bundle the kernel serves and
//      asserts the Crew copy is present (catches a stale dist) and that the rail
//      points "Crew" at /crew (not the legacy /agents console).
//
// Run: `npm test` (auto-discovered) or `node --test test/crew-render.test.mjs`.

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

// ── Render path: transpile + server-render the real Crew page ───────────────

// A tiny entry that renders <Crew/> inside a StaticRouter (Crew uses <Link>, so
// it needs router context) and returns the static markup. Crucially the router
// is the DECLARATIVE StaticRouter — the same family as the app's <BrowserRouter>
// — so a data-router-only hook (useLoaderData) throwing here mirrors production.
// useEffect does not fire under renderToStaticMarkup, so `useAsync` never calls
// fetch; the first synchronous render (the loading state) is what we assert.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { Crew } from "./Crew.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/crew">
      <Crew />
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
      sourcefile: "crew-render-entry.tsx",
    },
    bundle: true,
    // CJS so the bundled output keeps native `require` for node builtins
    // (react-dom/server dynamic-requires "stream"); a single bundled React copy
    // is shared by the component, react-dom/server, and react-router so hooks
    // stay consistent.
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-crew-render-"));
  const out = join(tmp, "crew-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("Crew RENDERS under a declarative router (no useLoaderData throw)", () => {
  // The regression: this call threw "useLoaderData must be used within a data
  // router" and blanked the route. It must now render real markup.
  const html = render();
  assert.match(html, /Your Crew/);
  // The first synchronous render is the honest loading state (effects, hence the
  // data fetch, have not run yet) — never a blank node.
  assert.match(html, /Loading your crew/);
  // The create-member affordance is part of the same view, so the page is useful
  // even with zero agents.
  assert.match(html, /Create New Crew Member/);
  // The Prime Brain section is mounted on Crew (the doc + recovery card name
  // "Crew → Prime Brain"); its heading + the shared panel render synchronously, so
  // the page always carries the brain-setup surface — never a brainless Crew.
  assert.match(html, /Prime Brain/);
  assert.match(html, /Prime Brain \/ AI Runtime/);
});

test("the create form exposes the name, persona, and adapter fields", () => {
  // The manual Crew config workflow: an operator must be able to set a name, an
  // optional persona (operating style), and pick an adapter/runtime. These render
  // even before any data loads (the create section is always shown), so the page
  // is usable on first paint.
  const html = render();
  assert.match(html, /Persona \(operating style/);
  assert.match(html, /Adapter \/ Runtime/);
  // The adapter picker always offers the safe local-Prime default option.
  assert.match(html, /Default \(local Prime\)/);
  // The skills/tags field is part of the same create section.
  assert.match(html, /Skills \/ Tags \(comma-separated/);
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

test("the committed dashboard bundle carries the Crew copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII fragments survive minification; their absence means the source gained
  // the Crew fix but the committed bundle was never rebuilt.
  assert.match(bundle, /Your Crew/);
  assert.match(bundle, /Loading your crew/);
  assert.match(bundle, /Create New Crew Member/);
  // The Prime Brain section copy must ship too (catches a stale dist after the
  // brain-on-Crew slice landed in source).
  assert.match(bundle, /Prime Brain \/ AI Runtime/);
  // The skills/tags field copy must be in the shipped bundle (catches a stale dist
  // after the skills slice landed in source).
  assert.match(bundle, /Skills \/ Tags \(comma-separated/);
  // The role-preset selector copy must ship too (catches a stale dist after the
  // role-preset slice landed in source). The selector only renders once presets load
  // (an effect), so the render path above can't assert it — the bundle path does.
  assert.match(bundle, /Role preset \(optional\)/);
});

test("the committed bundle points the rail's Crew entry at /crew, not legacy /agents", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // The rail entry is `{to:"/crew",label:"Crew",...}`. Assert the label+/crew
  // pairing is present (the fix) so the real Crew page is reachable from the rail.
  assert.match(bundle, /"\/crew"[^}]*"Crew"|"Crew"[^}]*"\/crew"/);
});
