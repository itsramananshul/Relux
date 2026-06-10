// Render/DOM verification for the session-expiry surfaces (RELUX_MASTER_PLAN
// "Local operator login v1"). The pure DECISION helpers in `account.ts` are unit
// tested (account.test.ts), but those tests never prove the React components
// actually RENDER the warning chip and the promoted re-auth banner/button — the
// thing a user sees is a missing chip when their session is about to lapse, or a
// re-auth callout that fires under the dev bypass when it must stay silent. A
// pure-function test cannot catch a JSX wiring regression there.
//
// This harness closes that gap WITHOUT a browser and WITHOUT new dependencies,
// exactly as render-interrupted.test.mjs does:
//   1. Render path — it transpiles the REAL `SessionWarnChip` (ReluxShell) and
//      `AccountReauth` (AccountPanel) with the esbuild Vite already vendors, then
//      server-renders them through react-dom/server. These are pure,
//      presentational components that take the already-decided warning/callout as
//      props, so SSR (which never runs effects) genuinely exercises the JSX
//      conditional — a regression that hides the chip, drops the re-auth banner,
//      or shows either under the dev bypass / an older kernel fails here.
//   2. Shipped-bundle path — it reads the COMMITTED bundle the kernel serves
//      (`crates/relix-web-bridge/dashboard-dist`) and asserts the chip + re-auth
//      copy is present, catching a STALE dist (source changed, bundle not rebuilt
//      → served UI missing the chip/callout).
//
// Run: `npm test` (auto-discovered) or `node --test test/account-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const componentsDir = join(dashboardRoot, "src", "components");
const repoRoot = resolve(dashboardRoot, "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

// ── Render path: transpile + server-render the real components ──────────────

// A tiny entry that drives the REAL presentational components through the REAL
// decision helpers (sessionWarning / reauthCallout), so the test feeds raw
// session metadata and the genuine product logic decides what renders. Neither
// component uses <Link>/router context, so no StaticRouter is needed.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { SessionWarnChip } from "./ReluxShell.tsx";
import { AccountReauth } from "./AccountPanel.tsx";
import { sessionWarning, reauthCallout } from "../account.ts";
const noop = () => {};
export function renderChip(meta, elapsed = 0) {
  return renderToStaticMarkup(
    <SessionWarnChip warn={sessionWarning(meta, elapsed)} onOpen={noop} />
  );
}
export function renderReauth(meta, elapsed = 0) {
  return renderToStaticMarkup(
    <AccountReauth
      authDisabled={!!meta.auth_disabled}
      callout={reauthCallout(meta, elapsed)}
      reauthErr={null}
      signingOut={false}
      onReauth={noop}
    />
  );
}
`;

let tmp = null;
let renderChip = null;
let renderReauth = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: componentsDir,
      loader: "tsx",
      sourcefile: "account-render-entry.tsx",
    },
    bundle: true,
    // CJS + node platform so the bundled react-dom/server keeps native `require`
    // for node builtins; one bundled React copy is shared so hooks stay consistent.
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-account-render-"));
  const out = join(tmp, "account-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  ({ renderChip, renderReauth } = await import(pathToFileURL(out).href));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

// Metadata fixtures mirror those pinned in account.test.ts.
const COMFORTABLE = {
  username: "ops",
  idle_expires_in_secs: 12 * 3600,
  absolute_expires_in_secs: 7 * 86400,
  idle_timeout_secs: 12 * 3600,
  absolute_max_secs: 7 * 86400,
};
const ABS_CLOSE = { username: "ops", absolute_expires_in_secs: 20 * 60 };
const IDLE_CLOSE = { username: "ops", idle_expires_in_secs: 8 * 60 };
const BYPASS = { username: "ops", auth_disabled: true, absolute_expires_in_secs: 60 };
const OLD_KERNEL = { username: "ops" };

// ── Expiry chip ─────────────────────────────────────────────────────────────

test("the chip RENDERS the absolute (hard) warning copy when the ceiling is close", () => {
  const html = renderChip(ABS_CLOSE);
  // The hard-ceiling variant carries the extra class and the re-sign-in copy.
  assert.match(html, /class="session-warn-chip hard"/);
  assert.match(html, /Re-sign-in required in 20m/);
  // The status dot is present and the chip is a real, clickable button.
  assert.match(html, /<span class="dot" aria-hidden="true">/);
  assert.match(html, /<button[^>]*>/);
});

test("the chip RENDERS the idle warning copy (no hard class) when inactivity is close", () => {
  const html = renderChip(IDLE_CLOSE);
  // Idle variant: base class only, inactivity copy with the countdown.
  assert.match(html, /class="session-warn-chip"/);
  assert.doesNotMatch(html, /session-warn-chip hard/);
  assert.match(html, /Signs out for inactivity in 8m/);
});

test("the chip renders NOTHING when both windows are comfortably open", () => {
  assert.equal(renderChip(COMFORTABLE), "");
});

test("the chip stays silent under the dev bypass and for an older kernel", () => {
  // RELUX_AUTH_DISABLED sends no real deadlines → no chip even with 60s nominal.
  assert.equal(renderChip(BYPASS), "");
  // An older kernel sends only { username } → no chip, no invented countdown.
  assert.equal(renderChip(OLD_KERNEL), "");
});

// ── Account re-auth promotion ────────────────────────────────────────────────

test("the Account re-auth PROMOTES (banner + primary button) when the ceiling is close", () => {
  const html = renderReauth(ABS_CLOSE);
  // The emphasised alert banner with the countdown + the "only a fresh sign-in" line.
  assert.match(html, /class="banner err" role="alert"/);
  assert.match(html, /Re-sign-in required in 20m/);
  assert.match(html, /Only a fresh sign-in extends it/);
  // Promoted button is PRIMARY (no ghost modifier) and clearly labelled.
  assert.match(html, /class="btn sm"/);
  assert.match(html, /Sign out and sign back in/);
});

test("the Account re-auth stays UNADORNED (ghost button, no banner) when the ceiling is far off", () => {
  const html = renderReauth(COMFORTABLE);
  // The button is always present — re-auth is the one path that clears the cap —
  // but without the alert banner and rendered as a quiet ghost button.
  assert.match(html, /class="btn ghost sm"/);
  assert.match(html, /Sign out and sign back in/);
  assert.doesNotMatch(html, /role="alert"/);
  assert.doesNotMatch(html, /Only a fresh sign-in extends it/);
});

test("the Account re-auth stays unadorned for an older kernel (no deadlines)", () => {
  const html = renderReauth(OLD_KERNEL);
  assert.match(html, /class="btn ghost sm"/);
  assert.doesNotMatch(html, /role="alert"/);
});

test("the Account re-auth is HIDDEN entirely under the dev bypass", () => {
  // No absolute ceiling to clear when expiry is disabled → the whole section is gone.
  assert.equal(renderReauth(BYPASS), "");
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

function shippedBundle() {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  return jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
}

test("the shipped JS bundle carries the session-expiry chip copy (no stale dist)", () => {
  const bundle = shippedBundle();
  // ASCII-only fragments survive minification; if the source gained the chip but
  // the committed bundle was never rebuilt, these are absent → fail.
  assert.match(bundle, /session-warn-chip/);
  assert.match(bundle, /Re-sign-in required in /);
  assert.match(bundle, /Signs out for inactivity in /);
  assert.match(bundle, /reaches its hard 7-day limit soon/);
});

test("the shipped JS bundle carries the Account re-auth promotion copy (no stale dist)", () => {
  const bundle = shippedBundle();
  assert.match(bundle, /Sign out and sign back in/);
  assert.match(bundle, /Only a fresh sign-in extends it/);
  assert.match(bundle, /Ends this session and shows the sign-in screen/);
});
