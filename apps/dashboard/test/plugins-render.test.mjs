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
import { Plugins, PrimeCanUseSection } from "./Plugins.tsx";
export function render() {
  return renderToStaticMarkup(
    <StaticRouter location="/plugins">
      <Plugins />
    </StaticRouter>
  );
}
const wrapperPlugin = {
  id: "acme-repo", name: "Acme", description: "", kind: "ToolSet", version: "0.1.0",
  enabled: true, source_kind: "Github", source_label: "https://github.com/acme/acme-repo",
  install_dir: "/data/acme-repo", protected: false, bundled: false, generated: true, tool_count: 0,
};
const bundledPlugin = { ...wrapperPlugin, id: "relux-tools-echo", protected: true, bundled: true, generated: false };
export function renderPrimeCanUse(p) {
  return renderToStaticMarkup(
    <StaticRouter location="/plugins">
      <PrimeCanUseSection plugin={p} isWrapper={p.generated} />
    </StaticRouter>
  );
}
export const fixtures = { wrapperPlugin, bundledPlugin };
`;

let tmp = null;
let render = null;
let renderPrimeCanUse = null;
let fixtures = null;

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
  ({ render, renderPrimeCanUse, fixtures } = await import(pathToFileURL(out).href));
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

test("Plugins RENDERS the 'no manifest needed' install guidance, never 'manifest required'", () => {
  // The reported UX bug: the install surface read as if an external repo NEEDS a
  // relux-plugin.json. The always-visible intro copy (outside the loading branch)
  // must say the opposite — no manifest needed, the file is optional — so an
  // operator can never misread it. This asserts the page-level guidance directly.
  const html = render();
  assert.match(html, /No Relux manifest needed/i);
  assert.match(html, /optional/i);
  // It must NOT frame the manifest as a requirement.
  assert.doesNotMatch(html, /manifest (is )?required/i);
});

test("PrimeCanUseSection RENDERS the read-only Plugin Lens capabilities for a non-bundled plugin", () => {
  // The product contract: an installed plugin shows what Prime can use with it. The
  // section lists the four read-only source tools and offers a "Summarize with Prime"
  // action — and must render without a throw (it uses useNavigate, so needs Router ctx).
  const html = renderPrimeCanUse(fixtures.wrapperPlugin);
  assert.match(html, /Prime can use/i);
  assert.match(html, /plugin\.summary/);
  assert.match(html, /plugin\.read_file/);
  assert.match(html, /Summarize with Prime/);
});

test("PrimeCanUseSection renders nothing for a bundled/protected fixture", () => {
  // Bundled fixtures already ship known capabilities, so the Lens section is hidden.
  const html = renderPrimeCanUse(fixtures.bundledPlugin);
  assert.equal(html, "");
});

test("the committed dashboard bundle carries the tool-run-task form (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // ASCII fragment survives minification; its absence means the source has the form
  // but the committed bundle was never rebuilt.
  assert.match(bundle, /Create a tool-run task/);
});

test("the committed dashboard bundle carries the 'no manifest needed' copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // If this fails, the source has the blunt manifest-optional copy but the tracked
  // bundle was never rebuilt — the shipped UI would still read the old way.
  assert.match(bundle, /No Relux manifest needed/i);
  assert.match(bundle, /Install any GitHub repo/i);
});

test("the committed dashboard bundle carries the 'Prime can use' Plugin Lens section (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  // The Plugin Lens UI must be in the shipped bundle, not just the source.
  assert.match(bundle, /Prime can use/i);
  assert.match(bundle, /Summarize with Prime/i);
});
