// Render/DOM verification for the GITHUB PLUGIN-IMPORT card in the Prime chat turn
// card (PrimeTurnCard → PluginInstallCard). A first-paint Prime render cannot stage a
// proposal (useEffect never fires under renderToStaticMarkup), so this renders the
// real PrimeTurnCard directly with a seeded "awaiting_approval" turn whose action is
// `install_plugin_from_github`, and asserts that:
//   - the import confirmation card renders with the canonical source + proposed id,
//   - the explicit no-code-run guarantee and next step are shown,
//   - the Confirm import / Cancel actions are present, and
//   - a non-GitHub-import turn does NOT render the card (no false positive).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it
// through react-dom/server + react-router's StaticRouter, so a render-time throw fails
// here exactly as it would white-screen the chat. It does NOT fire onClick (those call
// the live install-github / hints / approvals routes) — that path is covered by the
// kernel routing tests and the existing install-result render test.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-plugin-install-render.test.mjs`.

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
  intent: "plugin_installation",
  reply:
    "I can import https://github.com/nousresearch/hermes-agent from GitHub as a plugin. " +
    "No code from the repository runs on import.",
  disposition: "awaiting_approval",
  action: null,
  created_task: null,
  started_run: null,
  created_agent: null,
  approval: "appr_1",
};
const GITHUB_IMPORT = {
  ...base,
  action: {
    type: "install_plugin_from_github",
    repo_url: "https://github.com/nousresearch/hermes-agent",
    plugin_id: "relux-plugin-hermes-agent",
  },
};
// A non-import proposal turn — the card must NOT appear here.
const PERMISSION_PROPOSAL = {
  ...base,
  intent: "permission_change",
  reply: "I can grant access. I will not do this without approval.",
  action: { type: "grant_permission", subject_id: "agent_1", permission: "tool:x:y" },
};
export function renderGithubImport() {
  return at(<PrimeTurnCard turn={GITHUB_IMPORT} busy={false} onSuggestion={noop} onContinue={noop} />);
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
      sourcefile: "prime-plugin-install-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-plugin-install-render-"));
  const out = join(tmp, "prime-plugin-install-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a GitHub-import proposal renders the import card with source, id, guarantee, and actions", () => {
  const html = mod.renderGithubImport();
  // The canonical source and the proposed local id are shown.
  assert.match(html, /https:\/\/github\.com\/nousresearch\/hermes-agent/);
  assert.match(html, /relux-plugin-hermes-agent/);
  // The explicit no-code-run guarantee is on the card.
  assert.match(html, /No code from the repository runs on import/i);
  // The confirm + cancel actions are present (nothing has run yet).
  assert.match(html, /Confirm import/);
  assert.match(html, /Cancel/);
  // Honest "nothing cloned yet" footer.
  assert.match(html, /Nothing has been cloned yet/i);
});

test("a non-import proposal turn does NOT render the plugin-import card", () => {
  const html = mod.renderPermissionProposal();
  assert.doesNotMatch(html, /Confirm import/);
  assert.doesNotMatch(html, /No code from the repository runs on import/i);
});
