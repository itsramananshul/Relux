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
  ReluxMcpRegistrationProposal,
  ReluxMcpServer,
  ReluxPlugin,
  ReluxPluginHint,
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

// The editable fields of the "Register MCP server" review form, pre-filled from a
// detected proposal. The operator confirms/edits these before they are POSTed to the
// EXISTING loopback registry — Relux never auto-registers and never runs the source.
export interface McpRegisterDraft {
  id: string;
  endpoint: string;
  description: string;
}

// Seed the review form from a server-built proposal. The endpoint is pre-filled only
// when the server could safely infer a loopback address; otherwise it stays blank so
// the operator must supply it (fail-closed manual entry).
export function mcpDraftFromProposal(
  p: ReluxMcpRegistrationProposal,
): McpRegisterDraft {
  return {
    id: p.suggested_id ?? "",
    endpoint: p.suggested_endpoint ?? "",
    description: p.suggested_description ?? "",
  };
}

// Client-side pre-check for the review form, mirroring the kernel's fail-closed
// rules so the form never sends a request the registry would reject. Returns an
// error string, or null when the draft looks submittable. The server remains the
// authoritative validator (it re-checks the id charset and the loopback-only
// endpoint); this only catches the obvious cases early for a clean message.
export function validateMcpRegisterDraft(d: McpRegisterDraft): string | null {
  const id = d.id.trim();
  if (!id) return "Server id is required.";
  // Same charset the kernel's is_valid_mcp_id enforces: letters, digits, . - _ only.
  if (!/^[A-Za-z0-9._-]+$/.test(id)) {
    return "Server id may use only letters, digits, '.', '-' or '_'.";
  }
  if (id.length > 64) return "Server id is too long (max 64 characters).";
  if (!d.endpoint.trim()) {
    return "Loopback endpoint is required (e.g. http://127.0.0.1:8000/mcp).";
  }
  return null;
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
      headline: `Installed ${name} as a metadata-only wrapper.`,
      detail:
        "The source had no relux-plugin.json, so Relux generated a safe wrapper and discovered no tools. Nothing is runnable yet — open “Set up” on its row to add tool definitions. Relux never infers tools or runs downloaded code.",
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
