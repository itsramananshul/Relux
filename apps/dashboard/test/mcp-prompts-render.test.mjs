// Render/DOM verification for the Plugins page's MCP **Prompts** panel (prompts/list
// + prompts/get, read-only templates).
//
// Mirrors plugins-render.test.mjs: it transpiles the REAL `Plugins` page with esbuild
// and server-renders it through react-dom/server + react-router's StaticRouter (the
// SAME declarative-router context main.tsx uses). useEffect does not fire under
// renderToStaticMarkup, so `useAsync` never fetches; the first synchronous render is
// what we assert — this proves the page mounts the new prompts code without a
// render-time throw (which would white-screen /plugins). The per-server Prompts panel
// only mounts after an operator expands a listed server, so its heading is asserted
// against the COMMITTED bundle instead (proving the rebuilt dist carries the new UI,
// not a stale one) — the same "no stale dist" guard plugins-render.test.mjs uses.
//
// Run: `npm test` (auto-discovered) or `node --test test/mcp-prompts-render.test.mjs`.

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
import { Plugins } from "./Plugins.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/plugins">
      <Plugins />
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
      sourcefile: "mcp-prompts-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-mcp-prompts-render-"));
  const out = join(tmp, "mcp-prompts-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("Plugins RENDERS with the prompts code mounted (no render-time throw)", () => {
  // If the Plugins page or the new prompts components throw on mount, this call
  // throws and the test fails — exactly the reported blank page.
  const html = render();
  assert.match(html, /Tools/);
});

test("the committed dashboard bundle carries the MCP prompts panel (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII fragments survive minification; their absence means the source has the
  // panel but the committed bundle was never rebuilt.
  assert.match(bundle, /Prompts \(read-only templates\)/);
  assert.match(bundle, /prompts\/get/);
});
