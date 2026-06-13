// Render/DOM verification for the post-configure "Prime can use this now" cue
// (PrimeCanUseNowBanner), docs/prime-tool-use.md "The verified install → use path"
// §4 See it / §5 Use it + "Tools Prime can use"; RELUX_MASTER_PLAN §11.6,
// §10.1/§10.5/§17.1.
//
// A pure-function test pins the cue copy (plugins.test.ts `primeUseCue`); only an
// actual render proves the banner mounts (including its useAsync catalog re-pull)
// without a render-time throw, shows the natural chat phrase, links to Prime, and
// stays honest about gating. useEffect does not fire under renderToStaticMarkup, so
// the catalog re-pull stays in its initial "refreshing…" state — exactly what we
// want to assert it renders cleanly without a live fetch. Mirrors
// install-to-usable-render.test.mjs.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-use-cue-render.test.mjs`.

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
import { PrimeCanUseNowBanner } from "./Plugins.tsx";

function banner(toolName, kind) {
  return renderToStaticMarkup(
    <StaticRouter location="/plugins">
      <PrimeCanUseNowBanner toolName={toolName} kind={kind} />
    </StaticRouter>
  );
}

export function renderCommandTool() {
  return banner("repo.build", "command_tool");
}
export function renderMcpServer() {
  return banner("cool-mcp", "mcp_server");
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
      sourcefile: "prime-use-cue-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-use-cue-render-"));
  const out = join(tmp, "prime-use-cue-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a configured command tool shows the Prime-use cue with the exact chat phrase + Prime link", () => {
  const html = mod.renderCommandTool();
  assert.match(html, /Prime can use this now/);
  // The exact natural-language phrase to type at Prime.
  assert.match(html, /run the repo\.build tool/);
  // Honest about gating — pauses for approval, never a faked auto-run.
  assert.match(html, /approval/i);
  assert.match(html, /Nothing runs until you approve/i);
  // A link to Prime so the operator can act immediately.
  assert.match(html, /href="\/prime"/);
  assert.match(html, /Open Prime/);
});

test("the MCP variant points at discovery and names the server in the chat phrase", () => {
  const html = mod.renderMcpServer();
  assert.match(html, /Prime can use this now/);
  assert.match(html, /use the cool-mcp tools/);
  assert.match(html, /[Dd]iscover/);
  assert.match(html, /href="\/prime"/);
});
