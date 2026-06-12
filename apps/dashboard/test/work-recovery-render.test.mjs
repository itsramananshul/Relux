// Render/DOM verification for the RECOVERY DECISION CARD (execution-and-issue §3.3b;
// dashboard §6.9 remaining gap). Drives the REAL `RecoveryCard` export with assessments
// built by the REAL recovery model, so the card actually renders the root cause +
// recommendation + the right affordance per action: a wired button, a navigation link,
// a reassign picker, or — for an action this surface can't wire — a muted POINTER (never
// a dead button). The click → route bindings are pinned by the route tests + the pure
// recovery.test.ts; this pins what the operator SEES.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-recovery-render.test.mjs`.

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
import { RecoveryCard } from "./Work.tsx";
import { assessRunRecovery, assessTaskRecovery } from "../recovery.ts";

function run(over) {
  return {
    id: "run_0001", task_id: "task_0001", agent_id: "prime",
    adapter_plugin: "claude-cli", status: "failed", ...over,
  };
}
function task(over) {
  return {
    id: "task_0001", title: "held work", input: {}, status: "blocked",
    priority: 5, created_by: "operator", assigned_agent: "prime",
    namespace_id: "ns_root", created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z", ...over,
  };
}

// A failed run: Retry wired as a button, Configure as a link, Inspect unwired (pointer).
export function renderRunCard() {
  const assessment = assessRunRecovery(run({ failure_class: "adapter_missing", retryable: true }));
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard
        assessment={assessment}
        handlers={{
          retry_run: { onClick: () => {} },
          configure_agent: { to: "/crew" },
        }}
      />
    </StaticRouter>
  );
}

// An unclassified failure: missingInfo note must render; inspect leads (pointer when unwired).
export function renderUnknownRunCard() {
  const assessment = assessRunRecovery(run({ status: "failed", failure_class: undefined, retryable: true }));
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard assessment={assessment} handlers={{ retry_run: { onClick: () => {} } }} />
    </StaticRouter>
  );
}

// A blocked + assigned task: reopen buttons wired + a reassign agent picker.
export function renderTaskCard() {
  const assessment = assessTaskRecovery(task({ assigned_agent: "prime" }), null);
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard
        assessment={assessment}
        handlers={{
          reopen_and_run: { onClick: () => {} },
          reopen: { onClick: () => {} },
          reassign: { reassign: { agents: [{ id: "prime", name: "Prime" }, { id: "scout", name: "Scout" }], current: "prime", onReassign: () => {} } },
        }}
      />
    </StaticRouter>
  );
}

// A blocked + UNASSIGNED task: the only action is the assign picker + the missing-info note.
export function renderUnassignedTaskCard() {
  const assessment = assessTaskRecovery(task({ assigned_agent: undefined }), null);
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard
        assessment={assessment}
        handlers={{ reassign: { reassign: { agents: [{ id: "scout", name: "Scout" }], current: null, onReassign: () => {} } } }}
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
      sourcefile: "work-recovery-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-recovery-render-"));
  const out = join(tmp, "work-recovery-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("run card shows the Recovery eyebrow, root cause, and recommendation", () => {
  const html = mod.renderRunCard();
  assert.match(html, /Recovery/);
  assert.match(html, /Root cause/);
  assert.match(html, /Recommended/);
  assert.match(html, /Adapter not available/);
});

test("run card wires Retry as a button and Configure as a link", () => {
  const html = mod.renderRunCard();
  assert.match(html, /<button[^>]*>Retry<\/button>/);
  // configure_agent → an anchor (Link) to /crew, labelled "Configure adapter".
  assert.match(html, /<a[^>]*href="\/crew"[^>]*>Configure adapter<\/a>/);
});

test("run card renders an unwired action (Inspect logs) as a muted pointer, not a button", () => {
  const html = mod.renderRunCard();
  // The inspect action is present as a "→ Inspect logs" pointer (not a <button>/<a>).
  assert.match(html, /→ Inspect logs/);
  assert.doesNotMatch(html, /<button[^>]*>Inspect logs<\/button>/);
});

test("an unclassified run card surfaces the missing-info note", () => {
  const html = mod.renderUnknownRunCard();
  assert.match(html, /Unknown failure/);
  assert.match(html, /role="note"/);
  assert.match(html, /no structured failure class/i);
});

test("a blocked task card wires Reopen & run + Reopen buttons and a reassign picker", () => {
  const html = mod.renderTaskCard();
  assert.match(html, /Reopen &amp; run|Reopen & run/);
  assert.match(html, /<button[^>]*>Reopen<\/button>/);
  // Reassign renders as a <select> listing agents; the current one is marked.
  assert.match(html, /<select/);
  assert.match(html, /Prime \(current\)/);
  assert.match(html, /Scout/);
});

test("a blocked + unassigned task card shows the assign picker and the assign-first note", () => {
  const html = mod.renderUnassignedTaskCard();
  assert.match(html, /<select/);
  assert.match(html, /Assign operative/);
  assert.match(html, /role="note"/);
  assert.match(html, /assign an operative/i);
});
