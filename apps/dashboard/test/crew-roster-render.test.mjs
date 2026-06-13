// Render/DOM verification for the Crew roster section (Crew.tsx → CrewRoster), the
// presentational component extracted from the stateful `Crew` container so each of the
// page's states can actually be exercised. The full-page render only ever sees the
// loading state (useAsync never fetches under renderToStaticMarkup); this renders the
// EXPORTED CrewRoster directly with seeded props so the four required states are pinned:
//   - loading        — an honest "Loading your crew…" hint, never blank,
//   - error          — a clear failure message + a Retry control,
//   - prime-only     — an ACTIONABLE empty state (Prime is the lone seeded operative),
//   - populated      — real operative cards, and NO empty-state nudge.
//
// It transpiles the REAL component from Crew.tsx with esbuild + server-renders it through
// react-dom/server + react-router's StaticRouter, so a render-time throw fails here.
//
// Run: `npm test` (auto-discovered) or `node --test test/crew-roster-render.test.mjs`.

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
import { CrewRoster } from "./Crew.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/crew">{el}</StaticRouter>);
}
const noop = () => {};
const PRIME = {
  id: "prime",
  name: "Prime",
  description: "Control-plane operative",
  status: "active",
  adapter_plugin: "relux-adapter-local-prime",
  permissions: [],
  created_at: "2026-06-13T08:00:00Z",
};
const RESEARCHER = {
  id: "researcher",
  name: "Researcher",
  description: "Reads docs and drafts PRs",
  status: "active",
  adapter_plugin: "relux-adapter-claude-cli",
  permissions: [],
  created_at: "2026-06-13T08:05:00Z",
};
const base = {
  adapters: [],
  agentTaskCounts: {},
  editingId: null,
  onEdit: noop,
  onCancelEdit: noop,
  onRetry: noop,
  afterChange: noop,
};
export function renderLoading() {
  return at(<CrewRoster {...base} agents={[]} loading={true} />);
}
export function renderError() {
  return at(<CrewRoster {...base} agents={[]} loading={false} agentsError={"boom"} />);
}
export function renderPrimeOnly() {
  return at(<CrewRoster {...base} agents={[PRIME]} loading={false} />);
}
export function renderPopulated() {
  return at(<CrewRoster {...base} agents={[PRIME, RESEARCHER]} loading={false} />);
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
      sourcefile: "crew-roster-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-crew-roster-render-"));
  const out = join(tmp, "crew-roster-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("loading state shows an honest hint, never a blank node", () => {
  const html = mod.renderLoading();
  assert.match(html, /Your Crew/);
  assert.match(html, /Loading your crew/);
  // No error or empty-state copy while loading.
  assert.doesNotMatch(html, /Could not load your crew/);
  assert.doesNotMatch(html, /only operative/);
});

test("error state surfaces the failure and a Retry control", () => {
  const html = mod.renderError();
  assert.match(html, /Could not load your crew/);
  // The raw error is shown so the operator can act on it.
  assert.match(html, /boom/);
  assert.match(html, /Retry/);
});

test("prime-only roster renders Prime AND an actionable empty state (no marketing hero)", () => {
  const html = mod.renderPrimeOnly();
  // Prime itself still renders as a real card.
  assert.match(html, /Prime/);
  // The actionable nudge: create one, or ask Prime in chat.
  assert.match(html, /only operative/);
  assert.match(html, /Prime in chat/);
  // It links to the Prime chat surface (the "ask Prime to create one" path).
  assert.match(html, /href="\/prime"/);
});

test("a populated roster renders every operative and NO empty-state nudge", () => {
  const html = mod.renderPopulated();
  assert.match(html, /Researcher/);
  assert.match(html, /Reads docs and drafts PRs/);
  assert.match(html, /Prime/);
  // With a real crew member present the 'only operative' nudge must be gone.
  assert.doesNotMatch(html, /only operative/);
});
