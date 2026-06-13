// Render/DOM verification for the CAPABILITY-ACTIVATION card in the Prime chat turn
// card (PrimeTurnCard → ConfigureCandidateCard). A first-paint Prime render cannot stage
// a proposal (useEffect never fires under renderToStaticMarkup), so this renders the
// real PrimeTurnCard directly with a seeded "awaiting_approval" turn whose action is
// `configure_plugin_candidate`, and asserts that:
//   - the activation confirmation card renders with what/where + the no-code-run guarantee,
//   - the "Configure with Prime" + Cancel actions are present, and
//   - a non-configure turn does NOT render the card (no false positive).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it
// through react-dom/server + react-router's StaticRouter, so a render-time throw fails
// here exactly as it would white-screen the chat. It does NOT fire onClick (Confirm
// posts to the single backend action route POST /v1/relux/prime/actions/configure-candidate)
// — that path is covered by the kernel routing tests and the prime-configure-candidate
// contract test.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-configure-candidate-render.test.mjs`.

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
  intent: "plugin_configuration",
  reply: "I can register the detected MCP server from plugin hermes-agent.",
  disposition: "awaiting_approval",
  action: null,
  created_task: null,
  started_run: null,
  created_agent: null,
  approval: "appr_1",
};
const CONFIGURE_MCP = {
  ...base,
  action: {
    type: "configure_plugin_candidate",
    plugin_id: "hermes-agent",
    candidate_id: "mcp",
  },
};
// A non-configure proposal turn — the card must NOT appear here.
const PERMISSION_PROPOSAL = {
  ...base,
  intent: "permission_change",
  reply: "I can grant access. I will not do this without approval.",
  action: { type: "grant_permission", subject_id: "agent_1", permission: "tool:x:y" },
};
export function renderConfigureMcp() {
  return at(<PrimeTurnCard turn={CONFIGURE_MCP} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderPermissionProposal() {
  return at(<PrimeTurnCard turn={PERMISSION_PROPOSAL} busy={false} onSuggestion={noop} onContinue={noop} />);
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
      sourcefile: "prime-configure-candidate-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-configure-candidate-render-"));
  const out = join(tmp, "prime-configure-candidate-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a configure-candidate proposal renders the activation card with what/where, guarantee, and actions", () => {
  const html = mod.renderConfigureMcp();
  // What will be activated and where.
  assert.match(html, /register the detected MCP server/i);
  assert.match(html, /plugin hermes-agent/);
  // The explicit no-code-run guarantee is on the card.
  assert.match(html, /No code from the source runs/i);
  assert.match(html, /gated \(needs approval\) until you ask me to use it/i);
  // The confirm + cancel actions are present (nothing has run yet).
  assert.match(html, /Configure with Prime/);
  assert.match(html, /Cancel/);
});

test("a non-configure proposal turn does NOT render the activation card", () => {
  const html = mod.renderPermissionProposal();
  assert.doesNotMatch(html, /No code from the source runs/i);
  assert.doesNotMatch(html, /Configure with Prime/);
});
