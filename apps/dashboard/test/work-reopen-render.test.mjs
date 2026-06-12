// Render/DOM verification for the Work board REOPEN control (design §6.9) — the
// compact lifecycle action that re-queues a BLOCKED task so its assigned operative can
// run it again. It drives the REAL `ReopenControl` export so the button actually
// renders for a blocked, assigned task; nothing renders for a non-blocked task (no
// dead affordance); and a blocked-but-unassigned task surfaces the honest reason in the
// detail panel (showReason) instead of a dead button.
//
// The browser-free harness renders to static markup, so it cannot fire the button's
// click → reluxWork.reopenTask → reload binding; the actual reopen mutation + its
// 409/400/eligibility semantics are pinned by the backend route tests (relux-kernel
// server.rs `reopen_task_route_*`) and the pure helper test (taskmove.test.ts
// `reopenEligibility`).
//
// Run: `npm test` (auto-discovered) or `node --test test/work-reopen-render.test.mjs`.

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

// A minimal ReluxTask — the control reads only id / status / assigned_agent.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { ReopenControl } from "./Work.tsx";

function task(status, assigned_agent) {
  return {
    id: "task_1",
    title: "held work",
    input: {},
    status,
    priority: 5,
    created_by: "operator",
    assigned_agent,
    namespace_id: "ns_root",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

// Board card variant (showReason off): silent when ineligible (no dead affordance).
export function renderCard(status, assigned_agent) {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <ReopenControl task={task(status, assigned_agent)} onReopened={() => {}} />
    </StaticRouter>
  );
}

// Task Detail variant (showReason on): a blocked-but-ineligible task surfaces the
// honest reason as a role=note line.
export function renderDetail(status, assigned_agent) {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <ReopenControl task={task(status, assigned_agent)} onReopened={() => {}} showReason />
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
      sourcefile: "work-reopen-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-reopen-render-"));
  const out = join(tmp, "work-reopen-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("ReopenControl shows the Reopen button for a blocked, assigned task", () => {
  const html = mod.renderCard("blocked", "prime");
  assert.match(html, /<button/);
  assert.match(html, /Reopen/);
  // The button explains it re-queues the work (a lifecycle action, not a status set).
  assert.match(html, /re-queues it/i);
});

test("ReopenControl shows the one-click Reopen & run button for a blocked, assigned task", () => {
  const html = mod.renderCard("blocked", "prime");
  // Both the plain Reopen and the chained Reopen & run are offered for eligible work.
  assert.match(html, /Reopen &amp; run|Reopen & run/);
  // Its title makes the no-bypass guarantee explicit (same run gate).
  assert.match(html, /same run gate/i);
});

test("ReopenControl renders NOTHING on a board card for a blocked task with no assignee", () => {
  for (const a of [undefined, ""]) {
    const html = mod.renderCard("blocked", a);
    assert.doesNotMatch(html, /<button/, `unassigned blocked (a=${String(a)}) shows no button on a card`);
    assert.doesNotMatch(html, /role="note"/, `card stays silent (no note)`);
  }
});

test("ReopenControl in the detail panel explains WHY an unassigned blocked task can't be reopened", () => {
  const html = mod.renderDetail("blocked", undefined);
  // No button, but a clear screen-reader-readable note with the honest reason.
  assert.doesNotMatch(html, /<button/);
  assert.match(html, /role="note"/);
  assert.match(html, /assign an operative/i);
});

test("ReopenControl renders NOTHING for a non-blocked task (card and detail)", () => {
  for (const s of ["created", "queued", "running", "waiting_for_approval", "completed", "failed", "cancelled"]) {
    const card = mod.renderCard(s, "prime");
    assert.doesNotMatch(card, /Reopen/, `non-blocked ${s} shows no reopen button on a card`);
    assert.doesNotMatch(card, /role="note"/, `non-blocked ${s} shows no note on a card`);
    const detail = mod.renderDetail(s, "prime");
    assert.doesNotMatch(detail, /Reopen/, `non-blocked ${s} shows no reopen button in detail`);
    assert.doesNotMatch(detail, /role="note"/, `non-blocked ${s} shows no reopen note in detail`);
  }
});
