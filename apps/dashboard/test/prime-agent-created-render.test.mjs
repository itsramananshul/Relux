// Render/DOM verification for the AGENT-CREATED result card in the Prime chat turn card
// (PrimeTurnCard → AgentCreatedCard; RELUX_MASTER_PLAN §6, §7.3, §7.5, §8.1). When Prime
// hires an operative from chat, the turn must render a clear result card — not a bare
// "agent researcher" link. A first-paint Prime render cannot stage a turn (useEffect never
// fires under renderToStaticMarkup), so this renders the real PrimeTurnCard directly with a
// seeded agent-creation turn and asserts that:
//   - the result card renders the new operative's name/id and the adapter it runs on,
//   - a requested capability is shown as needing setup (NOT granted on creation) with a
//     clear "Grant … access" button that pre-fills the approval-gated follow-up,
//   - the "View in Crew" link is present, and
//   - casual ideation (no created agent) does NOT render the card (no false positive).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it through
// react-dom/server + react-router's StaticRouter, so a render-time throw fails here exactly
// as it would white-screen the chat. It does NOT fire onClick — the grant button just routes
// the existing approval-gated follow-up message, covered by the kernel tests.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-agent-created-render.test.mjs`.

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
  intent: "agent_creation",
  reply:
    "Creating agent \\"researcher\\" on the local adapter. I won't grant access to GitHub on " +
    "creation — that needs the matching tool plugin and a scoped permission you approve.",
  disposition: "executed",
  action: { type: "create_agent", name: "researcher", adapter_plugin: "relux-adapter-local-prime" },
  created_task: null,
  started_run: null,
  created_agent: "researcher",
  approval: null,
  ai_mode: "deterministic",
  state: {},
};
// A hire that asked for GitHub access → the card flags it as needing setup + a grant button.
const WITH_CAPABILITY = {
  ...base,
  suggested_actions: [
    {
      label: "Grant GitHub access to researcher",
      message: "grant tool:relux-tools-github:access to researcher",
      send: false,
    },
  ],
};
// A brain-shaped hire on the Claude adapter (agent_slots present).
const BRAIN_SHAPED = {
  ...base,
  agent_slots: {
    name: "Researcher",
    id: "researcher",
    description: "Reads GitHub and drafts PRs",
    adapter: "relux-adapter-claude-cli",
    persona: "Methodical and concise",
    source: "Claude CLI",
  },
};
// Casual ideation that created nothing — the card must NOT appear.
const CASUAL = {
  ...base,
  intent: "brainstorming",
  reply: "That could be a fun project to explore.",
  disposition: "answered",
  action: null,
  created_agent: null,
};
export function renderWithCapability() {
  return at(<PrimeTurnCard turn={WITH_CAPABILITY} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderBrainShaped() {
  return at(<PrimeTurnCard turn={BRAIN_SHAPED} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderCasual() {
  return at(<PrimeTurnCard turn={CASUAL} busy={false} onSuggestion={noop} onContinue={noop} />);
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
      sourcefile: "prime-agent-created-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-agent-created-render-"));
  const out = join(tmp, "prime-agent-created-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("an agent-creation turn renders the result card with operative, adapter, and capability setup", () => {
  const html = mod.renderWithCapability();
  // The card header + the new operative's name/id.
  assert.match(html, /operative created/i);
  assert.match(html, /researcher/);
  // The adapter it runs on (human brand + raw id).
  assert.match(html, /Local \(deterministic\)/);
  assert.match(html, /relux-adapter-local-prime/);
  // The honesty contract: a requested capability is NOT granted on creation. (The
  // apostrophe is HTML-entity-encoded in the markup, so match on the stable phrase.)
  assert.match(html, /granted on creation/i);
  assert.match(html, /Nothing was granted yet/i);
  // The grant follow-up renders as a clear button.
  assert.match(html, /Grant GitHub access to researcher/);
  // The Crew link is present so the operative is reachable.
  assert.match(html, /View in Crew/);
});

test("a brain-shaped hire shows the brain-validated adapter and role/persona", () => {
  const html = mod.renderBrainShaped();
  assert.match(html, /operative created/i);
  // The brain-validated Claude adapter wins over the action default.
  assert.match(html, /Claude/);
  assert.match(html, /relux-adapter-claude-cli/);
  assert.match(html, /Reads GitHub and drafts PRs/);
  assert.match(html, /Methodical and concise/);
  // No capability was requested → the least-privilege note is shown, not a grant button.
  assert.match(html, /least privilege/i);
});

test("casual ideation that created no agent does NOT render the result card", () => {
  const html = mod.renderCasual();
  assert.doesNotMatch(html, /operative created/i);
  assert.doesNotMatch(html, /View in Crew/);
});
