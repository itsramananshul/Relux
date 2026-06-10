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

import type { ReluxPlugin, ReluxToolDescriptor } from "./api";

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

// The status badge. The ONE rule the mission pins: a metadata-only wrapper must
// NOT read as ready/enabled — it shows "Needs configuration" (warn), because
// nothing about it can run yet. Everything else keeps the plain enabled/disabled.
export function pluginStatus(p: ReluxPlugin): PluginStatus {
  if (pluginCategory(p) === "wrapper") {
    return {
      label: "Needs configuration",
      variant: "warn",
      title:
        "Installed as metadata only — Relux generated a wrapper manifest because the source had no relux-plugin.json. No tools are runnable yet.",
    };
  }
  if (!p.enabled) {
    return { label: "disabled", variant: "muted", title: "This plugin is disabled." };
  }
  return { label: "enabled", variant: "ok", title: "This plugin is enabled." };
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

  // A generated wrapper: the honest next step is to ADD a manifest with tool
  // definitions. A loopback runtime alone would surface nothing.
  if (category === "wrapper") {
    return {
      kind: "add-manifest",
      cta: "Set up",
      detail:
        "This wrapper declares no tools, so a runtime alone runs nothing. Add a relux-plugin.json with tool definitions, re-install, then point a loopback runtime at your local server.",
    };
  }

  // Adapters are enabled from the Crew page (local CLI runtimes), not via the
  // loopback ToolSet runtime. Bundled adapters are locked here anyway.
  if (category === "adapter") {
    if (p.protected) {
      return { kind: "none", cta: "", detail: "Bundled adapter; configure it on the Crew page." };
    }
    return {
      kind: "configure-adapter",
      cta: "Configure on Crew",
      detail: "Enable this adapter's local CLI runtime from the Crew page.",
    };
  }

  // Bundled (protected) ToolSets are built-in and already runnable; nothing to do.
  if (p.protected) {
    return { kind: "none", cta: "", detail: "Bundled plugin; built-in and runnable." };
  }

  // A real ToolSet with tools: point Relux at a loopback server to run them.
  if (category === "toolset") {
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

export interface VisibleTools {
  shown: ReluxToolDescriptor[];
  hiddenCount: number;
}

// A tool is "runnable" when the kernel reports it ready (built-in or an enabled
// loopback runtime). By default the Tools list shows ONLY runnable tools so a
// metadata-only or unconfigured plugin never looks usable; a toggle reveals the
// rest with their honest non-runnable status. Nothing is permanently hidden.
export function isRunnableTool(t: ReluxToolDescriptor): boolean {
  return t.executable === "ready";
}

export function visibleTools(
  tools: ReluxToolDescriptor[],
  showAll: boolean,
): VisibleTools {
  if (showAll) return { shown: tools, hiddenCount: 0 };
  const shown = tools.filter(isRunnableTool);
  return { shown, hiddenCount: tools.length - shown.length };
}
