// Render/DOM verification for the "install-to-usable" Detected Capabilities panel
// (DetectedCapabilities + CapabilityCard), RELUX_MASTER_PLAN §8.2 "Converting an
// imported repo into a real plugin / tool / MCP config".
//
// A pure-function test pins the candidate helpers (plugins.test.ts), but only an
// actual render proves the panel mounts the per-candidate Configure paths without a
// render-time throw — that an MCP candidate offers a one-click register button, that a
// command-line candidate offers a governed Configure (command tool) path, that a CLI
// candidate with no inferable command stays an honest manual pending capability, and
// that a scanned-but-empty source shows exact "what to add" guidance instead of a dead
// end. Mirrors install-result-render.test.mjs: esbuild transpiles the REAL component
// and react-dom/server + StaticRouter render it. useEffect does not fire under
// renderToStaticMarkup, so we pass the hints payload directly (no fetch needed).
//
// Run: `npm test` (auto-discovered) or `node --test test/install-to-usable-render.test.mjs`.

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
import { DetectedCapabilities } from "./Plugins.tsx";

const plugin = {
  id: "relux-plugin-cool-mcp",
  name: "cool-mcp",
  description: "",
  kind: "ToolSet",
  version: "0.1.0",
  enabled: true,
  source_kind: "Github",
  source_label: "https://github.com/owner/cool-mcp",
  install_dir: "/data/relux/plugins/relux-plugin-cool-mcp",
  protected: false,
  bundled: false,
  generated: true,
  tool_count: 0,
};

const mcpCandidate = {
  id: "mcp-server",
  kind: "mcp_stdio",
  title: "MCP server (stdio)",
  confidence: "high",
  risk: "medium",
  rationale: "depends on @modelcontextprotocol/sdk",
  command_preview: "node ./dist/server.js",
  env_placeholders: ["GITHUB_TOKEN"],
  activation: "mcp_register",
  mcp_registration: {
    suggested_id: "cool-mcp",
    suggested_description: "",
    endpoint_required: true,
    suggested_transport: "managed_stdio",
    detected_command: "node",
    detected_args: ["./dist/server.js"],
    notes: [],
  },
  next_steps: ["Open the review form."],
};

const cliCandidate = {
  id: "cli-bin-tool",
  kind: "cli_command",
  title: "Command-line tool (npm bin)",
  confidence: "medium",
  risk: "medium",
  rationale: "package.json declares a bin entrypoint 'tool'",
  command_preview: "node ./cli.js",
  env_placeholders: [],
  activation: "command_tool",
  command_tool: {
    tool_name: "tool.run",
    program: "node",
    args: ["./cli.js"],
    description: "Command-line tool (npm bin)",
  },
  next_steps: ["Click Configure to open a pre-filled, reviewable command-tool form."],
};

const manualCandidate = {
  id: "cli-unknown",
  kind: "cli_command",
  title: "Command-line tool (unknown)",
  confidence: "low",
  risk: "medium",
  rationale: "an entrypoint was declared but no command could be inferred",
  env_placeholders: [],
  activation: "manual",
  next_steps: ["Run it yourself as a loopback server, then add a tool definition."],
};

function panel(hints) {
  return renderToStaticMarkup(
    <StaticRouter location="/plugins">
      <DetectedCapabilities plugin={plugin} hints={hints} loading={false} error={null} />
    </StaticRouter>
  );
}

export function renderWithCandidates() {
  return panel({
    plugin_id: plugin.id, install_dir: "/d", scanned: true, generated: true,
    hints: [], candidates: [mcpCandidate, cliCandidate, manualCandidate],
  });
}
export function renderEmptyAfterScan() {
  return panel({
    plugin_id: plugin.id, install_dir: "/d", scanned: true, generated: true,
    hints: [], candidates: [],
  });
}
export function renderUnscanned() {
  return panel({
    plugin_id: plugin.id, install_dir: "/d", scanned: false, generated: true,
    hints: [], candidates: [],
  });
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
      sourcefile: "install-to-usable-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-install-to-usable-render-"));
  const out = join(tmp, "install-to-usable-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("the panel headlines the detected capability count, one-click + configurable badges", () => {
  const html = mod.renderWithCandidates();
  assert.match(html, /Detected 3 possible capabilities/);
  assert.match(html, /1 one-click/);
  assert.match(html, /1 configurable/);
});

test("an MCP candidate offers a one-click Configure (register) path", () => {
  const html = mod.renderWithCandidates();
  assert.match(html, /MCP server \(stdio\)/);
  assert.match(html, /Configure \(register MCP server\)/);
  // The command it will register as, and the env it expects (name only, never a value).
  assert.match(html, /node \.\/dist\/server\.js/);
  assert.match(html, /GITHUB_TOKEN/);
  assert.doesNotMatch(html, /ghp_/);
});

test("a command-line candidate offers a governed Configure (command tool) path", () => {
  const html = mod.renderWithCandidates();
  assert.match(html, /Command-line tool \(npm bin\)/);
  assert.match(html, /configurable/);
  assert.match(html, /Configure \(command tool\)/);
  // It is honest that the command runs argv-only and only when invoked (gated).
  assert.match(html, /Will run argv-only \(gated, never on import\)/);
});

test("a CLI candidate with NO inferable command stays an honest manual pending capability", () => {
  const html = mod.renderWithCandidates();
  assert.match(html, /Command-line tool \(unknown\)/);
  // The manual fallback still shows next steps, never a faked ready / Configure button.
  assert.match(html, /How to make this usable/);
});

test("a scanned source with NO candidates shows exact what-to-add guidance, not a dead end", () => {
  const html = mod.renderEmptyAfterScan();
  assert.match(html, /No runnable capability detected/);
  assert.match(html, /here.s exactly what to add/i);
  assert.match(html, /mcp\.json/);
  assert.match(html, /relux-plugin\.json/);
});

test("an unscanned source (outside the plugins root) renders nothing here (no false empty)", () => {
  const html = mod.renderUnscanned();
  assert.equal(html, "");
});
