// Render/DOM verification for the cross-Guild Inbox page (src/pages/Inbox.tsx).
//
// inbox.test.ts pins the pure helpers; this harness proves the REAL Inbox page
// MOUNTS under react-router's declarative StaticRouter with no mount-time throw and
// shows its honest pre-data state (effects, hence fetches, do not run under
// renderToStaticMarkup). It mirrors the other *-render.test.mjs: transpile the page
// with the esbuild Vite already vendors, server-render it, and assert the static
// chrome is present. It also asserts the COMMITTED bundle carries the page, so a
// stale dist fails loudly.
//
// Run: `npm test` (auto-discovered) or `node --test test/inbox-render.test.mjs`.

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
import { Inbox } from "./Inbox.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/inbox">
      <Inbox />
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
      sourcefile: "inbox-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-inbox-render-"));
  const out = join(tmp, "inbox-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("Inbox RENDERS under a declarative router (no mount throw)", () => {
  const html = render();
  // The page heading + operational intro render before any data resolves.
  assert.match(html, /Attention queue/);
  assert.match(html, /most urgent first/);
  // No data has loaded under SSR, so neither the empty-state nor any group is
  // claimed yet — the page is in its honest pre-fetch state with a Refresh control.
  assert.match(html, /Refresh/);
  // It must NOT fabricate an "all clear" or any item before the projection loads.
  assert.doesNotMatch(html, /Nothing needs you/);
});

test("the committed dashboard bundle carries the Inbox page", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // A distinctive Inbox string — absence means the source gained the page but the
  // committed bundle was never rebuilt (see docs note dashboard-dist-is-tracked).
  assert.match(bundle, /Attention queue/);
});
