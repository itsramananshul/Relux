// Render/DOM verification for the GUIDED POST-ACTIVATION MCP DISCOVERY panel
// (Prime.tsx → CandidateActivationResult → McpDiscoveryResult). After Prime registers an
// MCP candidate, the configure-candidate result carries a bounded `tools/list` probe so
// the user sees what Prime can now use — or what's missing — without driving a separate
// Discover. The live CandidateActivation sets `result` from the POST (a static render does
// not run that), so this mounts the exported CandidateActivationResult directly with a
// fabricated result and asserts:
//   - a reachable result lists the discovered tools with their gated/runnable chips,
//   - the honest next_step + "N tools found" / "N gated" summary render,
//   - an unreachable result shows the actionable guidance + sanitized error (no fake tools),
//   - a command_tool result renders NO MCP discovery panel (no false positive).
//
// It transpiles the REAL component from Prime.tsx with esbuild + server-renders it through
// react-dom/server + StaticRouter, so a render-time throw fails here exactly as it would
// white-screen the chat.
//
// Run: `npm test` (auto-discovered) or `node --test test/prime-mcp-post-activation-render.test.mjs`.

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
import { CandidateActivationResult } from "./Prime.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
const mcpTool = (name, executable, description) => ({
  plugin_id: "mcp:gh",
  tool_name: name,
  description: description ?? "",
  permission: "tool:mcp-gh:" + name,
  risk: "medium",
  source_kind: "Mcp",
  installed: true,
  enabled: true,
  protected: false,
  executable,
});
// A reachable mcp_register result: two tools found, both gated.
const REACHABLE = {
  plugin_id: "hermes-agent",
  plugin_name: "hermes-agent",
  candidate_id: "mcp",
  kind: "mcp_stdio",
  activation: "mcp_register",
  mcp_server: { id: "gh", transport: "managed_stdio", endpoint: "", description: "", enabled: true, timeout_ms: 5000, status: "configured", tool_overrides: {} },
  mcp_discovery: {
    reachable: true,
    tool_count: 2,
    gated_count: 2,
    guidance: "Registered MCP server \\"gh\\" and discovered 2 tool(s): create_issue, list_repos. Each stays gated until you classify it — ask me to use the gh tools and I'll stage the approval before the first one runs.",
    tools: [mcpTool("create_issue", "needs_approval", "Open a new issue"), mcpTool("list_repos", "needs_approval")],
  },
  tool_name: "gh",
  next_step: "Registered MCP server \\"gh\\" and discovered 2 tool(s): create_issue, list_repos. Each stays gated until you classify it — ask me to use the gh tools and I'll stage the approval before the first one runs.",
  no_code_executed: true,
  approval_closed: true,
};
// An unreachable mcp_register result: needs secrets, no fabricated tools.
const UNREACHABLE = {
  ...REACHABLE,
  mcp_discovery: {
    reachable: false,
    tool_count: 0,
    gated_count: 0,
    guidance: "Registered MCP server \\"gh\\", but a tools/list probe couldn't reach it yet — it expects secrets (GITHUB_TOKEN). Map ENV_VAR=secret_name on the MCP page, then Discover to list its tools.",
    tools: [],
    error: "missing secret 'gh' for env var 'GITHUB_TOKEN'",
  },
  next_step: "Registered MCP server \\"gh\\", but a tools/list probe couldn't reach it yet — it expects secrets (GITHUB_TOKEN). Map ENV_VAR=secret_name on the MCP page, then Discover to list its tools.",
};
// A command_tool result: no MCP discovery panel at all.
const COMMAND_TOOL = {
  plugin_id: "plain",
  plugin_name: "plain",
  candidate_id: "command",
  kind: "cli_command",
  activation: "command_tool",
  tool_name: "plain.run",
  next_step: "Configured command tool \\"plain.run\\". It is gated (needs approval) — ask me to run the plain.run tool and I'll stage the approval before it runs.",
  no_code_executed: true,
  approval_closed: false,
};
export function renderReachable() { return at(<CandidateActivationResult result={REACHABLE} />); }
export function renderUnreachable() { return at(<CandidateActivationResult result={UNREACHABLE} />); }
export function renderCommandTool() { return at(<CandidateActivationResult result={COMMAND_TOOL} />); }
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "prime-mcp-post-activation-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-prime-mcp-post-activation-render-"));
  const out = join(tmp, "prime-mcp-post-activation-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a reachable MCP activation lists the discovered tools with gated chips + summary", () => {
  const html = mod.renderReachable();
  // The discovered tool names render.
  assert.match(html, /create_issue/);
  assert.match(html, /list_repos/);
  // Each is shown as gated (needs approval), never silently runnable.
  assert.match(html, /gated/);
  // The "N tools found" / "N gated" summary chips render.
  assert.match(html, /2 tools found/);
  assert.match(html, /2 gated/);
  // The honest, concrete "use the <server> tools" next step is present (mirrors the
  // Plugins page primeUseCue mcp_server phrase, never a vague "ask me to use it").
  assert.match(html, /use the gh tools/i);
});

test("an unreachable MCP activation shows guidance + sanitized error, no fabricated tools", () => {
  const html = mod.renderUnreachable();
  assert.match(html, /not reachable yet/i);
  // Actionable guidance: map secrets, then Discover.
  assert.match(html, /GITHUB_TOKEN/);
  assert.match(html, /Discover/);
  // The sanitized, value-free error reason is surfaced.
  assert.match(html, /missing secret/);
  // No tool names were fabricated.
  assert.doesNotMatch(html, /create_issue/);
});

test("a command-tool activation renders NO MCP discovery panel", () => {
  const html = mod.renderCommandTool();
  assert.match(html, /plain\.run/);
  assert.doesNotMatch(html, /tools found/i);
  assert.doesNotMatch(html, /not reachable yet/i);
  // The concrete next step names the exact phrase to try at Prime ("run the <tool> tool"),
  // mirroring the Plugins page primeUseCue — never a vague "ask me to use it".
  assert.match(html, /run the plain\.run tool/i);
});
