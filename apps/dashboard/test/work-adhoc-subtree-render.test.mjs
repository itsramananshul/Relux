// Render/DOM verification for ad-hoc task subtrees (design §6.2) — the Subtasks
// section the Task Detail panel shows for a selected parent. It drives the REAL
// `AdhocSubtaskSection` export with a seeded flat task list (a parent + three
// hand-made children in mixed states) so the progress strip, the numbered subtask
// list, the live-status badges, and the Add-subtask form are actually rendered and
// asserted — plus the honest empty state for a task with no sub-work.
//
// Transpiles the real component with the esbuild Vite already vendors, then
// server-renders it through react-dom/server + react-router's StaticRouter (the same
// declarative-router context the app uses). A render-time throw fails the test.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-adhoc-subtree-render.test.mjs`.

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
import { AdhocSubtaskSection } from "./Work.tsx";

const PARENT = "task_1";
const TASKS = [
  { id: "task_1", title: "Ship the feature", input: {}, status: "running", priority: 5, created_by: "op", namespace_id: "default", created_at: "1", updated_at: "2", assigned_agent: "a1" },
  { id: "task_2", title: "Write the docs", input: {}, status: "completed", priority: 5, created_by: "op", namespace_id: "default", parent_task: "task_1", created_at: "1", updated_at: "2", assigned_agent: "a1" },
  { id: "task_3", title: "Add tests", input: {}, status: "running", priority: 5, created_by: "op", namespace_id: "default", parent_task: "task_1", created_at: "1", updated_at: "2", assigned_agent: "a2" },
  { id: "task_4", title: "Fix the bug", input: {}, status: "blocked", priority: 5, created_by: "op", namespace_id: "default", parent_task: "task_1", created_at: "1", updated_at: "2", assigned_agent: "a2" },
];
const agentName = (id) => (id === "a1" ? "Builder" : id === "a2" ? "Tester" : "unassigned");

export function renderWithChildren() {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <AdhocSubtaskSection taskId={PARENT} tasks={TASKS} agentName={agentName} onInspectTask={() => {}} onChanged={() => {}} />
    </StaticRouter>
  );
}

export function renderEmpty() {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <AdhocSubtaskSection taskId={"task_5"} tasks={TASKS} agentName={agentName} onInspectTask={() => {}} onChanged={() => {}} />
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
      sourcefile: "work-adhoc-subtree-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-adhoc-render-"));
  const out = join(tmp, "work-adhoc-subtree-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("AdhocSubtaskSection renders the parent's children with a progress strip", () => {
  const html = mod.renderWithChildren();
  assert.match(html, /Subtasks/);
  // The three hand-made children (the real parent_task edge).
  assert.match(html, /Write the docs/);
  assert.match(html, /Add tests/);
  assert.match(html, /Fix the bug/);
  // The compact progress label: 1 done, 1 running, 1 blocked of 3.
  assert.match(html, /1\/3 done/);
  assert.match(html, /1 running/);
  assert.match(html, /1 blocked/);
  assert.match(html, /3 subtasks/);
  // The segmented progress strip (same as orchestration groups).
  assert.match(html, /seg-bar/);
  // Live board statuses and resolved assignee names surface on the rows.
  assert.match(html, /completed/);
  assert.match(html, /Builder/);
  assert.match(html, /Tester/);
});

test("AdhocSubtaskSection always offers an Add-subtask form", () => {
  const html = mod.renderWithChildren();
  assert.match(html, /Add a subtask/);
  assert.match(html, /Add subtask/);
});

test("AdhocSubtaskSection shows the honest empty state for a task with no sub-work", () => {
  const html = mod.renderEmpty();
  assert.match(html, /No sub-work yet/);
  // No fabricated subtree leaked in (no progress strip when empty).
  assert.doesNotMatch(html, /seg-bar/);
  // The Add-subtask form is still offered so the operator can start a subtree.
  assert.match(html, /Add subtask/);
});
