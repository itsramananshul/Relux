// Render/DOM verification for the Plugins page's new "Create a tool-run task" form.
//
// A pure-function test pins the payload builder (toolruntask.test.ts), but only an
// actual render proves the Tools section mounts the form without a render-time throw
// (which would white-screen /plugins). This mirrors work-render.test.mjs: it
// transpiles the REAL `Plugins` page with esbuild and server-renders it through
// react-dom/server + react-router's StaticRouter — the SAME declarative-router
// context main.tsx uses. useEffect does not fire under renderToStaticMarkup, so
// `useAsync` never fetches; the first synchronous render (loading/empty state) is
// what we assert. The form heading lives OUTSIDE the loading branch, so it renders
// even with zero discovered tools.
//
// Run: `npm test` (auto-discovered) or `node --test test/plugins-render.test.mjs`.

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
      sourcefile: "plugins-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-plugins-render-"));
  const out = join(tmp, "plugins-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("Plugins RENDERS the tool-run-task form (no render-time throw)", () => {
  // If Plugins or the new CreateToolRunTask panel throws on mount, this call throws
  // and the test fails — exactly the reported blank page. It must render real markup
  // including the new form's heading.
  const html = render();
  assert.match(html, /Tools/);
  assert.match(html, /Create a tool-run task/);
});

test("the committed dashboard bundle carries the tool-run-task form (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII fragment survives minification; its absence means the source has the form
  // but the committed bundle was never rebuilt.
  assert.match(bundle, /Create a tool-run task/);
});
