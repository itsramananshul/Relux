// Render/DOM verification for the operator Doctor panel (relix-dashboard-design.md
// §15). doctor.test.ts pins the pure helpers; this harness proves the REAL
// component renders every state — loading, error, ok, warn, fail — and that the
// Health page mounts it. It mirrors health-render.test.mjs: transpile the real
// component with the esbuild Vite already vendors, server-render it under
// react-router's declarative StaticRouter, and assert the markup. It also asserts
// the COMMITTED bundle carries the panel, so a stale dist fails loudly.
//
// Run: `npm test` (auto-discovered) or `node --test test/doctor-render.test.mjs`.

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
const pagesDir = join(dashboardRoot, "src", "pages");
const repoRoot = resolve(dashboardRoot, "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

// Drive the presentational card with explicit props (no fetch) so every state is
// reachable under renderToStaticMarkup; also render the Health page to prove it
// mounts the DoctorPanel container in its honest loading state.
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { DoctorReportCard } from "../components/DoctorPanel.tsx";
import { Health } from "./Health.tsx";

const WARN_REPORT = {
  generated_at: 1,
  overall: "warn",
  summary: { ok: 4, info: 1, warn: 1, fail: 0 },
  checks: [
    { id: "kernel.store", label: "Kernel state store", severity: "ok",
      message: "Kernel state store opened and loaded successfully." },
    { id: "dashboard.bundle", label: "Dashboard bundle", severity: "warn",
      message: "Dashboard bundle not found.",
      remediation: "Run npm run build in apps/dashboard." },
    { id: "prime.brain", label: "Prime brain", severity: "info",
      message: "Prime is using the built-in local deterministic brain.",
      action_link: "/health" },
  ],
};

const FAIL_REPORT = {
  generated_at: 2,
  overall: "fail",
  summary: { ok: 5, info: 1, warn: 0, fail: 1 },
  checks: [
    { id: "prime.brain", label: "Prime brain", severity: "fail",
      message: "OpenRouter brain is selected but no API key is configured.",
      remediation: "Add the OpenRouter key in Health.", action_link: "/health" },
  ],
};

export function renderCard(props) {
  return renderToStaticMarkup(
    <StaticRouter location="/health"><DoctorReportCard {...props} /></StaticRouter>
  );
}
export function renderWarn() { return renderCard({ report: WARN_REPORT, onRefresh: () => {} }); }
export function renderFail() { return renderCard({ report: FAIL_REPORT, onRefresh: () => {} }); }
export function renderError() { return renderCard({ report: null, error: "boom", onRefresh: () => {} }); }
export function renderLoading() { return renderCard({ report: null, loading: true, onRefresh: () => {} }); }
export function renderHealth() {
  return renderToStaticMarkup(<StaticRouter location="/health"><Health /></StaticRouter>);
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
      sourcefile: "doctor-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-doctor-render-"));
  const out = join(tmp, "doctor-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("ok/warn report renders each row with its severity badge + action link", () => {
  const html = mod.renderWarn();
  assert.match(html, /Doctor/);
  assert.match(html, /Read-only diagnostics/);
  // The overall WARN badge and a warn-mapped row class are present.
  assert.match(html, /badge in_progress/);
  // An ok row maps to the "done" badge; an info row offers a Fix link.
  assert.match(html, /badge done/);
  assert.match(html, /Fix/);
  assert.match(html, /Run npm run build/); // remediation line shown
});

test("fail report renders a blocked badge and the failing message", () => {
  const html = mod.renderFail();
  assert.match(html, /badge blocked/);
  assert.match(html, /no API key is configured/);
});

test("error state shows an honest error, never a blank panel", () => {
  const html = mod.renderError();
  assert.match(html, /Could not run diagnostics/);
  assert.match(html, /boom/);
  // It must NOT claim a clean report.
  assert.doesNotMatch(html, /Read-only diagnostics/);
});

test("loading state shows the running affordance", () => {
  const html = mod.renderLoading();
  assert.match(html, /Running diagnostics/);
});

test("Health page mounts the Doctor panel in its loading state", () => {
  const html = mod.renderHealth();
  assert.match(html, /Doctor/);
  assert.match(html, /Running diagnostics/);
});

// ── Shipped-bundle path: the artifact the kernel actually serves ────────────

test("the committed dashboard bundle carries the Doctor panel", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  assert.match(bundle, /Read-only diagnostics/);
  assert.match(bundle, /\/v1\/relux\/doctor/);
});
