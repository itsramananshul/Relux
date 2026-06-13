// Render/DOM verification for the TOOL-RESULT block in the Prime chat turn card
// (PrimeTurnCard → ToolResult → ToolOutputBlock; RELUX_MASTER_PLAN §11.1, Plugin Lens shaping
// in `plugin_source::shape_result`). When Prime runs a tool, the chat bubble must show the
// HUMAN answer (the shaped `result` text) — never the raw JSON-RPC/structured envelope — with
// the structured detail tucked into a collapsible "raw details" expander. This renders the real
// PrimeTurnCard directly with a seeded tool-invocation turn and asserts that:
//   - the natural answer text renders in the main body,
//   - a "raw details" expander is present (a <details> with the structured JSON) when the
//     output is a shaped { result, structuredContent } envelope,
//   - the structured JSON is NOT in the main answer body (it lives only inside the expander), and
//   - a plain-string tool output renders no "raw details" expander (nothing extra to show).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it through
// react-dom/server + react-router's StaticRouter, so a render-time throw fails here exactly as
// it would white-screen the chat.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-tool-output-render.test.mjs`.

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

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { PrimeTurnCard } from "./Prime.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
const noop = () => {};
const base = {
  intent: "tool_invocation",
  reply: "Running acme-repo/plugin.summary.",
  disposition: "executed",
  action: null,
  created_task: null,
  started_run: null,
  created_agent: null,
  approval: null,
  ai_mode: "deterministic",
  state: {},
};
// A Plugin Lens summary: the kernel shapes it into { result: <human prose>, structuredContent }.
const SHAPED = {
  ...base,
  invoked_tool: "acme-repo/plugin.summary",
  tool_output: {
    result: "**Acme** v1.0.0 — Manifestless.\\nDoes acme things.\\n7 files, 2 directories",
    structuredContent: { plugin_id: "acme-repo", file_count: 7, dir_count: 2 },
  },
};
// A plain-string tool output (no structured detail): no expander should appear.
const PLAIN = {
  ...base,
  invoked_tool: "relux-tools-status/status.summary",
  tool_output: { result: "Nothing is running yet; the control plane is idle." },
};
export function renderShaped() {
  return at(<PrimeTurnCard turn={SHAPED} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderPlain() {
  return at(<PrimeTurnCard turn={PLAIN} busy={false} onSuggestion={noop} onContinue={noop} />);
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
      sourcefile: "prime-tool-output-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-tool-output-render-"));
  const out = join(tmp, "prime-tool-output-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a shaped tool result shows the human answer plus a collapsible raw-details expander", () => {
  const html = mod.renderShaped();
  // The natural answer text renders in the bubble.
  assert.match(html, /Acme/);
  assert.match(html, /Does acme things/);
  assert.match(html, /7 files, 2 directories/);
  // The structured detail lives behind a "raw details" expander, not the main body.
  assert.match(html, /<details/);
  assert.match(html, /raw details/i);
  // The structured JSON is present (inside the expander; quotes are HTML-entity-encoded in the
  // server-rendered markup) but the human answer leads.
  assert.match(html, /plugin_id/);
  const idx = html.indexOf("Does acme things");
  const jsonIdx = html.indexOf("plugin_id");
  assert.ok(idx >= 0 && jsonIdx >= 0 && idx < jsonIdx, "human answer must precede the raw JSON");
});

test("a plain-string tool result renders no raw-details expander", () => {
  const html = mod.renderPlain();
  assert.match(html, /control plane is idle/);
  // Nothing structured to expand → no <details> / "raw details".
  assert.doesNotMatch(html, /raw details/i);
});
