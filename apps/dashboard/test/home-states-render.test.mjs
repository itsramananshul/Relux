// State-driven render verification for the Home launchpad (RELUX_MASTER_PLAN §22:
// "a dynamic first-run checklist based on current system state"). Unlike
// readiness-guide-render.test.mjs (which feeds the component hand-built reports to
// pin its three display modes), this drives the REAL `buildReadiness` derivation
// from crafted control-plane reads and renders the REAL ReadinessGuide, so the
// whole Home pipeline — inputs → report → DOM → first action — is proven for each
// state a new operator actually lands in:
//   1. fallback brain / no tools   2. real brain, tools ready
//   3. pending approval            4. a paused Prime continuation
//   5. blocked / failed work
//
// Run: `npm test` (auto-discovered) or `node --test test/home-states-render.test.mjs`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as esbuild from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const dashboardRoot = resolve(here, "..");
const componentsDir = join(dashboardRoot, "src", "components");

// The entry composes the real derivation + component. Builders mirror the runtime
// fields the pure functions read (the bundle is not type-checked).
const RENDER_ENTRY = `
import { renderToStaticMarkup } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";
import { ReadinessGuide } from "./ReadinessGuide.tsx";
import { buildReadiness } from "../readiness.ts";

const baseState = {
  db_path: "x", plugins: 0, installed_plugins: 0, namespaces: 1,
  agents: 0, tasks: 0, runs: 0, approvals: 0, open_tasks: 0,
  active_runs: 0, waiting_approval: 0, blocked: 0, failed: 0, pending_approvals: 0,
};
const state = (o = {}) => ({ ...baseState, ...o });
const ai = (o = {}) => ({
  mode: "deterministic", brain: "local", configured: false, disabled: false,
  model: "m", timeout_ms: 30000, reason: "r", ...o,
});
const tool = (executable) => ({
  plugin_id: "p", tool_name: "do", description: "", permission: "p.do", risk: "low",
  source_kind: "Github", installed: true, enabled: true, protected: false, executable,
});
const continuation = (o = {}) => ({
  id: "cont_1", reason: "ceiling reached", observation_count: 2,
  extended_used: false, awaiting_approval: false, ...o,
});
// A ready Claude CLI adapter — a claude_cli brain reads as connected only when its
// adapter is actually runnable (on PATH + enabled), mirroring primeBrainStep.
const claudeAdapter = () => ({
  plugin_id: "relux-adapter-claude-cli", adapter_name: "claude", kind: "adapter",
  configured: true, enabled: true, command: "claude", available_on_path: true,
  resolved_path: "/usr/bin/claude", timeout_seconds: 120, max_output_bytes: 65536,
  working_dir: null, state: "available", detail: "",
});

function render(inputs) {
  const report = buildReadiness(inputs);
  return renderToStaticMarkup(
    <StaticRouter location="/">
      <ReadinessGuide report={report} loading={false} onRefresh={() => {}} />
    </StaticRouter>
  );
}

export const fallbackNoTools = () =>
  render({ state: state(), ai: ai({ brain: "local" }), adapters: [], plugins: [], tools: [] });

export const realBrainReady = () =>
  render({
    state: state({ agents: 1, tasks: 2, open_tasks: 1 }),
    ai: ai({ brain: "claude_cli", mode: "claude_cli", configured: true }),
    adapters: [claudeAdapter()],
    plugins: [],
    tools: [tool("ready"), tool("ready")],
  });

export const pendingApproval = () =>
  render({ state: state({ tasks: 1, pending_approvals: 1 }), ai: ai({ brain: "local" }), adapters: [], plugins: [], tools: [] });

export const pausedContinuation = () =>
  render({ state: state({ tasks: 1 }), ai: ai({ brain: "local" }), adapters: [], plugins: [], tools: [], continuation: continuation() });

export const blockedWork = () =>
  render({ state: state({ tasks: 2, blocked: 1, failed: 1 }), ai: ai({ brain: "local" }), adapters: [], plugins: [], tools: [] });
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: componentsDir,
      loader: "tsx",
      sourcefile: "home-states-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-home-states-render-"));
  const out = join(tmp, "home-states-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("fallback brain / no tools: operational, honestly labels Local, invites the first turn", () => {
  const html = mod.fallbackNoTools();
  // A local brain works — the page is operational, never "setup needed".
  assert.match(html, /operational/);
  assert.doesNotMatch(html, /setup needed/);
  // The fallback is labelled honestly, never sold as the main path.
  assert.match(html, /Local \(deterministic\)/);
  // The guided first action is to ask Prime to start.
  assert.match(html, /Ask Prime to start your first task/);
});

test("real brain, tools ready: operational summary names the brain and ready tools", () => {
  const html = mod.realBrainReady();
  assert.match(html, /operational/);
  assert.match(html, /Set up —/);
  assert.match(html, /Claude CLI/);
  assert.match(html, /2 tools ready/);
});

test("pending approval: surfaces the decision and routes the first action to Approvals", () => {
  const html = mod.pendingApproval();
  assert.match(html, /Pending approvals/);
  assert.match(html, /Review 1 pending approval/);
});

test("paused continuation: surfaces resume-paused-work and routes the first action to Work", () => {
  const html = mod.pausedContinuation();
  assert.match(html, /Resume paused work/);
});

test("blocked / failed work: surfaces the attention row and routes the first action to the Inbox", () => {
  const html = mod.blockedWork();
  assert.match(html, /Work needs attention/);
  assert.match(html, /Inspect work that needs attention/);
});
