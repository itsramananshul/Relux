import { test } from "node:test";
import assert from "node:assert/strict";
import {
  pluginCategory,
  pluginKindLabel,
  pluginStatus,
  pluginNextStep,
  canConfigureTools,
  guidedConfigSteps,
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
  normalizeGithubUrl,
  candidateKindLabel,
  candidateConfidenceBadge,
  isOneClickCandidate,
  isCommandToolCandidate,
  mcpDraftFromCandidate,
  commandToolDraftFromCandidate,
  commandToolInputFromDraft,
  validateCommandToolDraft,
  parseCommandArgs,
  capabilitySummary,
  primeUseCue,
  primeSourceCapabilities,
  exposesPrimeSourceCapabilities,
  buildPluginSummarySeed,
  PRIME_SOURCE_CAPABILITIES,
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

// Plugin Lens: every non-bundled installed plugin exposes the four read-only source
// capabilities; a bundled fixture exposes none (its capabilities are already known).
test("plugin lens exposes the four read-only source capabilities for a non-bundled plugin", () => {
  const caps = primeSourceCapabilities(plugin({ generated: true, tool_count: 0 }));
  assert.equal(caps.length, 4);
  const names = caps.map((c) => c.tool);
  assert.deepEqual(names, [
    "plugin.summary",
    "plugin.inspect",
    "plugin.search",
    "plugin.read_file",
  ]);
  // A manifest plugin gets them too (in addition to its declared tools).
  assert.equal(primeSourceCapabilities(plugin({ tool_count: 3 })).length, 4);
  assert.equal(PRIME_SOURCE_CAPABILITIES.length, 4);
});

test("plugin lens is hidden for bundled/protected fixtures", () => {
  assert.equal(exposesPrimeSourceCapabilities(plugin({ bundled: true })), false);
  assert.equal(exposesPrimeSourceCapabilities(plugin({ protected: true })), false);
  assert.equal(primeSourceCapabilities(plugin({ bundled: true })).length, 0);
  assert.equal(exposesPrimeSourceCapabilities(plugin()), true);
});

test("plugin summary seed names the plugin id and is read-only", () => {
  const seed = buildPluginSummarySeed(plugin({ id: "acme-repo" }));
  assert.match(seed, /acme-repo/);
  assert.match(seed, /read-only/i);
});

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

// The GitHub install field is forgiving: `owner/repo` shorthand expands to the
// canonical https URL, while a full URL (or anything with a scheme) is passed
// through untouched for the kernel's authoritative validator. The expansion is
// conservative — it never injects credentials, never rewrites a scheme, and never
// turns junk into an accepted URL.
test("normalizeGithubUrl expands owner/repo shorthand to the canonical https URL", () => {
  assert.equal(
    normalizeGithubUrl("nousresearch/hermes-agent"),
    "https://github.com/nousresearch/hermes-agent",
  );
  // Surrounding whitespace is trimmed; a trailing slash / `.git` is dropped.
  assert.equal(normalizeGithubUrl("  owner/repo  "), "https://github.com/owner/repo");
  assert.equal(normalizeGithubUrl("owner/repo.git"), "https://github.com/owner/repo");
  assert.equal(normalizeGithubUrl("owner/repo/"), "https://github.com/owner/repo");
});

test("normalizeGithubUrl leaves a full URL (or any scheme) untouched", () => {
  assert.equal(
    normalizeGithubUrl("https://github.com/owner/repo"),
    "https://github.com/owner/repo",
  );
  assert.equal(
    normalizeGithubUrl("  https://github.com/owner/repo  "),
    "https://github.com/owner/repo",
  );
  // A non-github scheme is NOT rewritten — the server validator rejects it.
  assert.equal(normalizeGithubUrl("git://example.com/x"), "git://example.com/x");
  assert.equal(
    normalizeGithubUrl("ssh://git@github.com/owner/repo"),
    "ssh://git@github.com/owner/repo",
  );
});

test("normalizeGithubUrl never fabricates a URL from unsafe / non-shorthand input", () => {
  // A bare word, too many segments, or embedded credentials are passed through
  // unchanged so the kernel's validate_github_url stays the real gate.
  assert.equal(normalizeGithubUrl("owner"), "owner");
  assert.equal(normalizeGithubUrl("a/b/c"), "a/b/c");
  assert.equal(normalizeGithubUrl("owner repo"), "owner repo");
  assert.equal(normalizeGithubUrl("user:tok@owner/repo"), "user:tok@owner/repo");
  assert.equal(normalizeGithubUrl(""), "");
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

// ── Guided configuration checklist (RELUX_MASTER_PLAN "Tool Invocation Workflow")
// The documented sequence for a metadata-only wrapper — review → define a tool (or
// register an MCP server) → enable a loopback runtime → use it — surfaced as honest
// step statuses. These pin: nothing reads "done" before the backend supports it,
// the runtime step is upcoming until a tool exists (a runtime-first move surfaces
// nothing), and a non-configurable plugin's define step is honestly "blocked".

function runtime(over = {}) {
  return { plugin_id: "relux-tools-demo", configured: false, enabled: false, ...over };
}
function hints(over = {}) {
  return { plugin_id: "relux-tools-demo", install_dir: "/d", scanned: true, generated: true, hints: [], ...over };
}

test("guidedConfigSteps: a fresh scanned wrapper points at 'add a tool', NOT a runtime", () => {
  const g = guidedConfigSteps(plugin({ generated: true, tool_count: 0 }), [], runtime(), hints());
  assert.equal(g.total, 4);
  assert.equal(g.complete, false);
  const byKey = Object.fromEntries(g.steps.map((s) => [s.key, s]));
  // Review is done once scanned; the define step is the one to act on now.
  assert.equal(byKey.review.status, "done");
  assert.equal(byKey.define.status, "current");
  assert.ok(byKey.define.actionable, "define is actionable on a configurable wrapper");
  // Runtime is NOT yet actionable — a runtime with no tools surfaces nothing.
  assert.equal(byKey.runtime.status, "upcoming");
  assert.equal(byKey.runtime.actionable, false);
  assert.match(byKey.runtime.missing, /tool definition first/i);
  // Use is upcoming and honestly states no tool is defined yet.
  assert.equal(byKey.use.status, "upcoming");
  assert.match(byKey.use.missing, /No tool is defined/i);
});

test("guidedConfigSteps: a tool defined but no runtime makes runtime the current step", () => {
  const t = tool({ executable: "runtime_not_configured" });
  const g = guidedConfigSteps(plugin({ generated: true, tool_count: 1 }), [t], runtime(), hints());
  const byKey = Object.fromEntries(g.steps.map((s) => [s.key, s]));
  assert.equal(byKey.define.status, "done");
  assert.equal(byKey.runtime.status, "current");
  assert.ok(byKey.runtime.actionable);
  // Not ready yet → use is upcoming, citing the missing runtime.
  assert.equal(byKey.use.status, "upcoming");
  assert.match(byKey.use.missing, /enable a loopback runtime/i);
  assert.equal(g.complete, false);
});

test("guidedConfigSteps: a ready tool + enabled runtime completes the checklist", () => {
  const t = tool({ executable: "ready" });
  const g = guidedConfigSteps(
    plugin({ generated: true, tool_count: 1 }),
    [t],
    runtime({ configured: true, enabled: true }),
    hints(),
  );
  const byKey = Object.fromEntries(g.steps.map((s) => [s.key, s]));
  assert.equal(byKey.runtime.status, "done");
  assert.equal(byKey.use.status, "done");
  assert.equal(g.complete, true);
  assert.equal(g.doneCount, 4);
});

test("guidedConfigSteps: a non-configurable plugin's define step is honestly blocked", () => {
  // A bundled/protected plugin can't be configured in-UI — say so, don't pretend.
  const g = guidedConfigSteps(plugin({ generated: false, bundled: true, tool_count: 0 }), [], runtime(), hints());
  const byKey = Object.fromEntries(g.steps.map((s) => [s.key, s]));
  assert.equal(byKey.define.status, "blocked");
  assert.equal(byKey.define.actionable, false);
  assert.match(byKey.define.missing, /can't be configured in-UI|bundled|ToolSet/i);
});

test("guidedConfigSteps: an MCP-proposal source offers register-MCP as the define step", () => {
  const g = guidedConfigSteps(
    plugin({ generated: true, tool_count: 0 }),
    [],
    runtime(),
    hints({ mcp_proposal: { suggested_id: "x", suggested_description: "", endpoint_required: true, suggested_transport: "http_loopback", notes: [] } }),
  );
  const define = g.steps.find((s) => s.key === "define");
  assert.match(define.title, /MCP server/i);
  assert.match(define.detail, /Discover/i);
});

test("guidedConfigSteps: unloaded runtime/hints degrade honestly (no fake progress)", () => {
  // Before the runtime/hints fetch resolves, undefined must not read as configured.
  const t = tool({ executable: "runtime_not_configured" });
  const g = guidedConfigSteps(plugin({ generated: true, tool_count: 1 }), [t], undefined, undefined);
  const byKey = Object.fromEntries(g.steps.map((s) => [s.key, s]));
  // Review is still done because a tool already exists (it was reviewed to define it).
  assert.equal(byKey.review.status, "done");
  // Runtime unknown → treated as not-ready, the current actionable step.
  assert.equal(byKey.runtime.status, "current");
  assert.equal(g.complete, false);
});

// ── Detected capability candidates (install-to-usable configuration) ──────────
// The structured layer the kernel returns alongside hints. The honesty rule the
// page depends on: an mcp_register candidate is one-click + carries a draft; a
// manual candidate is an honest pending capability, never faked ready.

function mcpCandidate(over = {}) {
  return {
    id: "mcp-server",
    kind: "mcp_stdio",
    title: "MCP server (stdio)",
    confidence: "high",
    risk: "medium",
    rationale: "depends on @modelcontextprotocol/sdk",
    command_preview: "node ./server.js",
    env_placeholders: [],
    activation: "mcp_register",
    mcp_registration: {
      suggested_id: "cool-mcp",
      suggested_description: "",
      endpoint_required: true,
      suggested_transport: "managed_stdio",
      detected_command: "node",
      detected_args: ["./server.js"],
      notes: [],
    },
    next_steps: ["Open the review form."],
    ...over,
  };
}
function cliCandidate(over = {}) {
  return {
    id: "cli-bin-tool",
    kind: "cli_command",
    title: "Command-line tool (npm bin)",
    confidence: "medium",
    risk: "medium",
    rationale: "package.json declares a bin entrypoint",
    command_preview: "node ./cli.js",
    env_placeholders: [],
    activation: "manual",
    next_steps: ["Run it yourself as a loopback server, then add a tool."],
    ...over,
  };
}

test("candidateKindLabel maps each kind to a friendly label", () => {
  assert.equal(candidateKindLabel("mcp_stdio"), "MCP server (stdio)");
  assert.equal(candidateKindLabel("mcp_http"), "MCP server (loopback HTTP)");
  assert.equal(candidateKindLabel("cli_command"), "Command-line tool");
  assert.equal(candidateKindLabel("future"), "future");
});

test("candidateConfidenceBadge: high reads ok, lower reads muted (never inflated)", () => {
  assert.deepEqual(candidateConfidenceBadge("high"), { label: "high confidence", variant: "ok" });
  assert.equal(candidateConfidenceBadge("medium").variant, "muted");
  assert.equal(candidateConfidenceBadge("low").variant, "muted");
});

test("isOneClickCandidate: true only for an mcp_register candidate that carries a draft", () => {
  assert.equal(isOneClickCandidate(mcpCandidate()), true);
  assert.equal(isOneClickCandidate(cliCandidate()), false);
  // An mcp_register activation with no draft is NOT treated as one-click (defensive).
  assert.equal(isOneClickCandidate(mcpCandidate({ mcp_registration: undefined })), false);
});

test("mcpDraftFromCandidate seeds the SAME draft the existing registry form uses", () => {
  const d = mcpDraftFromCandidate(mcpCandidate());
  assert.equal(d.transport, "managed_stdio");
  assert.equal(d.command, "node");
  assert.equal(d.argsText, "./server.js");
  assert.equal(d.id, "cool-mcp");
  // A malformed candidate falls back to a usable empty draft, never throws.
  const empty = mcpDraftFromCandidate(cliCandidate());
  assert.equal(empty.transport, "http_loopback");
});

test("capabilitySummary counts one-click vs manual and flags a true empty-after-scan", () => {
  const s = capabilitySummary(hints({ candidates: [mcpCandidate(), cliCandidate()] }));
  assert.equal(s.total, 2);
  assert.equal(s.oneClick, 1);
  assert.equal(s.manual, 1);
  assert.equal(s.emptyAfterScan, false);

  // Scanned, no candidates → empty-after-scan true (the page shows what-to-add guidance).
  const empty = capabilitySummary(hints({ candidates: [] }));
  assert.equal(empty.total, 0);
  assert.equal(empty.emptyAfterScan, true);

  // Not yet loaded (undefined) or not scanned → never claims "empty" (no false dead-end).
  assert.equal(capabilitySummary(undefined).emptyAfterScan, false);
  assert.equal(capabilitySummary(hints({ scanned: false, candidates: [] })).emptyAfterScan, false);
});

// A cli_command candidate that carries a pre-filled argv draft (the governed
// command-tool activation path).
function commandToolCandidate(over = {}) {
  return {
    id: "cli-bin-tool",
    kind: "cli_command",
    title: "Command-line tool (npm bin)",
    confidence: "medium",
    risk: "medium",
    rationale: "package.json declares a bin entrypoint",
    command_preview: "node ./cli.js",
    env_placeholders: [],
    activation: "command_tool",
    command_tool: {
      tool_name: "tool.run",
      program: "node",
      args: ["./cli.js"],
      description: "Command-line tool (npm bin)",
    },
    next_steps: ["Click Configure to open the form."],
    ...over,
  };
}

test("isCommandToolCandidate: true only for a command_tool candidate that carries a draft", () => {
  assert.equal(isCommandToolCandidate(commandToolCandidate()), true);
  // A plain manual cli candidate (no draft) is NOT a command-tool activation.
  assert.equal(isCommandToolCandidate(cliCandidate()), false);
  assert.equal(isCommandToolCandidate(commandToolCandidate({ command_tool: undefined })), false);
  assert.equal(isCommandToolCandidate(mcpCandidate()), false);
});

test("commandToolDraftFromCandidate seeds the form from the pre-filled argv draft", () => {
  const d = commandToolDraftFromCandidate(commandToolCandidate());
  assert.equal(d.name, "tool.run");
  assert.equal(d.program, "node");
  assert.equal(d.argsText, "./cli.js");
  assert.equal(d.risk, "high");
  // A malformed candidate falls back to a usable empty draft, never throws.
  const empty = commandToolDraftFromCandidate(cliCandidate());
  assert.equal(empty.program, "");
});

test("commandToolInputFromDraft builds the POST body (args split, optional fields omitted)", () => {
  const body = commandToolInputFromDraft({
    name: "repo.build",
    description: "",
    program: "cargo",
    argsText: "build\n--release",
    cwd: "",
    timeoutSecs: "120",
    risk: "high",
  });
  assert.equal(body.name, "repo.build");
  assert.equal(body.program, "cargo");
  assert.deepEqual(body.args, ["build", "--release"]);
  assert.equal(body.cwd, undefined);
  assert.equal(body.description, undefined);
  assert.equal(body.timeout_secs, 120);
  assert.equal(body.risk, "high");
});

test("parseCommandArgs splits lines, trims, drops blanks (an arg may keep internal spaces)", () => {
  assert.deepEqual(parseCommandArgs("  a \n\n b c \n"), ["a", "b c"]);
  assert.deepEqual(parseCommandArgs(""), []);
});

test("validateCommandToolDraft mirrors the kernel's fail-closed argv contract", () => {
  const ok = {
    name: "repo.run",
    description: "",
    program: "node",
    argsText: "./cli.js",
    cwd: "crates",
    timeoutSecs: "30",
    risk: "high",
  };
  assert.equal(validateCommandToolDraft(ok), null);
  // A shell-string program is rejected (no shell — argv only).
  assert.match(validateCommandToolDraft({ ...ok, program: "rm -rf / && curl" }) ?? "", /shell/i);
  // A danger flag arg is rejected.
  assert.match(
    validateCommandToolDraft({ ...ok, argsText: "--dangerously-skip-permissions" }) ?? "",
    /danger/i,
  );
  // A '..' traversal cwd is rejected.
  assert.match(validateCommandToolDraft({ ...ok, cwd: "../etc" }) ?? "", /traversal/i);
  // A missing program / name is rejected.
  assert.match(validateCommandToolDraft({ ...ok, program: "" }) ?? "", /program/i);
  assert.match(validateCommandToolDraft({ ...ok, name: "" }) ?? "", /name/i);
});

test("capabilitySummary counts command-tool candidates separately from manual/one-click", () => {
  const s = capabilitySummary(
    hints({ candidates: [mcpCandidate(), commandToolCandidate(), cliCandidate()] }),
  );
  assert.equal(s.total, 3);
  assert.equal(s.oneClick, 1);
  assert.equal(s.commandTool, 1);
  assert.equal(s.manual, 1);
});

// docs/prime-tool-use.md "The verified install → use path" §4/§5 + "Tools Prime can
// use": after configuring a runnable tool, the page owes the operator a clear "Prime
// can use this now" cue with the EXACT chat phrase to try — and an honest gated note
// (the first call pauses for approval; it never claims auto-run).
test("primeUseCue gives the natural chat phrase for a command tool + an honest gated note", () => {
  const cue = primeUseCue("repo.build", "command_tool");
  assert.match(cue.headline, /Prime can use this now/i);
  assert.match(cue.phrase, /run the repo\.build tool/);
  // Honest: gated, pauses for approval, nothing runs until approved — never "auto".
  assert.match(cue.detail, /approval/i);
  assert.match(cue.detail, /Nothing runs until you approve/i);
  assert.doesNotMatch(cue.detail, /auto-?run/i);
});

test("primeUseCue's MCP variant points at discovery and stays honest about gating", () => {
  const cue = primeUseCue("cool-mcp", "mcp_server");
  assert.match(cue.phrase, /use the cool-mcp tools/);
  assert.match(cue.detail, /[Dd]iscover/);
  assert.match(cue.detail, /gated/i);
});

test("primeUseCue defaults to the command-tool phrasing and tolerates a blank name", () => {
  assert.match(primeUseCue("x").phrase, /run the x tool/);
  assert.match(primeUseCue("  ").phrase, /run the the tool tool/);
});
