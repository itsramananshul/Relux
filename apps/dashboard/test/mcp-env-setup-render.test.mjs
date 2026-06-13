// Render/DOM verification for the GUIDED MCP secret/env SETUP form
// (components/McpEnvSetupForm.tsx) and its wiring into Prime.tsx →
// CandidateActivationResult. When Prime registers an MCP candidate whose source declared
// env vars, the configure-candidate result carries a value-free `setup` requirement view;
// the chat then renders a form to supply/map the secrets and re-discover. This mounts the
// REAL exported components through react-dom/server + StaticRouter and asserts:
//   - a setup that needs work renders one input per required env var + the save button,
//   - a no-value-leak posture (the form never prints a secret value; only names/status),
//   - a READY setup renders NO form (nothing to set up — no false positive),
//   - a command_tool result (no setup) renders no setup form.
//
// Run: `npm test` (auto-discovered) or `node --test test/mcp-env-setup-render.test.mjs`.

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
import { McpEnvSetupForm } from "../components/McpEnvSetupForm.tsx";
function at(el) {
  return renderToStaticMarkup(<StaticRouter location="/prime">{el}</StaticRouter>);
}
const SETUP_NEEDS_WORK = {
  server_id: "gh",
  requirements: [
    { env_var: "OPENAI_API_KEY", required: true, description: "Expected by the imported MCP server — map it to a stored secret.", secret_mapped: false, secret_present: false },
  ],
  ready: false,
  missing: ["OPENAI_API_KEY"],
};
const SETUP_READY = {
  server_id: "gh",
  requirements: [
    { env_var: "OPENAI_API_KEY", required: true, description: "Mapped on this server's configuration.", secret_mapped: true, secret_name: "openai_key", secret_present: true },
  ],
  ready: true,
  missing: [],
};
const baseMcp = (setup) => ({
  plugin_id: "hermes-agent",
  plugin_name: "hermes-agent",
  candidate_id: "mcp",
  kind: "mcp_stdio",
  activation: "mcp_register",
  mcp_server: { id: "gh", transport: "managed_stdio", endpoint: "", description: "", enabled: true, timeout_ms: 5000, status: "configured", tool_overrides: {} },
  mcp_discovery: {
    reachable: false, tool_count: 0, gated_count: 0,
    guidance: "Registered MCP server \\"gh\\", but a tools/list probe couldn't reach it yet — it expects secrets (OPENAI_API_KEY).",
    tools: [], error: "missing secret for env var 'OPENAI_API_KEY'",
  },
  tool_name: "gh",
  next_step: "Registered MCP server \\"gh\\".",
  no_code_executed: true,
  approval_closed: true,
  setup,
});
const COMMAND_TOOL = {
  plugin_id: "plain", plugin_name: "plain", candidate_id: "command", kind: "cli_command",
  activation: "command_tool", tool_name: "plain.run",
  next_step: "Configured command tool \\"plain.run\\".", no_code_executed: true, approval_closed: false,
};
export function renderNeedsSetup() { return at(<CandidateActivationResult result={baseMcp(SETUP_NEEDS_WORK)} />); }
export function renderReadySetup() { return at(<CandidateActivationResult result={baseMcp(SETUP_READY)} />); }
export function renderCommandTool() { return at(<CandidateActivationResult result={COMMAND_TOOL} />); }
export function renderFormDirect() { return at(<McpEnvSetupForm serverId="gh" setup={SETUP_NEEDS_WORK} />); }
`;

let tmp = null;
let mod = null;

before(async () => {
  const built = await esbuild.build({
    stdin: {
      contents: RENDER_ENTRY,
      resolveDir: pagesDir,
      loader: "tsx",
      sourcefile: "mcp-env-setup-render-entry.tsx",
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    write: false,
    logLevel: "silent",
  });
  tmp = mkdtempSync(join(tmpdir(), "relux-mcp-env-setup-render-"));
  const out = join(tmp, "mcp-env-setup-render-entry.cjs");
  writeFileSync(out, built.outputFiles[0].text);
  mod = await import(pathToFileURL(out).href);
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("a setup that needs work renders an input per required var + the save action", () => {
  const html = mod.renderNeedsSetup();
  // The required env var name is shown.
  assert.match(html, /OPENAI_API_KEY/);
  // The honest "needs a secret" status chip + a "set up the secrets" header.
  assert.match(html, /needs a secret/i);
  assert.match(html, /Set up the secrets this server needs/i);
  // A value input (password type, never echoed) and the "use existing secret" choice.
  assert.match(html, /type="password"/);
  assert.match(html, /Use an existing secret/);
  // The save-and-discover action renders.
  assert.match(html, /Save secrets &amp; discover/);
});

test("the form never prints a secret value — only names/status", () => {
  const html = mod.renderFormDirect();
  // The mapped secret name is fine to surface; a value-shaped string must not appear (we
  // never feed one). The password field starts empty.
  assert.match(html, /OPENAI_API_KEY/);
  assert.doesNotMatch(html, /sk-/);
  assert.match(html, /value=""/);
});

test("a READY setup renders no setup form (nothing outstanding)", () => {
  const html = mod.renderReadySetup();
  assert.doesNotMatch(html, /Set up the secrets this server needs/i);
  assert.doesNotMatch(html, /Save secrets &amp; discover/);
});

test("a command-tool activation renders no setup form", () => {
  const html = mod.renderCommandTool();
  assert.match(html, /plain\.run/);
  assert.doesNotMatch(html, /Set up the secrets this server needs/i);
});
