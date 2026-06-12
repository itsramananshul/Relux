import { test } from "node:test";
import assert from "node:assert/strict";
import {
  pluginCategory,
  pluginKindLabel,
  pluginStatus,
  pluginNextStep,
  canConfigureTools,
  installResultSummary,
  visibleTools,
  isRunnableTool,
  toolReadiness,
  adapterStatusBadge,
  ADAPTER_STATE_LABEL,
  mcpServerStatusBadge,
  managedStdioStatusBadge,
  hintKindLabel,
  hintsNextStep,
  mcpDraftFromProposal,
  validateMcpRegisterDraft,
  mcpRegisterBody,
  parseEnvMappingLines,
  mcpEnvFromText,
} from "../src/plugins.ts";

// The Plugins page must read HONESTLY: a generated metadata-only wrapper is never
// shown as ready/enabled, and its next step is "add a manifest" (it has no tools),
// not "configure a runtime". A real ToolSet keeps the runtime call-to-action. The
// Tools list shows only runnable tools by default. These assertions pin that so a
// regression (a wrapper badged "enabled", a misleading runtime CTA) fails loudly.

// Minimal builders shaped like the real API types (runtime-only; not type-checked).
function plugin(over = {}) {
  return {
    id: "relux-tools-demo",
    name: "Demo",
    description: "",
    kind: "ToolSet",
    version: "0.1.0",
    enabled: true,
    source_kind: "Github",
    source_label: "https://github.com/owner/demo",
    install_dir: "/data/demo",
    protected: false,
    bundled: false,
    generated: false,
    tool_count: 1,
    ...over,
  };
}

function tool(over = {}) {
  return {
    plugin_id: "relux-tools-demo",
    tool_name: "demo.run",
    description: "",
    permission: "tool:relux-tools-demo:run",
    risk: "low",
    source_kind: "github",
    installed: true,
    enabled: true,
    protected: false,
    executable: "ready",
    ...over,
  };
}

function mcpServer(over = {}) {
  return {
    id: "fs-helper",
    transport: "http_loopback",
    endpoint: "http://127.0.0.1:8000/mcp",
    description: "local fs",
    enabled: true,
    timeout_ms: 10000,
    status: "configured",
    ...over,
  };
}

test("an MCP server badge reflects only its config (configured vs disabled), never faked ready", () => {
  const on = mcpServerStatusBadge(mcpServer({ enabled: true }));
  assert.equal(on.label, "configured");
  assert.equal(on.variant, "ok");
  const off = mcpServerStatusBadge(mcpServer({ enabled: false }));
  assert.equal(off.label, "disabled");
  assert.equal(off.variant, "muted");
});

test("a managed-stdio process badge reflects the LIVE process state, never faked", () => {
  // Running shows the pid and reads as ok; failed surfaces the redacted reason as a
  // warn (never ok); stopped/starting are muted.
  const running = managedStdioStatusBadge({ id: "gh", state: "running", pid: 4242 });
  assert.equal(running.variant, "ok");
  assert.ok(running.label.includes("4242"), `pid shown: ${running.label}`);

  const failed = managedStdioStatusBadge({
    id: "gh",
    state: "failed",
    last_error: "spawn npx: not found",
  });
  assert.equal(failed.label, "failed");
  assert.equal(failed.variant, "warn");
  assert.ok(failed.title.includes("not found"), "failure reason surfaced");

  assert.equal(managedStdioStatusBadge({ id: "gh", state: "stopped" }).label, "stopped");
  assert.equal(managedStdioStatusBadge({ id: "gh", state: "stopped" }).variant, "muted");
  assert.equal(managedStdioStatusBadge({ id: "gh", state: "starting" }).label, "starting");
});

// A discovered MCP tool flows through the SAME `toolReadiness` classifier a plugin
// tool uses, keyed off the kernel's `executable`. An unclassified MCP tool is
// `needs_approval` (gated, but can request a per-call approval); a classified
// low-risk + auto-approve tool is `ready` (directly invocable). This pins the
// invoke-surface integration so a regression (a gated MCP tool shown runnable, or
// a ready one shown as "not callable") fails loudly.
function mcpTool(over = {}) {
  return tool({
    plugin_id: "mcp:fs-helper",
    tool_name: "search",
    permission: "tool:mcp-fs-helper:search",
    source_kind: "Mcp",
    risk: "medium",
    executable: "needs_approval",
    ...over,
  });
}

test("an unclassified MCP tool is gated (needs approval), never directly runnable", () => {
  const r = toolReadiness(mcpTool());
  assert.equal(r.runnable, false);
  assert.equal(r.canRequestApproval, true);
  assert.equal(r.label, "needs approval");
  assert.equal(isRunnableTool(mcpTool()), false);
});

test("a classified low-risk auto-approve MCP tool is ready to invoke directly", () => {
  const r = toolReadiness(mcpTool({ executable: "ready", risk: "low" }));
  assert.equal(r.runnable, true);
  assert.equal(r.canRequestApproval, false);
  assert.equal(r.label, "ready");
  assert.equal(isRunnableTool(mcpTool({ executable: "ready" })), true);
});

test("a generated wrapper is categorized as a wrapper regardless of kind", () => {
  assert.equal(pluginCategory(plugin({ generated: true })), "wrapper");
  assert.equal(pluginCategory(plugin({ generated: true, kind: "ToolSet" })), "wrapper");
  assert.equal(pluginKindLabel(plugin({ generated: true })), "Metadata-only wrapper");
});

test("real adapters and toolsets keep their honest kind labels", () => {
  assert.equal(pluginCategory(plugin({ kind: "Adapter" })), "adapter");
  assert.equal(pluginKindLabel(plugin({ kind: "Adapter" })), "Adapter");
  assert.equal(pluginCategory(plugin({ kind: "ToolSet" })), "toolset");
  assert.equal(pluginKindLabel(plugin({ kind: "ToolSet" })), "ToolSet");
});

test("a metadata-only wrapper with NO tools is NEVER shown as ready/enabled", () => {
  const s = pluginStatus(plugin({ generated: true, enabled: true, tool_count: 0 }));
  assert.equal(s.label, "Needs configuration");
  assert.equal(s.variant, "warn");
  assert.notEqual(s.variant, "ok");
});

test("a wrapper the operator has added tools to behaves like a toolset", () => {
  // Once tools exist (tool_count > 0), the wrapper is no longer a dead-end: it
  // shows the plain enabled/disabled status like any ToolSet.
  const s = pluginStatus(plugin({ generated: true, enabled: true, tool_count: 1 }));
  assert.equal(s.label, "enabled");
  assert.equal(s.variant, "ok");
});

test("a real enabled toolset shows enabled; a disabled one shows disabled", () => {
  assert.equal(pluginStatus(plugin({ enabled: true })).label, "enabled");
  assert.equal(pluginStatus(plugin({ enabled: true })).variant, "ok");
  assert.equal(pluginStatus(plugin({ enabled: false })).label, "disabled");
  assert.equal(pluginStatus(plugin({ enabled: false })).variant, "muted");
});

test("a 0-tool wrapper's next step is add-manifest, and it explains the dead-end", () => {
  const step = pluginNextStep(plugin({ generated: true, tool_count: 0 }));
  assert.equal(step.kind, "add-manifest");
  assert.equal(step.cta, "Configure");
  assert.match(step.detail, /declares no tools/i);
  assert.match(step.detail, /tool definition/i);
});

test("a wrapper with tools added now points at the loopback runtime step", () => {
  const step = pluginNextStep(plugin({ generated: true, tool_count: 2 }));
  assert.equal(step.kind, "configure-runtime");
  assert.equal(step.cta, "Runtime");
  assert.match(step.detail, /2 tools/);
});

test("a real non-bundled toolset keeps the runtime call-to-action", () => {
  const step = pluginNextStep(plugin({ kind: "ToolSet", tool_count: 3 }));
  assert.equal(step.kind, "configure-runtime");
  assert.equal(step.cta, "Runtime");
  assert.match(step.detail, /3 tools/);
});

test("canConfigureTools: wrappers + non-bundled toolsets yes; bundled + adapters no", () => {
  assert.equal(canConfigureTools(plugin({ generated: true, tool_count: 0 })), true);
  assert.equal(canConfigureTools(plugin({ kind: "ToolSet" })), true);
  // Bundled/protected fixtures and adapters are refused (kernel rejects them too).
  assert.equal(canConfigureTools(plugin({ protected: true, bundled: true })), false);
  assert.equal(canConfigureTools(plugin({ kind: "Adapter" })), false);
});

test("a bundled toolset has no next step (built-in, runnable)", () => {
  const step = pluginNextStep(plugin({ protected: true, bundled: true }));
  assert.equal(step.kind, "none");
});

test("a non-bundled adapter points to the Crew page, not loopback runtime", () => {
  const step = pluginNextStep(plugin({ kind: "Adapter", protected: false }));
  assert.equal(step.kind, "configure-adapter");
  assert.match(step.detail, /Crew/);
});

test("a bundled (protected) adapter is configurable, NOT a dead-end locked row", () => {
  // The shipped Claude/Codex CLIs are protected (can't be removed) but must still
  // expose a real Configure path — the mission's "mysterious protected rows with no
  // path to use them" is the exact regression this pins.
  const step = pluginNextStep(plugin({ kind: "Adapter", protected: true }));
  assert.equal(step.kind, "configure-adapter");
  assert.notEqual(step.kind, "none");
  assert.match(step.detail, /Crew/);
  assert.match(step.detail, /Prime's brain|Settings/);
});

test("tools list shows only runnable tools by default, with a hidden count", () => {
  const tools = [
    tool({ tool_name: "a", executable: "ready" }),
    tool({ tool_name: "b", executable: "runtime_not_configured" }),
    tool({ tool_name: "c", executable: "not_implemented" }),
  ];
  const def = visibleTools(tools, false);
  assert.equal(def.shown.length, 1);
  assert.equal(def.shown[0].tool_name, "a");
  assert.equal(def.hiddenCount, 2);

  const all = visibleTools(tools, true);
  assert.equal(all.shown.length, 3);
  assert.equal(all.hiddenCount, 0);
});

test("install summary for a wrapper is honest: imported as metadata-only, no manifest needed", () => {
  const s = installResultSummary(plugin({ generated: true, tool_count: 0 }));
  assert.equal(s.tone, "info");
  // The headline must read as a SUCCESSFUL import that needed no manifest — never
  // as a failure or a "manifest required" message (the exact UX the mission fixes).
  assert.match(s.headline, /imported/i);
  assert.match(s.headline, /metadata-only/i);
  assert.match(s.headline, /no Relux manifest needed/i);
  // The detail says the manifest is optional and names the real next actions.
  assert.match(s.detail, /optional/i);
  assert.match(s.detail, /no tools|nothing runs/i);
  assert.match(s.detail, /tool definition|MCP server|hints/i);
});

test("install summary for a real toolset reports discovered tool count + runtime step", () => {
  const s = installResultSummary(plugin({ kind: "ToolSet", tool_count: 2 }));
  assert.equal(s.tone, "ok");
  assert.match(s.headline, /discovered 2 tools/);
  assert.match(s.detail, /loopback/i);
});

test("install summary for an adapter points at the Crew page", () => {
  const s = installResultSummary(plugin({ kind: "Adapter", tool_count: 0 }));
  assert.equal(s.tone, "ok");
  assert.match(s.headline, /adapter/i);
  assert.match(s.detail, /Crew/);
});

test("isRunnableTool is true only for ready tools", () => {
  assert.equal(isRunnableTool(tool({ executable: "ready" })), true);
  assert.equal(isRunnableTool(tool({ executable: "runtime_disabled" })), false);
  assert.equal(isRunnableTool(tool({ executable: "missing_permission" })), false);
});

// ── Tool readiness (the honest invocation surface) ────────────────────────────
// The single classifier that drives the Tools list: every non-ready state is a
// CLEAR refusal/disabled reason (never a blank "not callable"), and ONLY "ready"
// is runnable — matching what the kernel enforces in call_tool/invoke_tool.

test("a ready tool is runnable, ok-toned, and offers no next step", () => {
  const r = toolReadiness(tool({ executable: "ready" }));
  assert.equal(r.runnable, true);
  assert.equal(r.tone, "ok");
  assert.equal(r.label, "ready");
  assert.equal(r.nextStep, undefined);
});

test("a needs_approval tool is NOT runnable but CAN request a per-call approval", () => {
  // The exact regression the mission pins: a higher-risk configured tool must be
  // refused on the direct invoke path and SAY so — never blank, never pretend-run.
  // It is NOT directly runnable, but the per-call approval flow IS now available.
  const r = toolReadiness(tool({ executable: "needs_approval", risk: "high" }));
  assert.equal(r.runnable, false);
  assert.equal(r.canRequestApproval, true);
  assert.equal(r.label, "needs approval");
  assert.match(r.reason, /requires approval|refused/i);
  assert.match(r.reason, /high-risk/);
  // The next step points at the per-call approval flow (and still notes the
  // lower-risk alternative).
  assert.match(r.nextStep ?? "", /request a per-call approval/i);
  assert.match(r.nextStep ?? "", /lower its risk/i);
});

test("only needs_approval allows requesting a per-call approval", () => {
  // Every other state — runnable or not — must NOT offer the request-approval
  // form, so the gate is never bypassed and a ready tool just runs.
  for (const executable of [
    "ready",
    "runtime_not_configured",
    "runtime_disabled",
    "missing_permission",
    "not_implemented",
  ]) {
    const r = toolReadiness(tool({ executable }));
    assert.equal(r.canRequestApproval, false, `${executable} must not request approval`);
  }
});

test("a runtime_not_configured tool points at configuring a loopback runtime", () => {
  const r = toolReadiness(tool({ executable: "runtime_not_configured" }));
  assert.equal(r.runnable, false);
  assert.match(r.reason, /no runtime|never auto-runs/i);
  assert.match(r.nextStep ?? "", /loopback|runtime/i);
});

test("a runtime_disabled tool points at re-enabling the runtime", () => {
  const r = toolReadiness(tool({ executable: "runtime_disabled" }));
  assert.equal(r.runnable, false);
  assert.match(r.reason, /disabled/i);
  assert.match(r.nextStep ?? "", /re-enable/i);
});

test("a missing_permission tool names the missing permission and the fix", () => {
  const r = toolReadiness(
    tool({ executable: "missing_permission", permission: "tool:relux-tools-demo:run" }),
  );
  assert.equal(r.runnable, false);
  assert.match(r.reason, /tool:relux-tools-demo:run/);
  assert.match(r.nextStep ?? "", /grant/i);
});

test("a not_implemented tool is honestly muted with no next step", () => {
  const r = toolReadiness(tool({ executable: "not_implemented" }));
  assert.equal(r.runnable, false);
  assert.equal(r.tone, "muted");
  assert.match(r.reason, /no supported runtime/i);
  assert.equal(r.nextStep, undefined);
});

test("every non-ready readiness state carries a real reason (no blank refusal)", () => {
  for (const executable of [
    "needs_approval",
    "runtime_not_configured",
    "runtime_disabled",
    "missing_permission",
    "not_implemented",
  ]) {
    const r = toolReadiness(tool({ executable }));
    assert.equal(r.runnable, false, `${executable} must not be runnable`);
    assert.ok(r.reason.length > 0, `${executable} must explain itself`);
    assert.ok(r.label.length > 0, `${executable} must have a badge label`);
  }
});

// ── Live adapter runtime badge (Plugins page) ─────────────────────────────────
// An adapter row must surface its LIVE runtime state, not the static plugin
// record. These pin the label vocabulary (shared with Crew) and the honest
// fail-closed behavior: an unresolved probe is NEVER shown as ready/available.

function adapterRuntime(over = {}) {
  return {
    plugin_id: "relux-adapter-claude-cli",
    adapter_name: "Claude CLI",
    kind: "claude",
    configured: true,
    enabled: true,
    command: "claude",
    available_on_path: true,
    resolved_path: "/usr/local/bin/claude",
    timeout_seconds: 600,
    max_output_bytes: 1048576,
    working_dir: null,
    state: "available",
    detail: "Enabled. Relux will run 'claude' for assigned tasks.",
    ...over,
  };
}

test("an enabled, on-PATH adapter reads as available (ok), with the live detail", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "available" }));
  assert.equal(b.label, "Enabled — ready");
  assert.equal(b.variant, "ok");
  assert.match(b.title, /Relux will run/);
});

test("the local deterministic adapter reads as available (ok)", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "local_deterministic" }));
  assert.equal(b.label, "Local (deterministic)");
  assert.equal(b.variant, "ok");
});

test("an enabled adapter whose binary is missing reads as warn, not ok", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "missing_binary" }));
  assert.equal(b.label, "Enabled — binary missing");
  assert.equal(b.variant, "warn");
  assert.notEqual(b.variant, "ok");
});

test("a default/unconfigured CLI adapter reads as needs-configuration (warn)", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "needs_configuration" }));
  assert.equal(b.label, "Disabled (default)");
  assert.equal(b.variant, "warn");
});

test("a deliberately disabled adapter reads as muted, never ready", () => {
  const b = adapterStatusBadge(adapterRuntime({ state: "disabled" }));
  assert.equal(b.label, "Configured — disabled");
  assert.equal(b.variant, "muted");
  assert.notEqual(b.variant, "ok");
});

test("an unresolved adapter probe is honest 'status unavailable', NOT ready", () => {
  // undefined = the /v1/relux/adapters probe failed or no row matched this plugin.
  const b = adapterStatusBadge(undefined);
  assert.equal(b.label, "status unavailable");
  assert.equal(b.variant, "muted");
  assert.notEqual(b.variant, "ok");
  assert.match(b.title, /could not read/i);
});

test("every adapter state has a shared label (single source of truth with Crew)", () => {
  for (const state of [
    "local_deterministic",
    "available",
    "missing_binary",
    "disabled",
    "needs_configuration",
  ]) {
    assert.ok(ADAPTER_STATE_LABEL[state], `missing label for ${state}`);
  }
});

// Source hints (read-only introspection of an imported repo) are advisory ONLY:
// they describe what was detected so the operator can wire the plugin up. They
// never claim anything is runnable — Relux never turns a hint into a tool and
// never runs downloaded code. These pin the honest copy.

test("hintKindLabel gives a friendly label and falls back to the raw kind", () => {
  assert.equal(hintKindLabel("mcp-server"), "Possible MCP server");
  assert.equal(hintKindLabel("npm-package"), "npm package");
  assert.equal(hintKindLabel("python-entrypoint"), "Python entrypoint");
  // An unknown kind is shown verbatim, never dropped or faked.
  assert.equal(hintKindLabel("something-new"), "something-new");
});

test("hintsNextStep routes an MCP signal to the MCP page and never claims it runs", () => {
  const step = hintsNextStep([
    { kind: "mcp-server", label: "Possible MCP server", detail: "depends on sdk" },
  ]);
  assert.ok(step);
  assert.match(step, /MCP/);
  assert.match(step, /won’t run it automatically|never run/i);
});

test("hintsNextStep for a plain package points at the in-UI tool path, not auto-run", () => {
  const step = hintsNextStep([
    { kind: "npm-package", label: "npm package", detail: "left-pad" },
  ]);
  assert.ok(step);
  assert.match(step, /never runs downloaded code/i);
});

test("hintsNextStep is null when there are no hints (nothing to suggest)", () => {
  assert.equal(hintsNextStep([]), null);
});

test("mcpDraftFromProposal seeds the review form from an HTTP proposal", () => {
  const draft = mcpDraftFromProposal({
    suggested_id: "acme-cool-mcp",
    suggested_description: "A cool MCP server.",
    suggested_endpoint: "http://127.0.0.1:8000/mcp",
    endpoint_required: false,
    suggested_transport: "http_loopback",
    notes: [],
  });
  assert.deepEqual(draft, {
    id: "acme-cool-mcp",
    transport: "http_loopback",
    endpoint: "http://127.0.0.1:8000/mcp",
    command: "",
    argsText: "",
    envText: "",
    cwd: "",
    description: "A cool MCP server.",
  });
});

test("mcpDraftFromProposal prefills a managed-stdio draft from a detected command", () => {
  // A detected command pre-fills a reviewable managed-stdio draft (advisory). The
  // HTTP endpoint stays blank — the command is never used as an endpoint.
  const draft = mcpDraftFromProposal({
    suggested_id: "gh",
    suggested_description: "Imported MCP server",
    endpoint_required: true,
    suggested_transport: "managed_stdio",
    detected_command: "npx",
    detected_args: ["-y", "@modelcontextprotocol/server-github"],
    notes: ["This server runs as a command (stdio). Relux can register it…"],
  });
  assert.equal(draft.transport, "managed_stdio");
  assert.equal(draft.command, "npx");
  assert.equal(draft.argsText, "-y\n@modelcontextprotocol/server-github");
  assert.equal(draft.endpoint, "", "a command is never used as the endpoint");
  assert.equal(draft.id, "gh");
});

test("validateMcpRegisterDraft mirrors the kernel's fail-closed rules (HTTP)", () => {
  const http = (over: Partial<Parameters<typeof validateMcpRegisterDraft>[0]>) =>
    validateMcpRegisterDraft({
      id: "acme-mcp",
      transport: "http_loopback",
      endpoint: "http://127.0.0.1:8000/mcp",
      command: "",
      argsText: "",
      envText: "",
      cwd: "",
      description: "",
      ...over,
    });
  // A clean draft passes.
  assert.equal(http({}), null);
  // An empty endpoint is required (the manual-entry case).
  assert.match(http({ endpoint: "  " }) ?? "", /endpoint is required/i);
  // An id with an illegal char (a space / path separator) is rejected early.
  assert.match(http({ id: "bad id" }) ?? "", /letters, digits/i);
  assert.match(http({ id: "a/b" }) ?? "", /letters, digits/i);
  // An empty id is rejected.
  assert.match(http({ id: "" }) ?? "", /id is required/i);
});

test("validateMcpRegisterDraft enforces the managed-stdio safety rules", () => {
  const stdio = (over: Partial<Parameters<typeof validateMcpRegisterDraft>[0]>) =>
    validateMcpRegisterDraft({
      id: "gh",
      transport: "managed_stdio",
      endpoint: "",
      command: "npx",
      argsText: "-y\nserver-github",
      envText: "",
      cwd: "",
      description: "",
      ...over,
    });
  // A clean stdio draft passes (no endpoint needed).
  assert.equal(stdio({}), null);
  // A missing command is rejected.
  assert.match(stdio({ command: "  " }) ?? "", /requires a command/i);
  // A shell-metacharacter command is rejected (argv-only).
  assert.match(stdio({ command: "sh;rm" }) ?? "", /shell metacharacters/i);
  // A forbidden bypass/danger flag in args is rejected.
  assert.match(
    stdio({ argsText: "--dangerously-skip-permissions" }) ?? "",
    /bypass\/danger flag/i,
  );
  // An arg with spaces (e.g. JSON) is fine — args are per-line, never shell-split.
  assert.equal(stdio({ argsText: '--config\n{"k": 1}' }), null);

  // --- Env mappings (secret REFERENCES) + cwd ---
  // A valid env line (ENV_VAR=secret_name) passes.
  assert.equal(stdio({ envText: "OPENAI_API_KEY=my_openai_key" }), null);
  // An invalid env-var name is rejected.
  assert.match(stdio({ envText: "2bad=secret" }) ?? "", /env var name/i);
  assert.match(stdio({ envText: "has-dash=secret" }) ?? "", /env var name/i);
  // A missing / invalid secret reference is rejected (must be a NAME, not a value).
  assert.match(stdio({ envText: "OK_VAR=" }) ?? "", /reference a secret/i);
  assert.match(stdio({ envText: "OK_VAR=not a name" }) ?? "", /reference a secret/i);
  // A `..` traversal cwd is rejected; a clean relative cwd passes.
  assert.match(stdio({ cwd: "../escape" }) ?? "", /traversal/i);
  assert.match(stdio({ cwd: "a\\..\\b" }) ?? "", /traversal/i);
  assert.equal(stdio({ cwd: "workspace-a" }), null);
});

test("mcpRegisterBody includes env refs + cwd for a managed-stdio draft", () => {
  const body = mcpRegisterBody({
    id: "gh",
    transport: "managed_stdio",
    endpoint: "",
    command: "npx",
    argsText: "-y\nserver-github",
    envText: "OPENAI_API_KEY=my_openai_key\nGH_TOKEN=gh_pat",
    cwd: "workspace-a",
    description: "",
  });
  assert.equal(body.transport, "managed_stdio");
  assert.deepEqual(body.env, {
    OPENAI_API_KEY: { secret: "my_openai_key" },
    GH_TOKEN: { secret: "gh_pat" },
  });
  assert.equal(body.cwd, "workspace-a");

  // No env / cwd → those fields are omitted (undefined), not empty objects.
  const bare = mcpRegisterBody({
    id: "gh",
    transport: "managed_stdio",
    endpoint: "",
    command: "npx",
    argsText: "",
    envText: "",
    cwd: "",
    description: "",
  });
  assert.equal(bare.env, undefined);
  assert.equal(bare.cwd, undefined);
});

test("parseEnvMappingLines + mcpEnvFromText parse VAR=secret lines (refs only)", () => {
  assert.deepEqual(parseEnvMappingLines("A=one\n\n  B = two  "), [
    ["A", "one"],
    ["B", "two"],
  ]);
  assert.deepEqual(mcpEnvFromText("OPENAI_API_KEY=my_key"), {
    OPENAI_API_KEY: { secret: "my_key" },
  });
});
