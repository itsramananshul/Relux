// Render/DOM verification for a single Crew member card (Crew.tsx → CrewMemberCard).
// The Crew page loads agents through useAsync, which never fetches under
// renderToStaticMarkup, so the full-page render test only ever sees the loading state.
// This renders the EXPORTED card directly with seeded agent records so a populated
// created-agent state is actually exercised, and pins that the card:
//   - shows the adapter as a human brand + raw id (the runtime Prime named on hire),
//   - reads permissions as least-privilege when empty (the honest setup hint),
//   - renders cleanly when optional fields are MISSING (no blank, no throw) — the
//     guarantee that one missing agent field never blanks the Crew page.
//
// It transpiles the REAL component from Crew.tsx with esbuild + server-renders it through
// react-dom/server + react-router's StaticRouter, so a render-time throw fails here.
//
// Run: `npm test` (auto-discovered) or `node --test test/crew-member-render.test.mjs`.

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
import { CrewMemberCard } from "./Crew.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/crew">{el}</StaticRouter>);
}
const noop = () => {};
// A fully-populated operative freshly created by Prime on the Claude adapter.
const FULL = {
  id: "researcher",
  name: "Researcher",
  description: "Reads GitHub and drafts PRs",
  persona: "Methodical and concise",
  status: "active",
  adapter_plugin: "relux-adapter-claude-cli",
  skills: ["research", "rust"],
  permissions: [],
  reports_to: "lead-1",
  reports_to_name: "Lead One",
  reports: [],
  created_at: "2026-06-13T08:05:27Z",
};
// A sparse operative: only id/name/created_at — every optional field is missing.
const SPARSE = {
  id: "minimal",
  name: "Minimal",
  adapter_plugin: "",
  created_at: "not-a-real-date",
};
export function renderFull() {
  return at(<CrewMemberCard agent={FULL} queued={2} running={1} onEdit={noop} />);
}
export function renderSparse() {
  return at(<CrewMemberCard agent={SPARSE} queued={0} running={0} onEdit={noop} />);
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
      sourcefile: "crew-member-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-crew-member-render-"));
  const out = join(tmp, "crew-member-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a populated created agent renders adapter brand + id, skills, and least-privilege permissions", () => {
  const html = mod.renderFull();
  assert.match(html, /Researcher/);
  assert.match(html, /Reads GitHub and drafts PRs/);
  assert.match(html, /Methodical and concise/);
  // Adapter shows the human brand AND the raw plugin id.
  assert.match(html, /Claude/);
  assert.match(html, /relux-adapter-claude-cli/);
  // A freshly created agent has no permissions yet → the honest least-privilege hint.
  assert.match(html, /none \(least privilege\)/);
  // Skills render as chips.
  assert.match(html, /research/);
});

test("a sparse agent (missing optional fields) renders cleanly without blanking or throwing", () => {
  const html = mod.renderSparse();
  assert.match(html, /Minimal/);
  // Missing role → honest placeholder, never blank.
  assert.match(html, /Role:<\/strong> N\/A/);
  // Missing adapter → em dash, not a brand of empty string.
  assert.match(html, /Adapter:<\/strong> —/);
  // Top-level when no Lead is set.
  assert.match(html, /none \(top-level\)/);
  // A non-parseable created_at falls back to the raw string, never a crash.
  assert.match(html, /not-a-real-date/);
});
