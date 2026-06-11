// Render/DOM verification for the Relux Home readiness guide.
//
// A pure-function test (readiness.test.ts) pins the derivation, but only actually
// rendering Home catches a mount-time throw (e.g. a data-router-only hook, a bad
// import, a null-deref on the loading report). This harness mirrors
// crew-render.test.mjs: it transpiles the REAL ReluxHome with the esbuild Vite
// already vendors, server-renders it under react-router's declarative
// StaticRouter (the same family as the app's <BrowserRouter>), and asserts the
// readiness card renders even before any data has loaded (effects, hence fetches,
// do not run under renderToStaticMarkup). It also asserts the COMMITTED bundle the
// kernel serves carries the new copy, so a stale dist fails loudly.
//
// Run: `npm test` (auto-discovered) or `node --test test/readiness-render.test.mjs`.

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

// ── Render path: transpile + server-render the real ReluxHome page ──────────

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { ReluxHome } from "./ReluxHome.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/">
      <ReluxHome />
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
      sourcefile: "readiness-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-readiness-render-"));
  const out = join(tmp, "readiness-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("ReluxHome RENDERS under a declarative router (no mount throw)", () => {
  const html = render();
  // The product framing is always present.
  assert.match(html, /local control plane/);
  // The readiness card renders even before data loads — its honest loading state,
  // never a blank node.
  assert.match(html, /Readiness/);
  assert.match(html, /Checking readiness/);
});

test("ReluxHome no longer renders the redundant 'Run real work' instructional card", () => {
  const html = render();
  // The old prose card duplicated the readiness guide's brain + real-work-adapter
  // items; the guide replaced it, so Home must not carry the stale instructions.
  assert.doesNotMatch(html, /Run real work/);
  // The capability is still reachable: the product framing links to Crew, and the
  // readiness guide's real-work-adapter item links there too.
  assert.match(html, /Manage crew/);
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

test("the committed dashboard bundle carries the readiness copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII fragments survive minification; their absence means the source gained
  // the readiness guide but the committed bundle was never rebuilt.
  assert.match(bundle, /Checking readiness/);
  assert.match(bundle, /setup needed/);
  assert.match(bundle, /First action/);
  // The honest local-Prime fallback copy is part of the new derivation.
  assert.match(bundle, /built-in operative/);
});
