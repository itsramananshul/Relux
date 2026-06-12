// Render/DOM verification for the INLINE approval decisions in a cross-Guild Inbox
// row (src/pages/Inbox.tsx → InboxRow, docs/relix-dashboard-design.md §6.11). The
// page-level inbox-render test cannot seed an item (useEffect, hence the /inbox
// fetch, never fires under renderToStaticMarkup), so this renders the real InboxRow
// directly with a seeded pending_approval item and asserts the correct inline action
// set appears per approval shape — and that an item WITHOUT the embedded approval
// degrades to the generic "Open approval" nav button (never a dead end).
//
// It transpiles the REAL component from Inbox.tsx with the esbuild Vite vendors and
// server-renders it through react-dom/server + react-router's StaticRouter (the same
// declarative-router family the app uses), so a render-time throw fails here exactly
// as it would white-screen the row. It does NOT fire the onClick handlers (those
// mutate/execute through the live kernel) — the wiring of each button to its
// reluxApprovals route is covered by the shared action-model unit test + the existing
// backend approval route tests.
//
// Run: `npm test` (auto-discovered) or `node --test test/inbox-approval-render.test.mjs`.

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

// Render InboxRow inside a StaticRouter (it uses <Link>/useNavigate). onActed is a
// no-op — we assert the rendered controls, not the click behavior.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { InboxRow } from "./Inbox.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/inbox">{el}</StaticRouter>);
}
const noop = () => {};

const TI_APPROVAL = {
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
const GENERIC_APPROVAL = {
  id: "appr_generic",
  requested_by: "prime",
  action: "promote to production",
  reason: "needs sign-off",
  risk: "medium",
  status: "pending",
  created_at: "2026-06-12T00:00:00Z",
};

function approvalItem(approval, severity) {
  return {
    id: "approval:" + approval.id,
    kind: "pending_approval",
    severity,
    title: "Approval: " + approval.action,
    summary: approval.reason,
    approval_id: approval.id,
    approval,
    actions: ["open_approval"],
    link: "/approvals",
  };
}

// An approval item the OLD backend would emit: no embedded record — the row must
// still offer the generic Open-approval action (the honest fallback, not a dead end).
const NO_EMBED_ITEM = {
  id: "approval:appr_old",
  kind: "pending_approval",
  severity: "warn",
  title: "Approval: rotate the signing key",
  summary: "scheduled rotation",
  approval_id: "appr_old",
  actions: ["open_approval"],
  link: "/approvals",
};

export function renderToolInvocation() {
  return at(<InboxRow item={approvalItem(TI_APPROVAL, "critical")} onActed={noop} />);
}
export function renderGeneric() {
  return at(<InboxRow item={approvalItem(GENERIC_APPROVAL, "warn")} onActed={noop} />);
}
export function renderNoEmbed() {
  return at(<InboxRow item={NO_EMBED_ITEM} onActed={noop} />);
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
      sourcefile: "inbox-approval-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-inbox-approval-render-"));
  const out = join(tmp, "inbox-approval-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a per-call tool-invocation approval renders the full inline action set in the row", () => {
  const html = mod.renderToolInvocation();
  // The item identity + the detailed-surface link are present.
  assert.match(html, /Approval: delete the staging bucket/);
  assert.match(html, /Open approval/);
  // The full common set is inline — approve & run / allow always / deny.
  assert.match(html, /Approve &amp; run/);
  assert.match(html, /Allow always/);
  assert.match(html, /Deny/);
});

test("a generic approval renders Approve + Deny only (no run, no allow-always)", () => {
  const html = mod.renderGeneric();
  assert.match(html, /Approval: promote to production/);
  assert.match(html, /Open approval/);
  assert.match(html, /Approve(?!&amp; run)/);
  assert.match(html, /Deny/);
  // Allow-always must NOT appear for a generic approval (its route 404s).
  assert.doesNotMatch(html, /Allow always/);
  // The honest "nothing runs here" caveat is surfaced.
  assert.match(html, /nothing runs here/i);
});

test("an approval item with no embedded record degrades to the generic Open action", () => {
  const html = mod.renderNoEmbed();
  assert.match(html, /Approval: rotate the signing key/);
  // The generic nav button (label "Open approval") is offered — never a dead end.
  assert.match(html, /Open approval/);
  // With no embedded record there are no inline decide buttons to render.
  assert.doesNotMatch(html, /Approve &amp; run/);
  assert.doesNotMatch(html, /Allow always/);
});
