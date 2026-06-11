// Render/DOM verification for the token-authenticated manager-actions panel
// (docs/HERMES_OPENCLAW_DEEP_AUDIT.md §20 / §21). It server-renders the REAL exported
// `ManagerTokenActionsPanel` through the same esbuild + react-dom/server harness the Crew
// render test uses, and asserts the HONEST surface:
//   - it documents both agent-self routes (manager-grant + assign-task),
//   - the raw-token field is a password input the operator must paste (the dashboard never
//     reuses a minted token — only its hash is stored),
//   - the curl snippet embeds NO secret (the token is the $RELUX_AGENT_TOKEN shell var),
//   - on first render (nothing pasted) NO raw `relux_agt_` token appears in the markup.
// It also checks the committed bundle carries the panel copy (catches a stale dist).
// Run: `npm test` (auto-discovered) or `node --test test/manager-token-actions-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const pagesDir = join(dashboardRoot, "src", "pages");
const repoRoot = resolve(dashboardRoot, "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { ManagerTokenActionsPanel } from "./Crew.tsx";
export function render() {
  return renderToStaticMarkup(
    <ManagerTokenActionsPanel agentId="lead-1" targets={["ic-1", "ic-2"]} />
  );
}
`;

let tmp = null;
let render = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "mta-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-mta-render-"));
  const out = join(tmp, "mta-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ render } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the panel documents both token-authenticated manager routes", () => {
  const html = render();
  assert.match(html, /Manager actions \(token-authenticated\)/);
  assert.match(html, /\/v1\/relux\/agents\/me\/manager-grant/);
  assert.match(html, /\/v1\/relux\/agents\/me\/assign-task/);
  // The required scope is spelled out for THIS agent.
  assert.match(html, /agent:lead-1:subtree/);
});

test("the raw-token field is a password the operator must paste (never a stored token)", () => {
  const html = render();
  // The token input is type=password (not a value-bound text field that could echo a secret).
  assert.match(html, /type="password"/);
  // It documents the copy-once / hash-only honesty.
  assert.match(html, /shown <strong>once<\/strong> at mint|paste it yourself/);
});

test("the curl snippet embeds NO secret and shows the bearer-var shape", () => {
  const html = render();
  assert.match(html, /Bearer \$RELUX_AGENT_TOKEN/);
  // Nothing was pasted/minted, so no raw token (the prefix followed by token chars) must
  // ever appear in the markup — the ellipsis placeholder (`relux_agt_…`) is NOT a secret.
  assert.ok(
    !/relux_agt_[A-Za-z0-9]/.test(html),
    "no raw token may leak into the rendered panel",
  );
  // The placeholder hint uses the token shape but is just a placeholder, not a value.
  assert.match(html, /placeholder="relux_agt_/);
});

test("the target picker lists the manager's Branch operatives", () => {
  const html = render();
  assert.match(html, /ic-1/);
  assert.match(html, /ic-2/);
});

test("the panel offers BOTH token test forms (assign-task and manager-grant)", () => {
  const html = render();
  // Each action has its own collapsible test form summary + submit button.
  assert.match(html, /Test <[^>]*>assign-task<\/[^>]*> with a token/);
  assert.match(html, /Test <[^>]*>manager-grant<\/[^>]*> with a token/);
  assert.match(html, /Assign as manager \(token\)/);
  assert.match(html, /Grant as manager \(token\)/);
});

test("the manager-grant form has a permission field and an honest token-subject trust-boundary note", () => {
  const html = render();
  // A dedicated permission input (with the example placeholder) drives the grant action.
  assert.match(html, /Permission to grant:/);
  assert.match(html, /placeholder="e\.g\. tool:relux-tools-echo:say"/);
  // The trust boundary is spelled out: the token subject is the acting manager, the kernel
  // re-checks the grant_permission scope, and the operator cookie cannot stand in.
  assert.match(html, /token subject/);
  assert.match(html, /agent:lead-1:subtree:grant_permission/);
  // The grant form has its own paste-once password token field (id suffixed -grant-token).
  assert.match(html, /id="mta-lead-1-grant-token"/);
});

test("the committed dashboard bundle carries the manager-actions panel copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  assert.match(bundle, /Manager actions \(token-authenticated\)/);
  assert.match(bundle, /Assign as manager \(token\)/);
  assert.match(bundle, /Grant as manager \(token\)/);
  assert.match(bundle, /RELUX_AGENT_TOKEN/);
});
