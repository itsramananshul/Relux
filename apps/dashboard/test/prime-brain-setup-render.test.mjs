// Render/DOM verification for the Prime Brain setup panel (PrimeBrainPanel).
//
// The backend probe behaviour is pinned by Rust unit tests (ai.rs / adapter.rs);
// this harness proves the setup UI MOUNTS and presents the product-grade first-run
// choices on first paint: the three recommended real-brain paths, the Local
// fallback clearly labelled as test plumbing (not the product chat path), and a
// safe per-brain "Test" action. Effects (hence fetches) do not run under
// renderToStaticMarkup, so this is the honest pre-data state every user sees first.
//
// It also asserts the COMMITTED bundle carries the labelling, so a stale dist
// (source updated but bundle never rebuilt) fails loudly.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-brain-setup-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const componentsDir = join(dashboardRoot, "src", "components");
const repoRoot = resolve(dashboardRoot, "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { PrimeBrainPanel } from "./PrimeBrainPanel.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/health">
      <PrimeBrainPanel />
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
      resolveDir: componentsDir,
      loader: "tsx",
      sourcefile: "prime-brain-setup-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-brain-render-"));
  const out = join(tmp, "prime-brain-setup-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the setup panel mounts with the four brains and the runtime header", () => {
  const html = render();
  assert.match(html, /Prime Brain \/ AI Runtime/);
  assert.match(html, /Claude CLI/);
  assert.match(html, /Codex CLI/);
  assert.match(html, /OpenRouter/);
  assert.match(html, /Local \(deterministic\)/);
});

test("real brains are marked recommended and Local is labelled fallback/test", () => {
  const html = render();
  // The three real conversational paths carry a "recommended" chip...
  assert.match(html, /recommended/);
  // ...and the Local deterministic brain is plainly flagged as fallback/test
  // plumbing, so it never reads as the main product path.
  assert.match(html, /fallback \/ test/);
});

test("every brain exposes both the safe quick probe and the live chat test", () => {
  const html = render();
  // The read-only quick probe is the discoverable "is this usable?" action...
  assert.match(html, /Quick probe/);
  // ...and the explicit live chat test sits right beside it.
  assert.match(html, /Test live chat/);
});

test("the live chat test carries an explicit deliberate-action warning", () => {
  const html = render();
  // The panel must make it obvious the live test is explicit and may incur real
  // provider usage — never a silent or automatic billable call.
  assert.match(html, /only when you click it/);
  assert.match(html, /may use the real provider \/ CLI and may incur provider usage/);
  // And it must reassure that it creates no task or run (setup diagnostic only).
  assert.match(html, /never[\s\S]*creates a task or run/);
});

test("the committed dashboard bundle carries the Prime Brain setup labelling", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // Absence means the source gained the setup labelling but the committed bundle
  // was never rebuilt (a stale dist the kernel would actually serve).
  assert.match(bundle, /Prime Brain \/ AI Runtime/);
  assert.match(bundle, /fallback \/ test/);
  // The live chat probe + its warning must be in the shipped bundle too.
  assert.match(bundle, /Test live chat/);
  assert.match(bundle, /may incur provider usage/);
});
