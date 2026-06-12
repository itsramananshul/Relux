// Render/DOM verification for the §3.3b cheap DIAGNOSTIC PASS on the RECOVERY
// DECISION CARD (docs/relix-dashboard-design.md §6.10; relix-execution-and-issue
// §3.3b "the diagnostic LLM pass that writes a richer narrative root cause"). Drives
// the REAL `RecoveryCard` and asserts: the "Analyze failure" action renders as a
// wired button; a model narrative renders inline with its provenance; an
// "unavailable" result reads as an honest note; an error surfaces its reason; and a
// loading state shows progress. The request/redaction/bounding is pinned server-side
// (run_diagnosis.rs); this pins what the operator SEES.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-diagnose-render.test.mjs`.

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
import { assessRunRecovery } from "../recovery.ts";

function run(over) {
  return {
    id: "run_0001", task_id: "task_0001", agent_id: "prime",
    adapter_plugin: "claude-cli", status: "failed", ...over,
  };
}

function card(diagnostic) {
  const assessment = assessRunRecovery(run({ failure_class: "auth_required" }));
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard
        assessment={assessment}
        diagnostic={diagnostic}
        handlers={{
          configure_agent: { to: "/settings" },
          analyze: { onClick: () => {} },
          investigate: { onClick: () => {} },
        }}
      />
    </StaticRouter>
  );
}

// Analyze wired, no result yet.
export function renderIdle() { return card(null); }
// In-flight.
export function renderLoading() { return card({ status: "loading" }); }
// A model narrative came back.
export function renderModel() {
  return card({
    status: "done",
    result: {
      run_id: "run_0001", mode: "model", model: "openai/gpt-4o-mini",
      narrative: "Likely cause: the provider credential was rejected.",
      provider_configured: true,
    },
  });
}
// No provider configured → the honest fallback.
export function renderUnavailable() {
  return card({
    status: "done",
    result: {
      run_id: "run_0001", mode: "unavailable",
      narrative: "No diagnostic model is configured, so a written narrative is not available.",
      provider_configured: false,
    },
  });
}
// The call errored.
export function renderError() {
  return card({ status: "error", message: "network down" });
}

// Analyze UNWIRED → degrades to a muted pointer, never a dead button.
export function renderUnwired() {
  const assessment = assessRunRecovery(run({ failure_class: "auth_required" }));
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard assessment={assessment} handlers={{ configure_agent: { to: "/settings" } }} />
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
      sourcefile: "work-diagnose-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-diagnose-render-"));
  const out = join(tmp, "work-diagnose-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a wired Analyze renders as a button labelled 'Analyze failure'", () => {
  const html = mod.renderIdle();
  assert.match(html, /<button[^>]*>Analyze failure<\/button>/);
});

test("an unwired Analyze degrades to a muted pointer, not a dead button", () => {
  const html = mod.renderUnwired();
  assert.match(html, /→ Analyze failure/);
  assert.doesNotMatch(html, /<button[^>]*>Analyze failure<\/button>/);
});

test("a model narrative renders inline with its provenance, below the card", () => {
  const html = mod.renderModel();
  assert.match(html, /Diagnostic narrative/);
  assert.match(html, /openai\/gpt-4o-mini/);
  assert.match(html, /the provider credential was rejected/);
  // The deterministic card stays visible above it (root cause is still shown).
  assert.match(html, /Root cause:/);
});

test("an unavailable result reads as an honest note (no fabricated diagnosis)", () => {
  const html = mod.renderUnavailable();
  assert.match(html, /Diagnostic unavailable/);
  assert.match(html, /No diagnostic model is configured/);
  // No model provenance label when unavailable.
  assert.doesNotMatch(html, /Diagnostic narrative/);
});

test("a loading state shows progress and an error surfaces its reason", () => {
  assert.match(mod.renderLoading(), /Analyzing the failure/);
  assert.match(mod.renderError(), /Analysis failed: network down/);
});
