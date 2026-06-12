// Render/DOM verification for the Work board status-MOVE control (design §6) — the
// compact Block / Cancel select the board cards and the Task Detail panel show. It
// drives the REAL `StatusMoveControl` export so the offered options actually render
// for a non-terminal task, and nothing renders for a terminal one (no dead "Move…"
// affordance on a finished task).
//
// The browser-free harness renders to static markup, so it cannot fire the select's
// change → reluxWork.setTaskStatus → reload binding; that real onClick→network→
// re-render path is the live-browser smoke's job (apps/dashboard/scripts/
// browser-smoke.mjs asserts the control renders on a seeded card). The actual move
// mutation + its 400/409/allowlist semantics are pinned by the backend route tests
// (relux-kernel server.rs `set_task_status_route_*`) and the pure helper test
// (taskmove.test.ts).
//
// Transpiles the real component with the esbuild Vite already vendors, then
// server-renders it through react-dom/server + react-router's StaticRouter. A
// render-time throw fails the test.
//
// Run: `npm test` (auto-discovered) or `node --test test/task-move-render.test.mjs`.

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
import { StatusMoveControl } from "./Work.tsx";

export function renderForStatus(status) {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <StatusMoveControl taskId={"task_1"} status={status} onMoved={() => {}} />
    </StaticRouter>
  );
}

// The Task Detail variant: a finished task shows the clear "can't be moved" note
// (showUnsupportedNote) instead of nothing, so a keyboard / screen-reader user
// learns WHY there is no control.
export function renderDetail(status) {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <StatusMoveControl taskId={"task_1"} status={status} onMoved={() => {}} showUnsupportedNote />
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
      sourcefile: "task-move-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-task-move-render-"));
  const out = join(tmp, "task-move-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("StatusMoveControl offers Block + Cancel for a non-terminal (queued) task", () => {
  const html = mod.renderForStatus("queued");
  assert.match(html, /Move…/);
  assert.match(html, /<select/);
  assert.match(html, /<option value="blocked">Block<\/option>/);
  assert.match(html, /<option value="cancelled">Cancel<\/option>/);
});

test("StatusMoveControl is keyboard-accessible: a descriptive aria-label + a described-by helper", () => {
  const html = mod.renderForStatus("queued");
  // The select carries a DESCRIPTIVE label (the allowed verbs + effects), not a bare
  // "Move…" — so a screen reader announces the move semantics.
  assert.match(html, /aria-label="Move task status — Block to hold this task, Cancel to stop it"/);
  // It is tied to a VISIBLE helper line via aria-describedby, and that helper explains
  // the Block/Cancel semantics AND the machine-driven lanes.
  assert.match(html, /aria-describedby="status-move-help-task_1"/);
  assert.match(html, /id="status-move-help-task_1"/);
  assert.match(html, /Block holds the task; Cancel stops it\./);
  assert.match(html, /set by the run lifecycle/);
});

test("StatusMoveControl drops Block (its own status) for a blocked task, keeps Cancel", () => {
  const html = mod.renderForStatus("blocked");
  assert.match(html, /<option value="cancelled">Cancel<\/option>/);
  assert.doesNotMatch(html, /<option value="blocked">Block<\/option>/);
  // The label describes only the offered move (Cancel), never the dropped Block.
  assert.match(html, /aria-label="Move task status — Cancel to stop it"/);
});

test("StatusMoveControl renders NOTHING for a terminal task on a board card (no dead affordance)", () => {
  for (const s of ["completed", "failed", "cancelled", "expired"]) {
    const html = mod.renderForStatus(s);
    assert.doesNotMatch(html, /Move…/, `terminal ${s} must not show a move control`);
    assert.doesNotMatch(html, /<select/, `terminal ${s} must not render a select`);
  }
});

test("StatusMoveControl in the detail panel explains WHY a finished task can't be moved", () => {
  for (const s of ["completed", "failed", "cancelled", "expired"]) {
    const html = mod.renderDetail(s);
    // No control, but a clear, screen-reader-readable note (role=note) with the reason.
    assert.doesNotMatch(html, /<select/, `terminal ${s} still has no select`);
    assert.match(html, /role="note"/, `terminal ${s} surfaces a note`);
    assert.match(html, /finished and can/i, `terminal ${s} explains why`);
  }
  // A movable task in the detail panel still renders the real select (note only for terminal).
  const movable = mod.renderDetail("queued");
  assert.match(movable, /<select/);
  assert.doesNotMatch(movable, /role="note"/);
});
