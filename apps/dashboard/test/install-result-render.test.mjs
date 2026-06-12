// Render/DOM verification for the post-install result card (InstallResultCard).
//
// A pure-function test pins the install summary + URL normalization (plugins.test.ts),
// but only an actual render proves the result card mounts its LIVE next-action buttons
// without a render-time throw — and that, for a generated metadata-only wrapper, the
// configure+hints path auto-opens so the next step is immediate. This mirrors
// plugins-render.test.mjs: it transpiles the REAL component with esbuild and
// server-renders it through react-dom/server + react-router's StaticRouter (the same
// declarative-router context main.tsx uses). useEffect does not fire under
// renderToStaticMarkup, so `useAsync` never fetches; the first synchronous render is
// what we assert.
//
// Run: `npm test` (auto-discovered) or `node --test test/install-result-render.test.mjs`.

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

// Three honest install outcomes, shaped like the real ReluxPlugin.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { InstallResultCard } from "./Plugins.tsx";

const base = {
  id: "relux-plugin-hermes-agent",
  name: "hermes-agent",
  description: "",
  kind: "ToolSet",
  version: "0.1.0",
  enabled: true,
  source_kind: "Github",
  source_label: "https://github.com/nousresearch/hermes-agent",
  install_dir: "/data/relux/plugins/relux-plugin-hermes-agent",
  protected: false,
  bundled: false,
  generated: true,
  tool_count: 0,
};

function card(over) {
  const noop = () => {};
  return renderToStaticMarkup(
    <StaticRouter location="/plugins">
      <InstallResultCard
        result={{ ...base, ...over }}
        onChanged={noop}
        onInstallAnother={noop}
        onClose={noop}
      />
    </StaticRouter>
  );
}

export function renderWrapper() { return card({}); }
export function renderAdapter() {
  return card({ kind: "Adapter", generated: false, name: "Claude CLI" });
}
export function renderToolset() {
  return card({ kind: "ToolSet", generated: false, tool_count: 3 });
}
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "install-result-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-install-result-render-"));
  const out = join(tmp, "install-result-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a generated metadata-only wrapper result renders LIVE next-action buttons", () => {
  // If InstallResultCard throws on mount, this call throws and the test fails.
  const html = mod.renderWrapper();
  // The honest summary headline (imported as metadata-only, no manifest needed).
  assert.match(html, /metadata-only/i);
  // The next-action buttons must be present — not just a pointer to the row. The
  // configure toggle reads "Hide setup" because a 0-tool wrapper auto-opens it; its
  // title pins the intent regardless of open/closed state.
  assert.match(html, /Hide setup|Configure &amp; review hints/);
  assert.match(html, /Review detected hints, register an MCP server/);
  assert.match(html, /Copy install path/);
  assert.match(html, /Install another/);
  assert.match(html, /Done/);
});

test("the wrapper result AUTO-OPENS the configure + detected-hints path", () => {
  // For a 0-tool wrapper the configure panel is open immediately, so the next step
  // (review hints / register MCP / add a tool) is right there in the result card.
  const html = mod.renderWrapper();
  assert.match(html, /Configure tools/);
  // The DetectedHints child mounts (its initial loading state under static render).
  assert.match(html, /Detected in source|Inspecting source/);
});

test("the result card never says a manifest is required", () => {
  const html = mod.renderWrapper();
  assert.doesNotMatch(html, /manifest (is )?required/i);
});

test("an adapter result offers the Crew configure path, not a tool/runtime path", () => {
  const html = mod.renderAdapter();
  assert.match(html, /Configure on Crew/);
  assert.match(html, /\/crew/);
  // An adapter is not a configurable ToolSet, so it shows no tool-config panel.
  assert.doesNotMatch(html, /Configure &amp; review hints/);
});

test("a real toolset with tools offers the Runtime next-action", () => {
  const html = mod.renderToolset();
  assert.match(html, /discovered 3 tools/);
  assert.match(html, /Runtime/);
});
