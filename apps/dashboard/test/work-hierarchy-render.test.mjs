// Render/DOM verification for Work hierarchy/progress v1 (the WorkHierarchy section
// on the board). The browser-free first-paint test (work-render.test.mjs) only sees
// the empty state, because no effect fires under renderToStaticMarkup. This test
// drives the SAME real component with seeded groups — a parent with children in
// mixed states (done / running / blocked / open) and real dependency edges — so the
// progress strip, the numbered workflow checklist, the role + live-status badges,
// and the blocked-by / blocking chips are actually rendered and asserted.
//
// It transpiles the REAL `WorkHierarchy` export (plus the pure buildWorkGroups join)
// with the esbuild Vite already vendors, then server-renders it through
// react-dom/server + react-router's StaticRouter — the same declarative-router
// context the app uses. A render-time throw fails the test.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-hierarchy-render.test.mjs`.

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

// A research → implementation → testing chain (the same shape the kernel's
// orchestration planner emits): implementation depends on research, testing on
// implementation. The live task list puts them in mixed states so every progress
// bucket and dependency chip is exercised.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { WorkHierarchy } from "./Work.tsx";
import { buildWorkGroups, nonEmptyGroups } from "../workhierarchy.ts";

const ORCH = {
  id: "orch_0001",
  goal: "Ship the widget",
  created_by: "operator",
  namespace_id: "default",
  status: "running",
  notes: [],
  created_at: "1",
  updated_at: "2",
  steps: [
    { task_id: "task_1", agent_id: "a1", role: "research", title: "Research the widget", outcome: "completed", depends_on: [] },
    { task_id: "task_2", agent_id: "a1", role: "implementation", title: "Build the widget", outcome: "pending", depends_on: [0] },
    { task_id: "task_3", agent_id: "a2", role: "testing", title: "Test the widget", outcome: "pending", depends_on: [1] },
  ],
};
const TASKS = [
  { id: "task_1", title: "Research the widget", input: {}, status: "completed", priority: 5, created_by: "op", namespace_id: "default", created_at: "1", updated_at: "2", assigned_agent: "a1" },
  { id: "task_2", title: "Build the widget", input: {}, status: "running", priority: 5, created_by: "op", namespace_id: "default", created_at: "1", updated_at: "2", assigned_agent: "a1" },
  { id: "task_3", title: "Test the widget", input: {}, status: "blocked", priority: 5, created_by: "op", namespace_id: "default", created_at: "1", updated_at: "2", assigned_agent: "a2" },
];
const AGENTS = [
  { id: "a1", name: "Builder", description: "", adapter_plugin: "x", namespace: "default", status: "active", permissions_summary: "", permissions: [], created_at: "1" },
  { id: "a2", name: "Tester", description: "", adapter_plugin: "x", namespace: "default", status: "active", permissions_summary: "", permissions: [], created_at: "1" },
];

export function renderWithGroups() {
  const groups = nonEmptyGroups(buildWorkGroups([ORCH], TASKS));
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <WorkHierarchy groups={groups} error={null} loading={false} agents={AGENTS} onInspectTask={() => {}} />
    </StaticRouter>
  );
}

export function renderEmpty() {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <WorkHierarchy groups={[]} error={null} loading={false} agents={[]} onInspectTask={() => {}} />
    </StaticRouter>
  );
}

export function renderError() {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <WorkHierarchy groups={[]} error={"boom"} loading={false} agents={[]} onInspectTask={() => {}} />
    </StaticRouter>
  );
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
      sourcefile: "work-hierarchy-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-hierarchy-render-"));
  const out = join(tmp, "work-hierarchy-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("WorkHierarchy renders a parent group with its goal, progress and brief count", () => {
  const html = mod.renderWithGroups();
  assert.match(html, /Work groups/);
  assert.match(html, /Ship the widget/); // the goal (the parent)
  assert.match(html, /orch_0001/);
  // The compact progress label from the LIVE statuses: 1 done, 1 running, 1 blocked.
  assert.match(html, /1\/3 done/);
  assert.match(html, /1 running/);
  assert.match(html, /1 blocked/);
  assert.match(html, /3 briefs/);
  // The segmented progress strip is present.
  assert.match(html, /seg-bar/);
});

test("WorkHierarchy renders the numbered workflow checklist with children + dependency chips", () => {
  const html = mod.renderWithGroups();
  // The three child briefs (the nested sub-work) and their specialist roles.
  assert.match(html, /Research the widget/);
  assert.match(html, /Build the widget/);
  assert.match(html, /Test the widget/);
  assert.match(html, /research/);
  assert.match(html, /implementation/);
  assert.match(html, /testing/);
  // LIVE board statuses surface on the rows (not the durable pending outcome).
  assert.match(html, /running/);
  assert.match(html, /blocked/);
  // Real dependency edges resolved to sibling task ids (blocked-by / blocking).
  assert.match(html, /blocked by task_1/); // implementation waits on research
  assert.match(html, /blocks task_3/); // implementation blocks testing
  assert.match(html, /blocked by task_2/); // testing waits on implementation
  // Assignees resolve to crew names.
  assert.match(html, /Builder/);
  assert.match(html, /Tester/);
});

test("WorkHierarchy shows the honest empty state when there are no groups", () => {
  const html = mod.renderEmpty();
  assert.match(html, /No sub-work yet/);
  // No fabricated group leaked in.
  assert.doesNotMatch(html, /seg-bar/);
});

test("WorkHierarchy degrades to an inline note (not a blank) when the read failed", () => {
  const html = mod.renderError();
  assert.match(html, /Work groups unavailable/);
  assert.match(html, /board below still works/);
});
