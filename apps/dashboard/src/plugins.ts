// Pure, dependency-free derivation of the Plugins page's honest status, kind, and
// next-step affordances (RELUX_MASTER_PLAN §8.2 ToolSet plugins, §11.6 Plugins
// page, "Status after v0.1.1" item 2: surface a clear call-to-action and never a
// "ready"-looking label for a metadata-only plugin).
//
// The single hard truth this encodes: a generated metadata-only wrapper declares
// NO tools, so pointing a loopback runtime at it surfaces nothing. Its next step
// is therefore "add tool definitions" (a relux-plugin.json), NOT "configure a
// runtime" — the runtime only matters once real tools exist. Relux never infers
// tools from downloaded content, and never auto-runs it (§18).
//
// Kept React-free (like ./routing and ./onboarding) so `node --test` can assert
// the guidance without a DOM. The page renders whatever this returns; it invents
// nothing.

import type {
  ReluxAdapterStatus,
  ReluxCapabilityCandidate,
  ReluxCommandToolInput,
  ReluxManagedStdioStatus,
  ReluxMcpRegistrationProposal,
  ReluxMcpServer,
  ReluxPlugin,
  ReluxPluginHint,
  ReluxPluginHints,
  ReluxPluginRuntime,
  ReluxToolDescriptor,
} from "./api";

// What the plugin actually is, for an honest one-word category in the UI.
//   "wrapper"  — generated metadata-only manifest (no tools, not runnable yet)
//   "adapter"  — an Adapter plugin (configured from the Crew page, not loopback)
//   "toolset"  — a real ToolSet manifest that declares tools
//   "other"    — any other kind (provider, env, …) not specially handled here
export type PluginCategory = "wrapper" | "adapter" | "toolset" | "other";

export function pluginCategory(p: ReluxPlugin): PluginCategory {
  // A generated wrapper is the dominant fact regardless of its declared kind.
  if (p.generated) return "wrapper";
  if (p.kind === "Adapter") return "adapter";
  if (p.kind === "ToolSet") return "toolset";
  return "other";
}

// A human label for the category, distinct enough that an operator can tell a
// real ToolSet from a metadata-only wrapper at a glance.
export function pluginKindLabel(p: ReluxPlugin): string {
  switch (pluginCategory(p)) {
    case "wrapper":
      return "Metadata-only wrapper";
    case "adapter":
      return "Adapter";
    case "toolset":
      return "ToolSet";
    default:
      return p.kind || "Plugin";
  }
}

export type StatusVariant = "ok" | "warn" | "muted";

export interface PluginStatus {
  label: string;
  variant: StatusVariant;
  title: string;
}

// The status badge. The ONE rule the mission pins: a metadata-only wrapper with
// NO tools must NOT read as ready/enabled — it shows "Needs configuration" (warn),
// because nothing about it can run yet. Once the operator has configured tools
// (tool_count > 0) it behaves like a ToolSet (enabled/disabled). Everything else
// keeps the plain enabled/disabled.
export function pluginStatus(p: ReluxPlugin): PluginStatus {
  if (pluginCategory(p) === "wrapper" && (p.tool_count ?? 0) === 0) {
    return {
      label: "Needs configuration",
      variant: "warn",
      title:
        "Installed as metadata only — Relux generated a wrapper manifest because the source had no relux-plugin.json. No tools are configured yet.",
    };
  }
  if (!p.enabled) {
    return { label: "disabled", variant: "muted", title: "This plugin is disabled." };
  }
  return { label: "enabled", variant: "ok", title: "This plugin is enabled." };
}

// Whether the operator can configure tool definitions on this plugin in-UI. The
// kernel allows it for any INSTALLED, NON-bundled ToolSet — including a generated
// metadata-only wrapper (which is a ToolSet). Bundled fixtures and non-ToolSet
// plugins (adapters, …) are refused.
export function canConfigureTools(p: ReluxPlugin): boolean {
  if (p.protected || p.bundled) return false;
  const category = pluginCategory(p);
  return category === "wrapper" || category === "toolset";
}

// A friendly group label for a read-only source hint kind (api.ts ReluxPluginHint).
// These describe what Relux DETECTED in an imported source so the operator can
// decide how to wire it up. They are advisory only: Relux never turns a hint into
// a runnable tool and never executes the source.
export function hintKindLabel(kind: string): string {
  switch (kind) {
    case "mcp-server":
      return "Possible MCP server";
    case "mcp-config":
      return "MCP config file";
    case "npm-package":
      return "npm package";
    case "npm-bin":
      return "npm executable";
    case "python-package":
      return "Python package";
    case "python-entrypoint":
      return "Python entrypoint";
    case "container":
      return "Container image";
    case "rust-crate":
      return "Rust crate";
    case "scripts":
      return "Scripts";
    case "readme":
      return "Readme";
    case "relux-manifest":
      return "Relux manifest";
    default:
      return kind;
  }
}

// An honest, advisory next-step line derived from detected hints, or null when no
// hint suggests a specific path. The strongest signal (an MCP server/config) routes
// the operator to the MCP page; otherwise we point at the in-UI tool-definition
// path. Never claims anything is runnable — that always takes explicit operator
// action and existing gates.
export function hintsNextStep(hints: ReluxPluginHint[]): string | null {
  if (!hints.length) return null;
  const has = (kind: string) => hints.some((h) => h.kind === kind);
  if (has("mcp-server") || has("mcp-config")) {
    return "This source looks like an MCP server. Relux won’t run it automatically — register it on the MCP page (or point a loopback runtime at it once you run it locally) to expose its tools through the gate.";
  }
  if (has("npm-package") || has("python-package") || has("python-entrypoint")) {
    return "This source is a package/entrypoint, not a Relux ToolSet. Run it yourself as a local loopback server, then add a tool definition below — Relux never runs downloaded code.";
  }
  return "Detected source signals only. Add a tool definition below (and a loopback runtime) to make anything runnable; Relux never infers tools or runs downloaded code.";
}

// The transport an MCP server draft registers as.
export type McpDraftTransport = "http_loopback" | "managed_stdio";

// The editable fields of the "Register MCP server" review form, pre-filled from a
// detected proposal. The operator confirms/edits these before they are POSTed to the
// EXISTING registry — Relux never auto-registers and never runs the source on import.
// A draft is either an HTTP loopback endpoint OR a managed-stdio command + args.
export interface McpRegisterDraft {
  id: string;
  transport: McpDraftTransport;
  endpoint: string;
  // The managed-stdio program (one argv token). Used when transport is "managed_stdio".
  command: string;
  // The managed-stdio args, one argv element per line (whitespace-trimmed, blanks dropped).
  argsText: string;
  // The managed-stdio env mappings, one `ENV_VAR=secret_name` per line. The value is a
  // SECRET REFERENCE (a stored secret's NAME) — never a plaintext value.
  envText: string;
  // The optional managed-stdio working directory (inside the safe workspace root).
  cwd: string;
  description: string;
}

// Parse the args textarea into argv elements: one per line, trimmed, blanks dropped.
// (Per-line — never shell-split — so an arg may safely contain spaces, e.g. JSON.)
export function parseStdioArgs(argsText: string): string[] {
  return argsText
    .split("\n")
    .map((a) => a.trim())
    .filter((a) => a.length > 0);
}

// Parse the env textarea into ordered [envVarName, secretName] pairs: one
// `ENV_VAR=secret_name` per line, trimmed, blanks dropped. A line with no `=` yields
// an empty secret name (caught by validation). The secret name is a REFERENCE — never
// a value — so no plaintext is ever entered here.
export function parseEnvMappingLines(envText: string): Array<[string, string]> {
  return envText
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0)
    .map((line) => {
      const idx = line.indexOf("=");
      if (idx < 0) return [line, ""] as [string, string];
      return [line.slice(0, idx).trim(), line.slice(idx + 1).trim()] as [string, string];
    });
}

// Build the env-ref map ({ ENV_VAR: { secret } }) from the env textarea.
export function mcpEnvFromText(envText: string): Record<string, { secret: string }> {
  const out: Record<string, { secret: string }> = {};
  for (const [v, s] of parseEnvMappingLines(envText)) out[v] = { secret: s };
  return out;
}

// Seed the review form from a server-built proposal. For a detected stdio command the
// form defaults to managed stdio with the command + args pre-filled (advisory); else
// it defaults to HTTP, the endpoint pre-filled only when a loopback address was safely
// inferred (otherwise blank — fail-closed manual entry).
export function mcpDraftFromProposal(
  p: ReluxMcpRegistrationProposal,
): McpRegisterDraft {
  const transport: McpDraftTransport =
    p.suggested_transport === "managed_stdio" ? "managed_stdio" : "http_loopback";
  return {
    id: p.suggested_id ?? "",
    transport,
    endpoint: p.suggested_endpoint ?? "",
    command: p.detected_command ?? "",
    argsText: (p.detected_args ?? []).join("\n"),
    envText: "",
    cwd: "",
    description: p.suggested_description ?? "",
  };
}

// ── Detected capability candidates (install-to-usable configuration) ──────────
// RELUX_MASTER_PLAN §8.2 "Converting an imported repo into a real plugin / tool /
// MCP config": the kernel's read-only scan now returns STRUCTURED, per-capability
// candidates (api.ts ReluxCapabilityCandidate) — not just flat hints — so the page
// can say "Detected N possible capabilities" and give each one a concrete Configure
// path. The single honesty rule mirrored from the backend: only an `mcp_register`
// candidate has a one-click governed path to a usable capability (it carries a
// pre-filled mcp_registration); a `manual` candidate is an HONEST PENDING capability
// with concrete next steps — never a faked "ready". These helpers are pure so
// `node --test` can assert the presentation without a DOM.

// A friendly label for a candidate kind.
export function candidateKindLabel(kind: string): string {
  switch (kind) {
    case "mcp_stdio":
      return "MCP server (stdio)";
    case "mcp_http":
      return "MCP server (loopback HTTP)";
    case "cli_command":
      return "Command-line tool";
    default:
      return kind;
  }
}

// A confidence badge: high reads ok, medium muted, low muted — never inflated, and a
// low/medium candidate is visibly less certain so an operator weighs it honestly.
export function candidateConfidenceBadge(confidence: string): {
  label: string;
  variant: StatusVariant;
} {
  switch (confidence) {
    case "high":
      return { label: "high confidence", variant: "ok" };
    case "medium":
      return { label: "medium confidence", variant: "muted" };
    default:
      return { label: `${confidence} confidence`, variant: "muted" };
  }
}

// True when a candidate is the one-click governed path (an MCP registration). The UI
// surfaces these first and renders a pre-filled "Register MCP server…" review form.
export function isOneClickCandidate(c: ReluxCapabilityCandidate): boolean {
  return c.activation === "mcp_register" && !!c.mcp_registration;
}

// Seed the MCP review form from a candidate's pre-filled registration. Reuses the
// same draft builder as the proposal path, so a candidate registers through the
// identical loopback-only `POST /v1/relux/mcp/servers` route + validation.
export function mcpDraftFromCandidate(c: ReluxCapabilityCandidate): McpRegisterDraft {
  if (c.mcp_registration) return mcpDraftFromProposal(c.mcp_registration);
  // Defensive: a malformed mcp_register candidate with no draft falls back to empty
  // rather than throwing — the operator still gets a usable (manual) form.
  return emptyMcpRegisterDraft();
}

// True when a candidate is a governed command-tool activation: a detected CLI/script/
// binary that carries a pre-filled argv draft. The UI renders a Configure form that
// posts to POST /v1/relux/plugins/:id/command-tools (the tool is always gated).
export function isCommandToolCandidate(c: ReluxCapabilityCandidate): boolean {
  return c.activation === "command_tool" && !!c.command_tool;
}

export interface CapabilitySummary {
  total: number;
  // Candidates Relux can activate one-click through the MCP registry.
  oneClick: number;
  // Candidates the operator can configure into a governed argv command tool.
  commandTool: number;
  // Honest pending candidates with next-steps only (nothing concrete inferred).
  manual: number;
  // True once the source was scanned but no runnable capability was detected, so the
  // UI shows exact "what to add" guidance instead of a dead end.
  emptyAfterScan: boolean;
}

// Summarize a hints payload's candidates for the panel headline + empty-state. Pure;
// degrades to an honest zero/!empty when hints are still loading (scanned undefined).
export function capabilitySummary(
  hints: ReluxPluginHints | undefined,
): CapabilitySummary {
  const candidates = hints?.candidates ?? [];
  const oneClick = candidates.filter(isOneClickCandidate).length;
  const commandTool = candidates.filter(isCommandToolCandidate).length;
  return {
    total: candidates.length,
    oneClick,
    commandTool,
    manual: candidates.length - oneClick - commandTool,
    // Only call it "empty" once we KNOW the source was scanned and produced nothing.
    emptyAfterScan: hints?.scanned === true && candidates.length === 0,
  };
}

// The form model for configuring a governed command tool (argv-only). Args are
// newline-separated so an arg may itself contain spaces (e.g. a path).
export interface CommandToolDraft {
  name: string;
  description: string;
  program: string;
  argsText: string;
  cwd: string;
  timeoutSecs: string;
  risk: string;
}

export function emptyCommandToolDraft(): CommandToolDraft {
  return {
    name: "",
    description: "",
    program: "",
    argsText: "",
    cwd: "",
    timeoutSecs: "30",
    risk: "high",
  };
}

// Seed the command-tool review form from a candidate's pre-filled argv draft. The
// program is a best-guess the operator confirms/edits before anything is stored.
export function commandToolDraftFromCandidate(
  c: ReluxCapabilityCandidate,
): CommandToolDraft {
  const p = c.command_tool;
  if (!p) return emptyCommandToolDraft();
  return {
    name: p.tool_name ?? "",
    description: p.description ?? "",
    program: p.program ?? "",
    argsText: (p.args ?? []).join("\n"),
    cwd: p.cwd ?? "",
    timeoutSecs: "30",
    risk: "high",
  };
}

// Split the newline-separated args textarea into trimmed, non-empty argv elements.
export function parseCommandArgs(argsText: string): string[] {
  return argsText
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

// Build the POST payload for the command-tools route from the form model.
export function commandToolInputFromDraft(d: CommandToolDraft): ReluxCommandToolInput {
  const timeout = Number.parseInt(d.timeoutSecs.trim(), 10);
  return {
    name: d.name.trim(),
    description: d.description.trim() || undefined,
    program: d.program.trim(),
    args: parseCommandArgs(d.argsText),
    cwd: d.cwd.trim() || undefined,
    timeout_secs: Number.isFinite(timeout) && timeout > 0 ? timeout : undefined,
    risk: d.risk,
  };
}

// Client-side pre-check for the command-tool form, mirroring the kernel's fail-closed
// argv contract (relux_core::validate_command_tool_config) so the form never sends a
// request the kernel would reject. Returns an error string, or null when submittable.
// The server remains the authoritative validator.
export function validateCommandToolDraft(d: CommandToolDraft): string | null {
  if (!d.name.trim()) return "A tool name is required.";
  const program = d.program.trim();
  if (!program) return "A program (argv[0]) is required.";
  if (/[\x00-\x1f]/.test(program)) {
    return "The program must not contain control characters.";
  }
  if (STDIO_COMMAND_METACHARS.test(program)) {
    return "The program must not contain shell metacharacters — Relux runs it argv-only, never through a shell.";
  }
  for (const arg of parseCommandArgs(d.argsText)) {
    if (DANGEROUS_STDIO_FLAGS.has(arg.toLowerCase())) {
      return `Argument "${arg}" is a forbidden bypass/danger flag.`;
    }
    if (/[\x00-\x1f]/.test(arg)) {
      return "An argument must not contain control characters.";
    }
  }
  const cwd = d.cwd.trim();
  if (cwd && cwd.split(/[/\\]/).some((seg) => seg === "..")) {
    return "The working directory must not contain a '..' parent-directory traversal.";
  }
  return null;
}

// A fresh, empty HTTP-loopback draft for the manual "Add MCP server" form.
export function emptyMcpRegisterDraft(): McpRegisterDraft {
  return {
    id: "",
    transport: "http_loopback",
    endpoint: "",
    command: "",
    argsText: "",
    envText: "",
    cwd: "",
    description: "",
  };
}

// Shell metacharacters the kernel refuses in a managed-stdio command (argv-only —
// never shelled). Mirrors relux_core::validate_stdio_command's STDIO_COMMAND_METACHARS.
const STDIO_COMMAND_METACHARS = /[;|&$`<>(){}\[\]*?!#'"]/;
const DANGEROUS_STDIO_FLAGS = new Set([
  "--dangerously-skip-permissions",
  "--dangerously-bypass-approvals-and-sandbox",
  "--yolo",
]);
// Mirrors relux_core::is_valid_env_var_name (POSIX-style) and is_valid_secret_name.
const ENV_VAR_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const SECRET_NAME = /^[A-Za-z0-9._-]+$/;

// Client-side pre-check for the review form, mirroring the kernel's fail-closed rules
// so the form never sends a request the registry would reject. Returns an error
// string, or null when the draft looks submittable. The server remains the
// authoritative validator (it re-checks the id charset, the loopback-only endpoint,
// and the argv-only command); this only catches the obvious cases early.
export function validateMcpRegisterDraft(d: McpRegisterDraft): string | null {
  const id = d.id.trim();
  if (!id) return "Server id is required.";
  // Same charset the kernel's is_valid_mcp_id enforces: letters, digits, . - _ only.
  if (!/^[A-Za-z0-9._-]+$/.test(id)) {
    return "Server id may use only letters, digits, '.', '-' or '_'.";
  }
  if (id.length > 64) return "Server id is too long (max 64 characters).";

  if (d.transport === "managed_stdio") {
    const cmd = d.command.trim();
    if (!cmd) return "A managed-stdio server requires a command (e.g. npx).";
    if (cmd.length > 256) return "Command is too long (max 256 characters).";
    if (STDIO_COMMAND_METACHARS.test(cmd)) {
      return "Command must not contain shell metacharacters — Relux runs it directly (argv only), never through a shell.";
    }
    const args = parseStdioArgs(d.argsText);
    if (args.length > 64) return "Too many args (max 64).";
    for (const a of args) {
      if (DANGEROUS_STDIO_FLAGS.has(a.toLowerCase())) {
        return `Arg "${a}" is a forbidden bypass/danger flag and cannot be used.`;
      }
    }
    // Env mappings: each is `ENV_VAR=secret_name` — a secret REFERENCE, never a value.
    const envPairs = parseEnvMappingLines(d.envText);
    if (envPairs.length > 64) return "Too many env vars (max 64).";
    for (const [v, s] of envPairs) {
      if (!ENV_VAR_NAME.test(v)) {
        return `Env var name "${v || "(empty)"}" is invalid (letters/digits/_ only, not starting with a digit).`;
      }
      if (!SECRET_NAME.test(s)) {
        return `Env var "${v}" must reference a secret by name (e.g. ${v}=my_api_key); enter a secret NAME, never a value.`;
      }
    }
    // Working directory: optional, bounded, no `..` traversal.
    const cwd = d.cwd.trim();
    if (cwd) {
      if (cwd.length > 1024) return "Working directory path is too long (max 1024 characters).";
      if (cwd.split(/[\\/]/).includes("..")) {
        return "Working directory must not contain '..' (no parent-directory traversal).";
      }
    }
    return null;
  }

  if (!d.endpoint.trim()) {
    return "Loopback endpoint is required (e.g. http://127.0.0.1:8000/mcp).";
  }
  return null;
}

// Build the POST body for `reluxMcp.register` from a validated draft, dispatching on
// the transport. Stdio args are parsed per-line (never shell-split).
export function mcpRegisterBody(d: McpRegisterDraft): {
  id: string;
  transport: McpDraftTransport;
  endpoint?: string;
  command?: string;
  args?: string[];
  env?: Record<string, { secret: string }>;
  cwd?: string;
  description?: string;
} {
  if (d.transport === "managed_stdio") {
    const env = mcpEnvFromText(d.envText);
    const cwd = d.cwd.trim();
    return {
      id: d.id.trim(),
      transport: "managed_stdio",
      command: d.command.trim(),
      args: parseStdioArgs(d.argsText),
      env: Object.keys(env).length > 0 ? env : undefined,
      cwd: cwd || undefined,
      description: d.description.trim() || undefined,
    };
  }
  return {
    id: d.id.trim(),
    transport: "http_loopback",
    endpoint: d.endpoint.trim(),
    description: d.description.trim() || undefined,
  };
}

export type NextStepKind =
  | "add-manifest"
  | "configure-runtime"
  | "configure-adapter"
  | "none";

export interface PluginNextStep {
  kind: NextStepKind;
  // The short button label.
  cta: string;
  // One honest sentence on what this step does and why it's the right next one.
  detail: string;
}

// The single most useful next action for a plugin, or a "none" step for things
// that are already configured-as-far-as-they-go (e.g. bundled/protected plugins).
export function pluginNextStep(p: ReluxPlugin): PluginNextStep {
  const category = pluginCategory(p);

  // A generated wrapper with NO tools: the honest next step is to ADD tool
  // definitions (a loopback runtime alone would surface nothing). Once tools exist
  // the next step becomes pointing a runtime at a local server (handled below by
  // the shared toolset branch).
  if (category === "wrapper" && (p.tool_count ?? 0) === 0) {
    return {
      kind: "add-manifest",
      cta: "Configure",
      detail:
        "This wrapper declares no tools, so a runtime alone runs nothing. Add a tool definition (it stays disabled until you enable a loopback runtime), then point that runtime at your local server.",
    };
  }

  // Adapters are enabled from the Crew page (local CLI runtimes), not via the
  // loopback ToolSet runtime. A bundled adapter (the shipped Claude/Codex CLIs) is
  // protected — it cannot be REMOVED — but that is NOT the same as having no way to
  // USE it: it still gets a real "Configure" path so it never reads as a mysterious
  // locked row. Enable the local CLI runtime on the Crew page, then select it as
  // Prime's brain in Settings.
  if (category === "adapter") {
    return {
      kind: "configure-adapter",
      cta: "Configure on Crew",
      detail: p.protected
        ? "Bundled CLI adapter (Claude/Codex). Enable its local CLI runtime on the Crew page, then select it as Prime's brain in Settings. Protected: it cannot be removed, but it is fully configurable."
        : "Enable this adapter's local CLI runtime from the Crew page.",
    };
  }

  // Bundled (protected) ToolSets are built-in and already runnable; nothing to do.
  if (p.protected) {
    return { kind: "none", cta: "", detail: "Bundled plugin; built-in and runnable." };
  }

  // A ToolSet with tools — including a wrapper the operator has now added tools to:
  // point Relux at a loopback server to run them.
  if (category === "toolset" || category === "wrapper") {
    const n = p.tool_count ?? 0;
    return {
      kind: "configure-runtime",
      cta: "Runtime",
      detail:
        n > 0
          ? `Point Relux at a loopback HTTP server to make ${n} tool${n === 1 ? "" : "s"} runnable.`
          : "This ToolSet declares no tools yet; add tool definitions before a runtime helps.",
    };
  }

  return { kind: "none", cta: "", detail: "" };
}

// ── Guided configuration checklist (metadata-only imports) ─────────────────────
// RELUX_MASTER_PLAN "Tool Invocation Workflow + Honest Readiness": a generated
// metadata-only wrapper becomes usable through a FIXED, documented sequence —
//   1. install → a metadata-only wrapper (nothing runnable)
//   2. add a tool definition (or register an MCP server)
//   3. enable a loopback runtime so a tool flips to `ready`
//   4. invoke it from a tool-run task / Prime / Work.
// The panels to do each step already exist, but were never PRESENTED as an
// ordered checklist, so an operator had to guess the order (and a runtime alone,
// the wrong first move, surfaces nothing). This derives that exact sequence's
// honest status from REAL state and is the single source of truth the
// GuidedConfigChecklist renders. It invents no new authority: a step is
// `actionable` only when the backend already supports it, and when it is not,
// `missing` says exactly what is absent. Relux never infers tools or runs
// downloaded code (§18).

export type ConfigStepStatus = "done" | "current" | "upcoming" | "blocked";

export interface ConfigStep {
  key: "review" | "define" | "runtime" | "use";
  title: string;
  status: ConfigStepStatus;
  // One honest sentence on what this step is / why it sits where it does.
  detail: string;
  // True when the operator can act on this step right now with a supported action.
  actionable: boolean;
  // When NOT actionable, exactly what is missing (an earlier step's output, or a
  // capability the backend does not support). Undefined when actionable.
  missing?: string;
}

export interface GuidedConfig {
  steps: ConfigStep[];
  doneCount: number;
  total: number;
  // True once at least one tool on this plugin is `ready` — runnable end to end.
  complete: boolean;
}

// `tools` are the descriptors for THIS plugin (filtered defensively here too).
// `runtime`/`hints` are optional: when still loading they are undefined and the
// checklist degrades to an honest "not yet known" rather than claiming progress.
export function guidedConfigSteps(
  plugin: ReluxPlugin,
  tools: ReluxToolDescriptor[],
  runtime: ReluxPluginRuntime | undefined,
  hints: ReluxPluginHints | undefined,
): GuidedConfig {
  const configurable = canConfigureTools(plugin);
  const myTools = tools.filter((t) => t.plugin_id === plugin.id);
  const hasTool = myTools.length > 0;
  const hasReady = myTools.some((t) => t.executable === "ready");
  const runtimeReady = !!runtime?.configured && !!runtime?.enabled;
  const hasMcpProposal = !!hints?.mcp_proposal;
  const scannedFalse = hints?.scanned === false;

  // 1 — Review the imported source (read-only hints). Done once the source has
  // been scanned (or any tool already exists, which means it was reviewed).
  const reviewDone = hasTool || hints?.scanned === true;
  const review: ConfigStep = {
    key: "review",
    title: "Review the imported source",
    status: reviewDone ? "done" : "current",
    detail: scannedFalse
      ? "Nothing to inspect — the source is not in the local plugins directory. You can still define a tool manually."
      : "The read-only “Detected in source” hints below show what the source is (a possible MCP server, an npm/python package, scripts). Relux never runs the source or turns a hint into a tool.",
    actionable: true,
  };

  // 2 — Add a tool definition (or, for an MCP source, register an MCP server).
  let define: ConfigStep;
  if (hasTool) {
    define = {
      key: "define",
      title: "Add a tool definition",
      status: "done",
      detail: `${myTools.length} tool${myTools.length === 1 ? "" : "s"} defined. Add more below, or remove any you don't want.`,
      actionable: configurable,
    };
  } else if (!configurable) {
    define = {
      key: "define",
      title: "Add a tool definition",
      status: "blocked",
      detail:
        "Only an installed, non-bundled ToolSet (a generated wrapper is one) can have tool definitions added in the dashboard.",
      actionable: false,
      missing:
        "This plugin kind can't be configured in-UI — it is bundled/protected or not a ToolSet.",
    };
  } else {
    define = {
      key: "define",
      title: hasMcpProposal ? "Register an MCP server, or add a tool" : "Add a tool definition",
      status: "current",
      detail: hasMcpProposal
        ? "This source looks like an MCP server: register it below (review form, loopback-only) and Discover its tools, OR add a tool definition that points at a loopback HTTP server you run. Relux runs neither for you."
        : "Add a tool definition below — a name, description and risk. Relux derives the permission and never infers tools from the source.",
      actionable: true,
    };
  }

  // 3 — Enable a loopback runtime so a defined tool can become `ready`.
  let runtimeStep: ConfigStep;
  if (!hasTool) {
    runtimeStep = {
      key: "runtime",
      title: "Enable a loopback runtime",
      status: "upcoming",
      detail:
        "A tool runs only through an HTTP loopback server you run locally. Enable that runtime once a tool exists — a runtime with no tools surfaces nothing.",
      actionable: false,
      missing: "Add a tool definition first (step 2).",
    };
  } else if (runtimeReady) {
    runtimeStep = {
      key: "runtime",
      title: "Enable a loopback runtime",
      status: "done",
      detail:
        "A loopback runtime is enabled, so a low-risk tool can be ready. Update or disable it on the plugin row's Runtime panel.",
      actionable: true,
    };
  } else {
    runtimeStep = {
      key: "runtime",
      title: "Enable a loopback runtime",
      status: "current",
      detail: runtime?.configured
        ? "A loopback runtime is configured but disabled — enable it on the Runtime panel."
        : "Point Relux at the http://127.0.0.1:<port> server you run (the Runtime panel) so the tool can run. Relux never auto-runs downloaded code.",
      actionable: true,
    };
  }

  // 4 — Use it from a tool-run task / Prime / Work.
  let use: ConfigStep;
  if (hasReady) {
    use = {
      key: "use",
      title: "Use it from Prime or the Work board",
      status: "done",
      detail:
        "A tool is ready. Add it as a step in a “Create a tool-run task” here, or ask Prime to run it — every call is permission-checked, approval-gated, and audited.",
      actionable: true,
    };
  } else {
    use = {
      key: "use",
      title: "Use it from Prime or the Work board",
      status: "upcoming",
      detail:
        "Once a tool is ready you can run it from a tool-run task on this page, or from Prime and the Work board.",
      actionable: false,
      missing: !hasTool
        ? "No tool is defined yet (step 2)."
        : !runtimeReady
          ? "No tool is ready — enable a loopback runtime (step 3) and keep the tool low-risk with auto-approve."
          : "The defined tool isn't ready — it may require approval (higher risk) or the calling agent lacks its permission.",
    };
  }

  const steps = [review, define, runtimeStep, use];
  return {
    steps,
    doneCount: steps.filter((s) => s.status === "done").length,
    total: steps.length,
    complete: hasReady,
  };
}

// ── Live adapter runtime status (Plugins page) ────────────────────────────────
// An Adapter plugin row's enabled/disabled flag is just the plugin RECORD; it
// does NOT say whether the local CLI can actually run. The truth lives in the
// kernel's runtime probe (`GET /v1/relux/adapters`, the SAME endpoint the Crew
// adapters section uses) which reports `state`:
//   local_deterministic | available | missing_binary | disabled | needs_configuration
// We surface that live state inline on the Plugins page so an operator sees, at a
// glance, whether Claude/Codex/Local Prime is available, enabled, disabled,
// missing its binary, or needs configuration — without re-probing or faking it.

// The human label for an adapter runtime state. Single source of truth, shared
// with the Crew adapters section so the two surfaces never disagree on what
// "available" vs "disabled" means.
export const ADAPTER_STATE_LABEL: Record<ReluxAdapterStatus["state"], string> = {
  local_deterministic: "Local (deterministic)",
  available: "Enabled — ready",
  missing_binary: "Enabled — binary missing",
  disabled: "Configured — disabled",
  needs_configuration: "Disabled (default)",
};

// The live runtime badge for an adapter plugin row. `runtime` is the matching
// ReluxAdapterStatus from /v1/relux/adapters, or `undefined` when it could not be
// resolved (the adapters probe failed, or no row matched this plugin id). We never
// fake "ready": an unresolved runtime reads as an honest muted "status unavailable"
// badge, never as available/enabled.
//
//   available / local_deterministic → ok   (runnable now)
//   missing_binary / needs_configuration → warn (action needed before it can run)
//   disabled → muted (deliberately off)
//   unresolved → muted "status unavailable" (honest, not "ready")
export function adapterStatusBadge(
  runtime: ReluxAdapterStatus | undefined,
): PluginStatus {
  if (!runtime) {
    return {
      label: "status unavailable",
      variant: "muted",
      title:
        "Could not read this adapter's live runtime status from /v1/relux/adapters.",
    };
  }
  const label = ADAPTER_STATE_LABEL[runtime.state] ?? runtime.state;
  let variant: StatusVariant;
  switch (runtime.state) {
    case "available":
    case "local_deterministic":
      variant = "ok";
      break;
    case "missing_binary":
    case "needs_configuration":
      variant = "warn";
      break;
    default:
      // disabled (and any unknown future state) is muted, never ready-looking.
      variant = "muted";
      break;
  }
  return { label, variant, title: runtime.detail };
}

export interface InstallSummary {
  headline: string;
  detail: string;
  // "ok"   — installed something immediately usable (real tools / adapter)
  // "info" — installed, but a next step is required before anything runs
  tone: "ok" | "info";
}

// What to tell the operator right after an install: whether real tools were
// discovered, a metadata-only wrapper was generated, or an adapter landed — and
// the exact next step. This is the honest "what just happened + what now" the
// install flow owes the user (mission: install result summary).
export function installResultSummary(p: ReluxPlugin): InstallSummary {
  const name = p.name || p.id;
  const category = pluginCategory(p);

  if (category === "wrapper") {
    return {
      tone: "info",
      headline: `Imported ${name} as metadata-only — no Relux manifest needed.`,
      detail:
        "The source had no relux-plugin.json (that file is optional — only first-class Relux plugins ship one), so Relux created a safe metadata-only plugin and discovered no tools. Nothing runs until you configure it: open “Configure” on its row to review detected hints, register an MCP server, or add a tool definition. Relux never infers tools or runs downloaded code.",
    };
  }

  if (category === "adapter") {
    return {
      tone: "ok",
      headline: `Installed adapter ${name}.`,
      detail:
        "Adapters drive how assigned tasks run. Enable its local CLI runtime from the Crew page (disabled by default).",
    };
  }

  if (category === "toolset") {
    const n = p.tool_count ?? 0;
    if (n > 0) {
      return {
        tone: "ok",
        headline: `Installed ${name} — discovered ${n} tool${n === 1 ? "" : "s"}.`,
        detail:
          "Point Relux at a loopback HTTP server you run (“Runtime” on its row) to make the tools runnable. Relux never auto-runs downloaded code.",
      };
    }
    return {
      tone: "info",
      headline: `Installed ${name}.`,
      detail:
        "The manifest declares no tools yet, so nothing is runnable. Add tool definitions before configuring a runtime.",
    };
  }

  return {
    tone: "ok",
    headline: `Installed ${name} v${p.version}.`,
    detail: "See its row for configuration options.",
  };
}

// Make the GitHub install field forgiving: accept the `owner/repo` shorthand in
// addition to a full `https://github.com/owner/repo` URL. PURE and conservative —
// it expands ONLY the exact `owner/repo[.git]` shape into the canonical
// `https://github.com/owner/repo`; anything else (a full URL, an ssh/git URL,
// junk, or a credentialed URL) is passed through trimmed so the kernel's
// authoritative `validate_github_url` stays the real gate. It never injects
// credentials and never rewrites a scheme, so it cannot turn an unsafe input into
// an accepted one — at worst the server still rejects it.
export function normalizeGithubUrl(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return trimmed;
  // Anything that already carries a scheme (https://, git://, ssh://, …) is left
  // verbatim for the server validator — never rewritten.
  if (trimmed.includes("://")) return trimmed;
  // Exactly `owner/repo` (optionally a trailing `.git` and/or `/`): two segments
  // of GitHub-legal name chars, no extra slashes, no spaces, no `@` credentials.
  const m = /^([A-Za-z0-9][A-Za-z0-9._-]*)\/([A-Za-z0-9][A-Za-z0-9._-]*?)(?:\.git)?\/?$/.exec(
    trimmed,
  );
  if (m) return `https://github.com/${m[1]}/${m[2]}`;
  // Not a recognized shorthand — pass through unchanged; the server decides.
  return trimmed;
}

// ── Tool readiness (the honest invocation surface) ────────────────────────────
// ONE classifier from the kernel's `executable` state to what the operator sees
// and may do — mirroring openclaw's approval-classifier
// (`reference/openclaw-main/src/acp/approval-classifier.ts`, where a single
// function maps a tool to a named class and only the safe classes auto-approve;
// every other class carries an explicit reason and is never auto-run). Here
// `runnable` is true ONLY for "ready"; every other state is non-runnable with a
// concrete reason + next step, so a non-ready tool reads as a clear refusal /
// disabled state — never a blank row, never a pretend invocation. The kernel is
// authoritative (it refuses the same states in `call_tool`/`invoke_tool`); this
// just renders the same truth honestly.
export interface ToolReadiness {
  // True ONLY when the kernel reports the tool ready to invoke directly.
  runnable: boolean;
  // True ONLY for `needs_approval`: the tool is gated, but the operator can
  // request a per-call approval for a specific invocation (and then execute it
  // once from the Approvals page). The direct invoke path stays refused.
  canRequestApproval: boolean;
  // Short badge label.
  label: string;
  // ok = runnable now; warn = an operator action would make it runnable; muted =
  // unsupported / off, nothing to do here.
  tone: StatusVariant;
  // One honest sentence: why it is (not) runnable.
  reason: string;
  // The concrete next step an operator can take, or undefined when there is none.
  nextStep?: string;
}

export function toolReadiness(t: ReluxToolDescriptor): ToolReadiness {
  switch (t.executable) {
    case "ready":
      return {
        runnable: true,
        canRequestApproval: false,
        label: "ready",
        tone: "ok",
        reason:
          "A built-in handler or an enabled HTTP loopback runtime backs this tool. Every call is permission-checked and audited.",
      };
    case "needs_approval":
      return {
        runnable: false,
        canRequestApproval: true,
        label: "needs approval",
        tone: "warn",
        reason: `Configured as ${t.risk}-risk, so it requires approval and is refused on the direct invoke path — it is never run just because a runtime is enabled.`,
        nextStep:
          "Request a per-call approval for a specific invocation below. Once an operator approves it on the Approvals page, that exact call can be executed once. (Or lower its risk to low with auto-approve to make it directly callable.)",
      };
    case "runtime_not_configured":
      return {
        runnable: false,
        canRequestApproval: false,
        label: "runtime not configured",
        tone: "warn",
        reason:
          "Installed, but no runtime backs it yet. Relux never auto-runs downloaded code — it only calls a local loopback server you run.",
        nextStep:
          'Point this plugin at an HTTP loopback endpoint ("Runtime" on its plugin row), then invoke.',
      };
    case "runtime_disabled":
      return {
        runnable: false,
        canRequestApproval: false,
        label: "runtime disabled",
        tone: "warn",
        reason:
          "An HTTP loopback runtime is configured for this plugin but is currently disabled, so invocation is refused.",
        nextStep: "Re-enable the loopback runtime on its plugin row to make this tool callable again.",
      };
    case "missing_permission":
      return {
        runnable: false,
        canRequestApproval: false,
        label: "missing permission",
        tone: "warn",
        reason: `The scoped agent does not hold this tool's permission (${t.permission}).`,
        nextStep: "Grant the permission to the agent, or invoke as an agent that already holds it.",
      };
    case "not_implemented":
    default:
      return {
        runnable: false,
        canRequestApproval: false,
        label: "runtime not implemented",
        tone: "muted",
        reason:
          "The kernel has no supported runtime for this tool. Listed honestly rather than pretending to run.",
      };
  }
}

// ── MCP servers (loopback HTTP discovery — MCP v1) ────────────────────────────
// An MCP server row's badge reflects only its stored config: `configured`
// (enabled) vs `disabled`. Reachability is dynamic (it needs a live `tools/list`
// probe) and is reported separately by the discovery action — never faked as
// "ready" here. This mirrors the adapter-row honesty rule.
export function mcpServerStatusBadge(server: ReluxMcpServer): PluginStatus {
  if (!server.enabled) {
    return {
      label: "disabled",
      variant: "muted",
      title: "This MCP server is registered but disabled; enable it to discover its tools.",
    };
  }
  return {
    label: "configured",
    variant: "ok",
    title:
      "Registered and enabled. Use Discover to run a live tools/list against the loopback server.",
  };
}

// ── Managed-stdio process lifecycle ───────────────────────────────────────────
// A managed-stdio server is registered (config) independently of whether its
// process is running. This badge reflects the LIVE process state reported by the
// kernel's managed pool: stopped / starting / running / failed. Honest by
// construction — `failed` surfaces the redacted reason; nothing is faked.
export function managedStdioStatusBadge(
  status: ReluxManagedStdioStatus,
): PluginStatus {
  switch (status.state) {
    case "running":
      return {
        label: status.pid ? `running · pid ${status.pid}` : "running",
        variant: "ok",
        title:
          "The managed process is up; Discover and tool calls reuse it (one initialized process, no per-call spawn).",
      };
    case "starting":
      return {
        label: "starting",
        variant: "muted",
        title: "The managed process is spawning and running its initialize handshake.",
      };
    case "failed":
      return {
        label: "failed",
        variant: "warn",
        title: status.last_error
          ? `The managed process failed: ${status.last_error}`
          : "The managed process failed to start or died. Restart it to try again.",
      };
    default:
      return {
        label: "stopped",
        variant: "muted",
        title:
          "No managed process is running. Discover still works (spawn-per-operation); Start one to reuse a single long-lived process.",
      };
  }
}

export interface VisibleTools {
  shown: ReluxToolDescriptor[];
  hiddenCount: number;
}

// A tool is "runnable" when the kernel reports it ready (built-in or an enabled
// loopback runtime). By default the Tools list shows ONLY runnable tools so a
// metadata-only or unconfigured plugin never looks usable; a toggle reveals the
// rest with their honest non-runnable status. Nothing is permanently hidden.
export function isRunnableTool(t: ReluxToolDescriptor): boolean {
  return toolReadiness(t).runnable;
}

export function visibleTools(
  tools: ReluxToolDescriptor[],
  showAll: boolean,
): VisibleTools {
  if (showAll) return { shown: tools, hiddenCount: 0 };
  const shown = tools.filter(isRunnableTool);
  return { shown, hiddenCount: tools.length - shown.length };
}
