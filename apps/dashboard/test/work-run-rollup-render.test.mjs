// Render/DOM verification for the per-subtree run/cost ROLLUP chips (design §6
// "live cost (tokens + spend) for the subtree"). It drives the REAL `RunRollupChips`
// export from Work.tsx with seeded runs + a subtree's task ids, asserting that the
// honest chip strip actually renders: a run count, the attention chips (failed),
// a real cost when reported, and — critically — an HONEST "cost unavailable" chip
// (never a fabricated $0.00) when no run in the subtree reported a cost.
//
// The static harness renders to markup, so it cannot exercise data fetching; the
// pure join + honesty semantics are pinned by runrollup.test.ts. This test proves
// the component wires the helper to the DOM and the tooltips/labels survive render.
//
// Transpiles the real component with the esbuild Vite already vendors, then
// server-renders it through react-dom/server + react-router's StaticRouter. A
// render-time throw fails the test.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-run-rollup-render.test.mjs`.

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
import { RunRollupChips } from "./Work.tsx";

export function render(runs, taskIds) {
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RunRollupChips runs={runs} taskIds={taskIds} />
    </StaticRouter>
  );
}
`;

const run = (p) => ({
  id: p.id,
  task_id: p.task_id,
  agent_id: "agent_1",
  adapter_plugin: "relux-adapter-claude-cli",
  status: p.status,
  cost: p.cost,
  duration_ms: p.duration_ms,
  usage: p.usage,
});

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "work-run-rollup-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-run-rollup-render-"));
  const out = join(tmp, "work-run-rollup-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("RunRollupChips renders run count + real cost + failed chip when cost is reported", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed", cost: 0.012, duration_ms: 8000, usage: { input_tokens: 1200, output_tokens: 340 } }),
    run({ id: "r2", task_id: "t2", status: "failed", cost: 0.003, duration_ms: 1000 }),
  ];
  const html = mod.render(runs, ["t1", "t2"]);
  assert.match(html, /2 runs/);
  assert.match(html, /1 failed/);
  assert.match(html, /\$0\.0150/); // 0.012 + 0.003 summed, real cost
  assert.match(html, /tok/); // a token chip
  assert.doesNotMatch(html, /cost unavailable/);
});

test("RunRollupChips renders an HONEST 'cost unavailable' chip (no fake $0.00) when no run reported a cost", () => {
  const runs = [
    run({ id: "r1", task_id: "t1", status: "completed" }),
    run({ id: "r2", task_id: "t1", status: "running" }),
  ];
  const html = mod.render(runs, ["t1"]);
  assert.match(html, /2 runs/);
  assert.match(html, /1 active/);
  assert.match(html, /cost unavailable/);
  assert.doesNotMatch(html, /\$/, "must not print a fabricated dollar figure");
  assert.doesNotMatch(html, /tok/, "no token chip when none reported");
});

test("RunRollupChips renders a single 'no runs yet' chip for a subtree with no runs", () => {
  const html = mod.render([], ["t1", "t2"]);
  assert.match(html, /no runs yet/);
  assert.doesNotMatch(html, /\$/);
});
