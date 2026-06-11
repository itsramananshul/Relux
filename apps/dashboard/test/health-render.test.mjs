// Render/DOM verification for the Relux Health page's readiness integration.
//
// readiness.test.ts pins the pure derivation; this harness proves the Health page
// actually MOUNTS the shared ReadinessGuide (no mount-time throw, no duplicated
// logic) and degrades honestly. It mirrors readiness-render.test.mjs: transpile
// the REAL Health page with the esbuild Vite already vendors, server-render it
// under react-router's declarative StaticRouter, and assert the readiness card is
// present even before any data has loaded (effects, hence fetches, do not run
// under renderToStaticMarkup — so the page is in its honest loading state). It
// also asserts the COMMITTED bundle carries the integration, so a stale dist fails
// loudly.
//
// Run: `npm test` (auto-discovered) or `node --test test/health-render.test.mjs`.

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

// ── Render path: transpile + server-render the real Health page ─────────────

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { Health } from "./Health.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/health">
      <Health />
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
      sourcefile: "health-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-health-render-"));
  const out = join(tmp, "health-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("Health RENDERS under a declarative router (no mount throw)", () => {
  const html = render();
  // The shared readiness guide mounts at the top of the page (reused, not
  // re-implemented) — its honest loading state before any data resolves.
  assert.match(html, /Readiness/);
  assert.match(html, /Checking readiness/);
  // The page's own loading affordance is still shown honestly while data loads,
  // never a faked "ready" health summary.
  assert.match(html, /Loading health status/);
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

test("the committed dashboard bundle carries the Health readiness integration", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // The Health page's own copy and the shared readiness copy both ship — absence
  // means the source gained the integration but the committed bundle was never
  // rebuilt.
  assert.match(bundle, /Relux Health Status/);
  assert.match(bundle, /Checking readiness/);
});
