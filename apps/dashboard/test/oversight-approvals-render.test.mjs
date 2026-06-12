// Render/DOM verification for the INLINE approval controls in the Work oversight
// strip (OversightApprovalRow). The first-paint Work render test cannot seed a
// pending approval (useEffect never fires under renderToStaticMarkup, so the
// composed /oversight read never populates), so this renders the real
// OversightApprovalRow directly with a seeded approval and asserts the correct
// inline action set appears per approval shape.
//
// It transpiles the REAL component from Work.tsx with the esbuild Vite vendors and
// server-renders it through react-dom/server + react-router's StaticRouter (the
// same declarative-router family the app uses), so a render-time throw fails here
// exactly as it would white-screen the strip. It does NOT fire the onClick handlers
// (those mutate/execute through the live kernel) — the wiring of each button to its
// reluxApprovals route is covered by the action-model unit test + the existing
// backend approval route tests; see the browser-smoke note for why a live click is
// not seeded here.
//
// Run: `npm test` (auto-discovered) or `node --test test/oversight-approvals-render.test.mjs`.

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

// Render OversightApprovalRow inside a StaticRouter (it uses <Link>). onReload is a
// no-op — we assert the rendered controls, not the click behavior.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { OversightApprovalRow } from "./Work.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/work">{el}</StaticRouter>);
}
const noop = () => {};
const TI = {
  id: "appr_ti",
  requested_by: "prime",
  action: "delete the staging bucket",
  reason: "high-risk write",
  risk: "high",
  status: "pending",
  created_at: "2026-06-12T00:00:00Z",
  tool_invocation: {
    plugin_id: "relux-tools-github",
    tool_name: "delete_repo",
    agent_id: "agent_7",
    permission: "tool:relux-tools-github:delete_repo",
    risk: "high",
    args_preview: "{ repo }",
    args_sha256: "abc123",
    consumed: false,
    executable: false,
  },
};
const GENERIC = {
  id: "appr_generic",
  requested_by: "prime",
  action: "promote to production",
  reason: "needs sign-off",
  risk: "medium",
  status: "pending",
  created_at: "2026-06-12T00:00:00Z",
};
export function renderToolInvocation() { return at(<OversightApprovalRow approval={TI} onReload={noop} />); }
export function renderGeneric() { return at(<OversightApprovalRow approval={GENERIC} onReload={noop} />); }
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "oversight-approvals-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-oversight-approvals-render-"));
  const out = join(tmp, "oversight-approvals-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a per-call tool-invocation approval renders the full inline action set", () => {
  const html = mod.renderToolInvocation();
  // The action + the detailed-surface link are always present.
  assert.match(html, /delete the staging bucket/);
  assert.match(html, /Open/);
  // The full common set is inline.
  assert.match(html, /Approve &amp; run/);
  assert.match(html, /Allow always/);
  assert.match(html, /Deny/);
  // The bound tool is identified (so the operator knows what runs).
  assert.match(html, /delete_repo/);
});

test("a generic approval renders Approve + Deny only (no run, no allow-always)", () => {
  const html = mod.renderGeneric();
  assert.match(html, /promote to production/);
  assert.match(html, /Open/);
  // Plain Approve (not "Approve & run") and Deny are offered.
  assert.match(html, /Approve(?!&amp; run)/);
  assert.match(html, /Deny/);
  // Allow-always must NOT appear for a generic approval.
  assert.doesNotMatch(html, /Allow always/);
  // The honest "nothing runs here" caveat is surfaced.
  assert.match(html, /nothing runs here/i);
});
