// Render/DOM verification for the Work board DRAG-TO-COLUMN affordances (design §6
// "Drag a card to a column → status mutation, with transition validation"). It drives
// the REAL `Column` export so the drop-target and draggable-card attributes actually
// render: a column exposes a labelled drop region, a non-terminal card is draggable
// with a drag handle, and a terminal card is NOT draggable (no dead drag affordance).
//
// The browser-free harness renders to static markup, so it cannot fire the native
// dragstart → drop → reluxWork.setTaskStatus → reload binding; that path is covered by
// the pure helper test (taskmove.test.ts columnDropTarget / parseTaskDrag) and the
// backend route tests (relux-kernel server.rs set_task_status_route_*). The live smoke
// (apps/dashboard/scripts/browser-smoke.mjs) asserts the same attributes on a seeded
// card. Drag is ADDITIVE — the StatusMoveControl select stays for keyboard use and is
// pinned by task-move-render.test.mjs.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-drag-render.test.mjs`.

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
import { Column } from "./Work.tsx";

function task(id, status, extra) {
  return {
    id,
    title: "Task " + id,
    status,
    namespace: "default",
    assigned_agent: null,
    parent_task: null,
    ...(extra || {}),
  };
}

// A column with a non-terminal (draggable) card and a terminal (non-draggable) card.
export function renderColumn(bucket, title) {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <Column
        title={title}
        bucket={bucket}
        tasks={[task("task_open", "queued"), task("task_done", "completed")]}
        onAction={() => {}}
        onInspectTask={() => {}}
        agents={[]}
        subtaskCounts={new Map()}
      />
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
      sourcefile: "work-drag-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-drag-render-"));
  const out = join(tmp, "work-drag-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a column renders a labelled drop region carrying its bucket", () => {
  const html = mod.renderColumn("blocked", "Blocked / Failed");
  assert.match(html, /class="board-column[^"]*"/, "the drop region has the board-column class");
  assert.match(html, /data-bucket="blocked"/, "the drop region carries its bucket");
  assert.match(html, /aria-label="Blocked \/ Failed column — drop a task here to move it"/, "labelled for AT");
});

test("a non-terminal card is draggable with a drag handle; a terminal card is not", () => {
  const html = mod.renderColumn("done", "Done");
  // The queued card is draggable and announces its role to assistive tech.
  assert.match(html, /draggable="true"/, "non-terminal card is draggable");
  assert.match(html, /aria-roledescription="draggable task card"/, "drag role announced");
  assert.match(html, /Drag to the Blocked or Done column to change status/, "drag hint present");
  // The completed card must NOT be draggable (no dead drag affordance on a finished task).
  assert.doesNotMatch(html, /draggable="true"[\s\S]*draggable="true"/, "only one card is draggable");
});

test("the select stays alongside drag (additive, keyboard-accessible)", () => {
  // The non-terminal card still carries the StatusMoveControl select — drag does not
  // replace the keyboard/accessibility path. The select now announces a DESCRIPTIVE
  // label + a described-by helper (the keyboard-accessible movement path, §6.8).
  const html = mod.renderColumn("blocked", "Blocked / Failed");
  assert.match(html, /aria-label="Move task status — [^"]+"/, "the keyboard move select remains, now described");
  assert.match(html, /aria-describedby="status-move-help-task_open"/, "the select is tied to its helper text");
});
