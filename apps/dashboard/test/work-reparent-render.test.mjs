// Render/DOM verification for the safe-reparent control (design §6.6) — the compact
// "Move under…" selector + "Remove parent" button the Task Detail panel shows. It
// drives the REAL `ReparentControl` export with a seeded flat task list so the
// candidate options (excluding self + descendants + cross-namespace), the Remove-parent
// affordance (only when the task has a parent), and the honest "no valid parent" empty
// state are actually rendered and asserted.
//
// Transpiles the real component with the esbuild Vite already vendors, then
// server-renders it through react-dom/server + react-router's StaticRouter (the same
// declarative-router context the app uses). A render-time throw fails the test.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-reparent-render.test.mjs`.

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
import { ReparentControl } from "./Work.tsx";

// task_1 -> task_2 (a real subtree), a same-ns standalone task_3, and task_4 in a
// different namespace (must never be offered as a parent).
const TASKS = [
  { id: "task_1", title: "Ship the feature", input: {}, status: "running", priority: 5, created_by: "op", namespace_id: "default", created_at: "1", updated_at: "2", assigned_agent: "a1" },
  { id: "task_2", title: "Write the docs", input: {}, status: "created", priority: 5, created_by: "op", namespace_id: "default", parent_task: "task_1", created_at: "1", updated_at: "2", assigned_agent: "a1" },
  { id: "task_3", title: "Add tests", input: {}, status: "created", priority: 5, created_by: "op", namespace_id: "default", created_at: "1", updated_at: "2", assigned_agent: "a2" },
  { id: "task_4", title: "Other tenant", input: {}, status: "created", priority: 5, created_by: "op", namespace_id: "other", created_at: "1", updated_at: "2", assigned_agent: "a3" },
];
const byId = (id) => TASKS.find((t) => t.id === id);

export function renderForChild() {
  // task_2 is a subtask of task_1 — it has a parent (Remove shows) and a candidate
  // (task_3); task_1 is its current parent (no-op, excluded), task_4 is cross-ns.
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <ReparentControl task={byId("task_2")} tasks={TASKS} onReparented={() => {}} />
    </StaticRouter>
  );
}

export function renderForRoot() {
  // task_1 is the subtree ROOT: its descendants (task_2) must NOT be offered; task_3 is
  // the only safe candidate; task_1 has no parent so Remove must be absent.
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <ReparentControl task={byId("task_1")} tasks={TASKS} onReparented={() => {}} />
    </StaticRouter>
  );
}

export function renderNoCandidates() {
  // A lone same-ns task: nothing can be its parent → honest empty state, no <select>.
  const LONE = [TASKS[0], TASKS[3]]; // task_1 (default) + task_4 (other ns)
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <ReparentControl task={LONE[0]} tasks={LONE} onReparented={() => {}} />
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
      sourcefile: "work-reparent-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-reparent-render-"));
  const out = join(tmp, "work-reparent-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("ReparentControl offers a Move-under selector with only safe candidates + Remove for a parented task", () => {
  const html = mod.renderForChild();
  assert.match(html, /Move under…/);
  // task_3 is the only safe candidate (same ns, not self/descendant/current-parent).
  assert.match(html, /Add tests/);
  // The current parent (task_1) is a no-op and must NOT be offered.
  assert.doesNotMatch(html, /Ship the feature/);
  // The cross-namespace task_4 must NEVER be offered.
  assert.doesNotMatch(html, /Other tenant/);
  // task_2 has a parent → Remove parent affordance is present.
  assert.match(html, /Remove parent/);
});

test("ReparentControl never offers a descendant of the moved task, and hides Remove for a top-level task", () => {
  const html = mod.renderForRoot();
  // task_3 is safe; task_2 (a descendant of task_1) must NOT appear as a candidate.
  assert.match(html, /Add tests/);
  assert.doesNotMatch(html, /Write the docs/);
  // task_1 is top-level → no Remove-parent button.
  assert.doesNotMatch(html, /Remove parent/);
});

test("ReparentControl shows the honest empty state when no task can be its parent", () => {
  const html = mod.renderNoCandidates();
  assert.match(html, /No other task can be its parent/);
  // No selector is rendered when there is nothing safe to choose.
  assert.doesNotMatch(html, /Move under…/);
});
