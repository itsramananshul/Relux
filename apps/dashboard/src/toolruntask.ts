// Pure, dependency-free builder for a "tool-run task" creation payload — the
// compact operator UI for the backend's run-driven single tool-call directive and
// bounded multi-tool plan (`docs/mcp.md` "Run-driven MCP tool call" + "Run-driven
// multi-tool plan"). One step builds a `tool_call`; two-to-five steps build a
// `tool_plan`. The bounds and shape mirror the kernel's create-time validation
// (`relux_core::TaskToolPlan::validate` + `CreateTaskReq`) so the UI fails closed
// the SAME way the backend does, rather than posting a request the kernel will 400.
//
// Kept React-free (like ./plugins and ./routing) so `node --test` can pin the
// payload shape and every validation branch without a DOM. The form renders
// whatever this returns and invents nothing.

import type { ReluxToolDescriptor } from "./api";

// The single tool-call directive shape the backend accepts (`CreateTaskReq.tool_call`
// / `relux_core::TaskToolCall`). `args` defaults to `{}` when the operator leaves it
// blank — exactly as the kernel does (`#[serde(default)]` on `args`).
export interface ToolCallDirective {
  plugin: string;
  tool: string;
  args: unknown;
}

// The create-task body this builder emits. Exactly one of `tool_call` (one step) or
// `tool_plan` (multiple steps) is present — never both (the backend 400s on both),
// never neither (a step is required to build a tool-run task).
export interface ToolRunTaskPayload {
  title: string;
  tool_call?: ToolCallDirective;
  tool_plan?: ToolCallDirective[];
}

// One operator-authored step as the form holds it: the chosen tool (a plugin id +
// tool name) and the raw JSON-args TEXT the operator typed (parsed/validated here).
export interface ToolRunStep {
  plugin: string;
  tool: string;
  // Raw textarea contents. Blank => `{}`. Anything else must parse as JSON.
  argsText: string;
}

export type BuildResult =
  | { ok: true; payload: ToolRunTaskPayload }
  | { ok: false; error: string };

// Mirrors `relux_core::MAX_TASK_TOOL_PLAN_STEPS` (5). A plan may carry at most this
// many steps; the kernel 400s an over-long plan (never silently truncates), so the
// UI refuses it up front with the same ceiling.
export const MAX_TOOL_RUN_STEPS = 5;

// Parse one step's JSON args exactly as the kernel will read them: a blank textarea
// is the canonical empty `{}`; any other text must be valid JSON. Returns the parsed
// value or a human error naming the 1-based step (so the form can point at the row).
function parseStepArgs(argsText: string, stepNo: number): { ok: true; args: unknown } | { ok: false; error: string } {
  const trimmed = argsText.trim();
  if (!trimmed) return { ok: true, args: {} };
  try {
    return { ok: true, args: JSON.parse(trimmed) };
  } catch {
    return { ok: false, error: `Step ${stepNo}: arguments must be valid JSON (or empty for {}).` };
  }
}

// Build the create-task payload from a title + ordered steps, failing closed the
// same way the backend does:
//   - title required (trimmed);
//   - at least one step, at most MAX_TOOL_RUN_STEPS (never silently truncated);
//   - every step needs a non-empty plugin + tool (trimmed);
//   - every step's args must be valid JSON (blank => {}).
// One valid step => a `tool_call`; two-or-more => a `tool_plan` (run sequentially,
// stopping on the first failure). The caller posts the returned payload verbatim to
// `POST /v1/relux/tasks` (`reluxWork.createTask`).
export function buildToolRunTaskPayload(title: string, steps: ToolRunStep[]): BuildResult {
  const trimmedTitle = title.trim();
  if (!trimmedTitle) return { ok: false, error: "A task title is required." };

  if (steps.length === 0) return { ok: false, error: "Add at least one tool step." };
  if (steps.length > MAX_TOOL_RUN_STEPS) {
    return {
      ok: false,
      error: `A tool plan may have at most ${MAX_TOOL_RUN_STEPS} steps (you have ${steps.length}).`,
    };
  }

  const directives: ToolCallDirective[] = [];
  for (let i = 0; i < steps.length; i++) {
    const step = steps[i];
    const stepNo = i + 1;
    const plugin = step.plugin.trim();
    const tool = step.tool.trim();
    if (!plugin || !tool) {
      return { ok: false, error: `Step ${stepNo}: choose a tool (plugin and tool are required).` };
    }
    const parsed = parseStepArgs(step.argsText, stepNo);
    if (!parsed.ok) return { ok: false, error: parsed.error };
    directives.push({ plugin, tool, args: parsed.args });
  }

  // One step is the single tool-call directive; more than one is the multi-tool plan.
  // This is the SAME split the docs describe (1 => tool_call, N => tool_plan).
  if (directives.length === 1) {
    return { ok: true, payload: { title: trimmedTitle, tool_call: directives[0] } };
  }
  return { ok: true, payload: { title: trimmedTitle, tool_plan: directives } };
}

// ── Tool picker options (installed plugin tools + live MCP-discovered tools) ──
// The "Create a tool-run task" picker offers EVERY tool an operator can put in a
// directive: the installed plugin tools (from `reluxTools.list`) AND the tools a
// live `tools/list` discovery surfaced from each ENABLED MCP server (from
// `reluxMcp.tools`). An MCP tool's directive uses the stable plugin id
// `mcp:<server>` and the discovered tool name — exactly what the kernel routes a
// run-driven MCP call through (`plugin_id = "mcp:<server>"`, `docs/mcp.md`
// "Run-driven MCP tool call"). Gating is decided by the caller's `isGated`
// predicate (the SAME `toolReadiness` rule the Tools list uses) so a gated tool is
// labelled "needs approval" rather than hidden. Kept React-free + classifier-free
// (it imports no runtime) so `node --test` can pin the merge without a DOM.

// One option in the picker. `key` is the `<select>` value the form splits back into
// (plugin, tool) on a single space — neither a plugin id (`mcp:<server>` or a slug)
// nor a tool name contains a space, so the round-trip is lossless.
export interface ToolPickerOption {
  key: string;
  // A plugin id, or `mcp:<server>` for an MCP-discovered tool.
  plugin: string;
  tool: string;
  // Human label; an MCP-discovered tool reads as `mcp:<server>/<tool>`.
  label: string;
  // True => the run will need an approval/grant for this tool (non-runnable now).
  gated: boolean;
  source: "plugin" | "mcp";
}

// One registered MCP server's outcome as the picker sees it. A DISABLED server is
// not discovered (its tools are absent by design); an ENABLED server either
// resolved a (possibly empty) tool list or FAILED discovery (server down / not
// speaking MCP). The component fills this in from `reluxMcp.list` + `reluxMcp.tools`.
export interface McpServerDiscovery {
  serverId: string;
  enabled: boolean;
  // Present when enabled and discovery resolved (possibly empty).
  tools?: ReluxToolDescriptor[];
  // True when enabled but the live `tools/list` errored.
  failed?: boolean;
  error?: string;
}

export interface ToolPickerModel {
  options: ToolPickerOption[];
  // Enabled servers whose live discovery errored — surfaced as a WARNING so a
  // failed server never silently vanishes from the picker without explanation.
  failures: { serverId: string; error?: string }[];
  // Registered-but-disabled servers — surfaced as INFO (enable to discover). Their
  // tools are deliberately absent, not failed.
  disabledServers: string[];
}

// Merge installed plugin tools and per-server MCP discovery outcomes into the
// picker's options + the honest notes (failed discovery, disabled servers). The
// caller supplies `isGated` so this stays free of the readiness classifier (which
// imports the API types) — typically `(t) => !toolReadiness(t).runnable`.
export function buildToolPickerOptions(
  installed: ReluxToolDescriptor[],
  mcp: McpServerDiscovery[],
  isGated: (t: ReluxToolDescriptor) => boolean,
): ToolPickerModel {
  const options: ToolPickerOption[] = [];
  const seen = new Set<string>();

  const push = (
    plugin: string,
    tool: string,
    label: string,
    gated: boolean,
    source: "plugin" | "mcp",
  ) => {
    const p = plugin.trim();
    const t = tool.trim();
    if (!p || !t) return;
    const key = `${p} ${t}`;
    if (seen.has(key)) return; // first occurrence wins; never list the same tool twice
    seen.add(key);
    options.push({ key, plugin: p, tool: t, label, gated, source });
  };

  for (const t of installed) {
    const gated = isGated(t);
    push(
      t.plugin_id,
      t.tool_name,
      `${t.tool_name} (${t.plugin_id})${gated ? " — needs approval" : ""}`,
      gated,
      "plugin",
    );
  }

  const failures: { serverId: string; error?: string }[] = [];
  const disabledServers: string[] = [];
  for (const s of mcp) {
    if (!s.enabled) {
      disabledServers.push(s.serverId);
      continue;
    }
    if (s.failed) {
      failures.push({ serverId: s.serverId, error: s.error });
      continue;
    }
    const plugin = `mcp:${s.serverId}`;
    for (const t of s.tools ?? []) {
      const gated = isGated(t);
      push(
        plugin,
        t.tool_name,
        `mcp:${s.serverId}/${t.tool_name}${gated ? " — needs approval" : ""}`,
        gated,
        "mcp",
      );
    }
  }

  return { options, failures, disabledServers };
}
