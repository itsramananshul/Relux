// Render/DOM verification for the tool-glue PREVIEW affordance on the Prime page
// (RELUX_MASTER_PLAN §23, the execute_code foundation). Two real components from Prime.tsx
// are server-rendered through react-dom/server + react-router's StaticRouter, so a render-time
// throw fails here exactly as it would white-screen the page:
//
//   - ToolGluePreviewPanel: the page must expose the preview affordance (a goal input, a
//     structured-steps editor, a "Preview plan" button) and TAKE NO ACTION on render — there is
//     no proposal yet, so no "Create tool-run task" commit button, and the inert "nothing runs"
//     contract is visible.
//   - ToolPlanCard: a preview result that mixes ready + needs-approval + unknown steps must
//     render ALL THREE readiness labels honestly (the unknown tool is never hidden) and, because
//     an unknown step forces ready_to_create:false, the commit button stays disabled.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-tool-glue-render.test.mjs`.

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
import { ToolGluePreviewPanel, ToolPlanCard } from "./Prime.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
// A grounded preview that mixes all three honest outcomes: a ready step, a gated step, and an
// unknown tool. The unknown forces ready_to_create:false, with the blocking issue surfaced.
const MIXED = {
  goal: "inspect the repo, then summarise it",
  summary: "3 steps: 1 ready, 1 needs approval, 1 unknown",
  steps: [
    { index: 1, plugin: "acme", tool: "build", args: {}, readiness: "ready", risk: "low" },
    { index: 2, plugin: "mcp:fs", tool: "write", args: { path: "x" }, readiness: "needs_approval", risk: "high" },
    { index: 3, plugin: "ghost", tool: "missing", args: {}, readiness: "unknown" },
  ],
  ready_to_create: false,
  issues: ["step 3: tool ghost/missing is not in the catalog"],
};
export function renderPanel() {
  return at(<ToolGluePreviewPanel busy={false} />);
}
export function renderMixedPlan() {
  return at(<ToolPlanCard proposal={MIXED} busy={false} />);
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
      sourcefile: "prime-tool-glue-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-tool-glue-render-"));
  const out = join(tmp, "prime-tool-glue-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the panel exposes the preview affordance and takes NO action on render", () => {
  const html = mod.renderPanel();
  // The affordance is present: a summary, a structured-steps editor, and the Preview button.
  assert.match(html, /Tool glue/i);
  assert.match(html, /multi-step ability plan/i);
  assert.match(html, /Preview plan/i);
  assert.match(html, /<textarea/);
  // It is honest about being inert.
  assert.match(html, /creates and runs nothing|nothing runs/i);
  // Nothing has been previewed yet → no proposal → no commit button anywhere in the panel.
  assert.doesNotMatch(html, /Create tool-run task/i);
});

test("a mixed preview renders ready + needs-approval + unknown honestly", () => {
  const html = mod.renderMixedPlan();
  // All three readiness outcomes are visible; the unknown tool is never hidden.
  assert.match(html, /\bready\b/i);
  assert.match(html, /needs approval/i);
  assert.match(html, /unknown tool/i);
  // The resolved tool labels render.
  assert.match(html, /acme\/build/);
  assert.match(html, /mcp:fs\/write/);
  assert.match(html, /ghost\/missing/);
  // The blocking issue is surfaced before commit.
  assert.match(html, /not in the catalog/i);
  // The commit button exists but is DISABLED — an unknown step forces ready_to_create:false.
  assert.match(html, /Create tool-run task/i);
  const btnIdx = html.indexOf("Create tool-run task");
  const slice = html.slice(Math.max(0, btnIdx - 200), btnIdx);
  assert.match(slice, /disabled/, "commit button must be disabled while the plan is not ready");
  // Previewing/rendering commits nothing.
  assert.match(html, /Nothing is created or run yet/i);
});
