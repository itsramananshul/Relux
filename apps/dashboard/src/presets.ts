// Pure helpers for the crew role-preset selector (no React, unit-testable under
// `node --strip-types` — see apps/dashboard/test/presets.test.ts and the
// dashboard-test-tsx-vs-ts split note).
//
// Section: docs/relix-dashboard-design.md §9.1 (Crew create role presets). A preset is
// an OPERATOR CONVENIENCE: applying one fills the (still editable) role/persona/skills
// fields of the create form. A preset SUGGESTS configuration only — it never grants a
// permission or picks an adapter. The backend is the authority: presets are fetched
// read-only from GET /v1/relux/agent-presets, and creation flows through the normal
// validated path (which grants only the minimal echo tool, preset or not).

// One role preset as returned by the backend. Mirrors the server `AgentPresetRecord`
// (advisory fields ONLY — there is deliberately no permission/adapter field).
export interface AgentPreset {
  id: string;
  label: string;
  summary: string;
  role: string;
  persona: string;
  skills: string[];
}

// The three editable fields a preset fills. Kept separate from the full form state so
// applying a preset can never touch name/id/adapter/status/permissions.
export interface PresetFields {
  role: string;
  persona: string;
  skills: string; // the comma-separated text the skills input shows
}

// True when any of the preset-managed fields already has operator-entered content, so
// the caller can confirm before an apply would overwrite that work. Whitespace-only is
// treated as empty (nothing meaningful to lose).
export function presetFieldsDirty(fields: PresetFields): boolean {
  return (
    fields.role.trim() !== "" ||
    fields.persona.trim() !== "" ||
    fields.skills.trim() !== ""
  );
}

// Expand a preset into the form-field values it fills. The skills array is rendered
// into the comma-separated form the skills input expects. Returns the fields verbatim
// from the (already bounded, backend-authored) preset — applying does not mutate any
// other field, and the result remains fully editable before save.
export function applyPreset(preset: AgentPreset): PresetFields {
  return {
    role: preset.role,
    persona: preset.persona,
    skills: (preset.skills ?? []).join(", "),
  };
}
