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
  assert.match(html, /Move task status/); // the accessible label
  assert.match(html, /<option value="blocked">Block<\/option>/);
  assert.match(html, /<option value="cancelled">Cancel<\/option>/);
});

test("StatusMoveControl drops Block (its own status) for a blocked task, keeps Cancel", () => {
  const html = mod.renderForStatus("blocked");
  assert.match(html, /<option value="cancelled">Cancel<\/option>/);
  assert.doesNotMatch(html, /<option value="blocked">Block<\/option>/);
});

test("StatusMoveControl renders NOTHING for a terminal task (no move possible)", () => {
  for (const s of ["completed", "failed", "cancelled", "expired"]) {
    const html = mod.renderForStatus(s);
    assert.doesNotMatch(html, /Move…/, `terminal ${s} must not show a move control`);
    assert.doesNotMatch(html, /<select/, `terminal ${s} must not render a select`);
  }
});
