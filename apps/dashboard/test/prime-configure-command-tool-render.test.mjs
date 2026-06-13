// Render/DOM verification for the FROM-SCRATCH command-tool card in the Prime chat turn
// card (PrimeTurnCard → ConfigureCommandToolCard) — the bridge a source-only plugin uses
// when the user names a command ("configure this repo as a tool that runs npm test"). A
// first-paint Prime render cannot stage a proposal (useEffect never fires under
// renderToStaticMarkup), so this renders the real PrimeTurnCard directly with a seeded
// "awaiting_approval" turn whose action is `configure_command_tool`, and asserts that:
//   - the confirmation card renders with the argv-only/gated guarantees,
//   - the reviewable fields are pre-filled from the action (program + args + tool name), and
//   - a non-configure turn does NOT render the card (no false positive).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it through
// react-dom/server + react-router's StaticRouter, so a render-time throw fails here exactly
// as it would white-screen the chat. It does NOT fire onClick (Confirm posts to the single
// backend route POST /v1/relux/prime/actions/configure-command-tool) — that path is covered
// by the kernel routing tests and the prime-configure-command-tool contract test.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-configure-command-tool-render.test.mjs`.

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
import { PrimeTurnCard, ConfigureCommandToolResult } from "./Prime.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
const noop = () => {};
const base = {
  intent: "plugin_configuration",
  reply: "I can configure a governed command tool on plugin acme-tools that runs npm test.",
  disposition: "awaiting_approval",
  action: null,
  created_task: null,
  started_run: null,
  created_agent: null,
  approval: "appr_1",
};
const CONFIGURE_CMD = {
  ...base,
  action: {
    type: "configure_command_tool",
    plugin_id: "acme-tools",
    tool_name: "npm.test",
    program: "npm",
    args: ["test"],
    cwd: "",
  },
};
// A non-configure proposal turn — the card must NOT appear here.
const PERMISSION_PROPOSAL = {
  ...base,
  intent: "permission_change",
  reply: "I can grant access. I will not do this without approval.",
  action: { type: "grant_permission", subject_id: "agent_1", permission: "tool:x:y" },
};
export function renderConfigureCmd() {
  return at(<PrimeTurnCard turn={CONFIGURE_CMD} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderPermissionProposal() {
  return at(<PrimeTurnCard turn={PERMISSION_PROPOSAL} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderResult() {
  return at(<ConfigureCommandToolResult result={{
    plugin_id: "acme-tools",
    plugin_name: "acme-tools",
    tool_name: "npm.test",
    permission: "tool:acme-tools:test",
    gated: true,
    next_step: "Configured command tool \\"npm.test\\". Ask me to use npm.test.",
    no_code_executed: true,
    catalog_refresh: true,
    approval_closed: true,
  }} />);
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
      sourcefile: "prime-configure-command-tool-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-configure-command-tool-render-"));
  const out = join(tmp, "prime-configure-command-tool-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a configure-command-tool proposal renders the card with guarantees + reviewable fields", () => {
  const html = mod.renderConfigureCmd();
  // Where + the argv-only/gated safety copy.
  assert.match(html, /configure a command tool on plugin acme-tools/i);
  assert.match(html, /Argv-only, never a shell/i);
  assert.match(html, /gated \(needs approval\) until you ask me to use it/i);
  // The reviewable fields are pre-filled from the action.
  assert.match(html, /value="npm\.test"/);
  assert.match(html, /value="npm"/);
  assert.match(html, /test<\/textarea>/);
  // Confirm + Cancel actions are present (nothing has run yet).
  assert.match(html, /Configure with Prime/);
  assert.match(html, /Cancel/);
});

test("a non-configure proposal turn does NOT render the command-tool card", () => {
  const html = mod.renderPermissionProposal();
  assert.doesNotMatch(html, /Argv-only, never a shell/i);
  assert.doesNotMatch(html, /configure a command tool on/i);
});

test("the result view shows the configured tool, its permission, and gated/no-code badges", () => {
  const html = mod.renderResult();
  assert.match(html, /npm\.test/);
  assert.match(html, /tool:acme-tools:test/);
  assert.match(html, /gated/);
  assert.match(html, /no code run/);
});
