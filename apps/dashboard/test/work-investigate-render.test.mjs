// Render/DOM verification for the "Investigate with Prime" choice on the RECOVERY
// DECISION CARD (§3.3b chat companion; docs/relix-dashboard-design.md §6.10). Drives
// the REAL `RecoveryCard` with REAL assessments and asserts the investigate action
// renders as a WIRED BUTTON when the surface supplies an onClick, and as a muted
// POINTER when it does not (never a dead button). The seed building + consume-once
// semantics are pinned by investigateseed.test.ts; this pins what the operator SEES.
//
// Run: `npm test` (auto-discovered) or `node --test test/work-investigate-render.test.mjs`.

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

// A failed run with Investigate WIRED as a button (the real Work panel wiring).
export function renderWired() {
  const assessment = assessRunRecovery(run({ failure_class: "auth_required" }));
  return renderToStaticMarkup(
    <StaticRouter location="/work">
      <RecoveryCard
        assessment={assessment}
        handlers={{
          configure_agent: { to: "/settings" },
          investigate: { onClick: () => {} },
        }}
      />
    </StaticRouter>
  );
}

// The same card with Investigate UNWIRED → it must degrade to a muted pointer.
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
      sourcefile: "work-investigate-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-work-investigate-render-"));
  const out = join(tmp, "work-investigate-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a wired Investigate renders as a button labelled 'Investigate with Prime'", () => {
  const html = mod.renderWired();
  assert.match(html, /<button[^>]*>Investigate with Prime<\/button>/);
});

test("an unwired Investigate degrades to a muted pointer, not a dead button", () => {
  const html = mod.renderUnwired();
  assert.match(html, /→ Investigate with Prime/);
  assert.doesNotMatch(html, /<button[^>]*>Investigate with Prime<\/button>/);
});
