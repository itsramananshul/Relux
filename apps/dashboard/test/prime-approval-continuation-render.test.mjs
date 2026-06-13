// Render/DOM verification for the APPROVAL-CONTINUATION wiring in the Prime chat
// turn card (PrimeTurnCard → ApprovalCard). A first-paint Prime render cannot
// stage a pending approval (useEffect never fires under renderToStaticMarkup, so
// no turn is ever produced), so this renders the real PrimeTurnCard directly with
// a seeded paused-on-approval turn and asserts that:
//   - the per-call approval card renders its full action set, and
//   - the continuation strip tells the operator Prime will continue AUTOMATICALLY
//     with the tool result (the agentic approve → run → continue flow), rather than
//     dead-ending the chat.
// It also covers the limit-paused case (a plain "Keep working" button, no approval).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it
// through react-dom/server + react-router's StaticRouter, so a render-time throw
// fails here exactly as it would white-screen the chat. It does NOT fire the onClick
// handlers (those decide/execute through the live kernel + resume the loop) — the
// auto-resume wiring is exercised by the backend continuation tests
// (approved_tool_result_folds_into_the_waiting_continuation) and the continue route.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-approval-continuation-render.test.mjs`.

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
import { PrimeTurnCard } from "./Prime.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
const noop = () => {};
const base = {
  intent: "tool_invocation",
  reply: "That tool needs your approval first.",
  disposition: "awaiting_approval",
  action: null,
  created_task: null,
  started_run: null,
  created_agent: null,
  approval: "appr_1",
};
const APPROVAL_REQUEST = {
  approval_id: "appr_1",
  label: "mcp:notes/delete",
  plugin_id: "mcp:notes",
  tool_name: "delete",
  source: "mcp",
  server: "notes",
  risk: "high",
  reason: "destructive write",
  args_preview: "{ id }",
  permission: "tool:mcp:notes:delete",
  allow_always_supported: true,
};
const PAUSED_ON_APPROVAL = {
  ...base,
  pending_tool_approval: APPROVAL_REQUEST,
  prime_continuation: {
    id: "cont_0001",
    reason: "a tool needs approval",
    observation_count: 2,
    extended_used: false,
    awaiting_approval: true,
  },
};
const PAUSED_ON_LIMIT = {
  ...base,
  reply: "I reached the tool-call limit.",
  disposition: "needs_clarification",
  prime_continuation: {
    id: "cont_0002",
    reason: "tool-call limit",
    observation_count: 3,
    extended_used: false,
    awaiting_approval: false,
  },
};
export function renderPausedOnApproval() {
  return at(<PrimeTurnCard turn={PAUSED_ON_APPROVAL} busy={false} onSuggestion={noop} onContinue={noop} />);
}
export function renderPausedOnLimit() {
  return at(<PrimeTurnCard turn={PAUSED_ON_LIMIT} busy={false} onSuggestion={noop} onContinue={noop} />);
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
      sourcefile: "prime-approval-continuation-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-approval-continuation-render-"));
  const out = join(tmp, "prime-approval-continuation-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a paused-on-approval turn renders the approval card AND the auto-continue affordance", () => {
  const html = mod.renderPausedOnApproval();
  // The per-call approval card is present with its full action set (nothing ran yet).
  assert.match(html, /Approve &amp; run/);
  assert.match(html, /Allow always/);
  assert.match(html, /Deny/);
  // The bound tool is identified so the operator knows what runs.
  assert.match(html, /mcp:notes\/delete/);
  // The continuation strip promises an AUTOMATIC continue once the tool runs — the chat does
  // not dead-end into "type another message".
  assert.match(html, /continue automatically with its result/i);
  // The paused badge reflects the gathered observations.
  assert.match(html, /paused/i);
  assert.match(html, /gathered/i);
});

test("a limit-paused turn (no approval) renders a plain Keep working button, no approval card", () => {
  const html = mod.renderPausedOnLimit();
  assert.match(html, /Keep working/);
  // No approval card on a limit pause.
  assert.doesNotMatch(html, /Approve &amp; run/);
});
