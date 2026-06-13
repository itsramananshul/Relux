// Pure, dependency-free helpers for the Prime "tool glue / multi-step ability plan"
// affordance (RELUX_MASTER_PLAN §23, the `execute_code` foundation). The operator pastes
// or edits a STRUCTURED program — a JSON array of `{ plugin, tool, args? }` steps — or
// builds one by clicking known Prime abilities; this module turns that text into the
// `ReluxProposedGlueStep[]` posted to `POST /v1/relux/prime/glue/preview`, failing closed
// on a malformed shape BEFORE any request so the UI never sends a body the backend rejects.
//
// It deliberately validates ONLY the wire shape (an array of steps, each with a non-empty
// plugin + tool, args optional). It does NOT decide readiness or gating — that is the
// kernel's grounding job (`relux_core::ground_tool_glue_plan`), surfaced honestly on the
// returned `ReluxPrimeToolPlanProposal` (an unknown tool comes back `readiness: "unknown"`).
// Mirroring `./toolruntask`, it is kept React-free so `node --test` can pin every branch
// without a DOM, and the component renders whatever this returns and invents nothing.

import type { ReluxProposedGlueStep } from "./api";

export type GlueParseResult =
  | { ok: true; steps: ReluxProposedGlueStep[] }
  | { ok: false; error: string };

// The canonical empty program — what the textarea seeds with and what an empty editor
// round-trips to. A blank/whitespace editor is an error on preview (a program needs a
// step), but the placeholder array keeps the JSON shape obvious.
export const EMPTY_GLUE_STEPS_TEXT = "[]";

// Parse the operator's structured-steps TEXT into the wire steps, failing closed:
//   - blank => "add at least one step";
//   - must be valid JSON, and a JSON ARRAY (not an object / scalar);
//   - a non-empty array, each element an object with a non-empty plugin + tool (trimmed);
//   - `args` is optional — omitted/`null` normalize to `{}` (the kernel default), any other
//     value is forwarded verbatim (the kernel forwards it; an unknown tool still fails closed).
// The 1-based step number is named in every error so the editor can point at the row. The
// returned steps carry trimmed plugin/tool + a defined `args`, ready to POST verbatim.
export function parseGlueSteps(text: string): GlueParseResult {
  const trimmed = text.trim();
  if (!trimmed) {
    return { ok: false, error: "Add at least one step (a JSON array of { plugin, tool, args? })." };
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return { ok: false, error: "Steps must be valid JSON — an array of { plugin, tool, args? }." };
  }
  if (!Array.isArray(parsed)) {
    return { ok: false, error: "Steps must be a JSON ARRAY of { plugin, tool, args? }." };
  }
  if (parsed.length === 0) {
    return { ok: false, error: "Add at least one step." };
  }

  const steps: ReluxProposedGlueStep[] = [];
  for (let i = 0; i < parsed.length; i++) {
    const stepNo = i + 1;
    const raw = parsed[i];
    if (raw == null || typeof raw !== "object" || Array.isArray(raw)) {
      return { ok: false, error: `Step ${stepNo}: must be an object { plugin, tool, args? }.` };
    }
    const rec = raw as Record<string, unknown>;
    const plugin = typeof rec.plugin === "string" ? rec.plugin.trim() : "";
    const tool = typeof rec.tool === "string" ? rec.tool.trim() : "";
    if (!plugin || !tool) {
      return { ok: false, error: `Step ${stepNo}: a step needs a non-empty plugin and tool.` };
    }
    // Omitted/null args is the canonical empty `{}`; any other value is forwarded as-is.
    const args = rec.args == null ? {} : rec.args;
    steps.push({ plugin, tool, args });
  }
  return { ok: true, steps };
}

// Append one known ability (a plugin id + tool name) as a fresh step to the current editor
// TEXT, returning the new pretty-printed JSON. NON-DESTRUCTIVE: a blank editor seeds a
// single-step array; an existing valid array gains the step. If the editor holds non-array
// or unparseable JSON, the text is returned UNCHANGED (the operator's hand-edited content is
// never clobbered — they fix the JSON, and `parseGlueSteps` reports it on preview).
export function appendAbilityStep(text: string, plugin: string, tool: string): string {
  const step: ReluxProposedGlueStep = { plugin, tool, args: {} };
  const trimmed = text.trim();
  let arr: unknown[] = [];
  if (trimmed) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      return text; // invalid JSON — don't clobber the operator's content
    }
    if (!Array.isArray(parsed)) return text; // non-array JSON — same
    arr = parsed;
  }
  return JSON.stringify([...arr, step], null, 2);
}
