// Pure helpers for the guided MCP secret/env setup form (docs/mcp.md "Guided env/secret
// setup"). The form lets a user supply or map the secrets a managed-stdio MCP server needs
// without hand-editing config; these helpers turn the value-free requirement view into form
// rows and turn the filled rows back into the POST body. NEVER hold or return a secret value
// beyond the transient form state the user just typed (which the backend stores write-only).

import type {
  ReluxMcpServerSetup,
  ReluxMcpEnvRequirement,
  ReluxMcpEnvSetupMapping,
} from "./api";

// How the user is supplying one env var's secret.
export type EnvSecretMode = "value" | "existing";

// One row of the guided setup form's local state.
export interface EnvSetupRow {
  envVar: string;
  // Whether the requirement is already satisfied (maps to a present secret). A satisfied
  // row is shown for transparency but needs no input.
  satisfied: boolean;
  // The currently-mapped secret name, when any (display only — never the value).
  mappedSecret?: string;
  mode: EnvSecretMode;
  // The inline value the user typed (mode === "value"). Never persisted client-side.
  value: string;
  // The existing secret name the user chose (mode === "existing").
  secretName: string;
}

// Whether a setup view has outstanding work (mirrors McpServerSetup::needs_setup).
export function setupNeedsWork(
  setup: ReluxMcpServerSetup | undefined | null,
): boolean {
  return !!setup && !setup.ready && setup.requirements.length > 0;
}

// A short, value-free status label for one requirement.
export function requirementStatusLabel(req: ReluxMcpEnvRequirement): string {
  if (req.secret_present) return "secret mapped";
  if (req.secret_mapped) return "mapped, secret missing";
  return "needs a secret";
}

// Build the initial form rows from a setup view: one row per requirement, ordered as the
// backend ordered them (source-declared first). Each row defaults to entering a value;
// a row already mapped to a secret prefills "use existing" with that name.
export function rowsFromSetup(setup: ReluxMcpServerSetup): EnvSetupRow[] {
  return setup.requirements.map((req) => ({
    envVar: req.env_var,
    satisfied: req.secret_present,
    mappedSecret: req.secret_name,
    mode: req.secret_mapped ? "existing" : "value",
    value: "",
    secretName: req.secret_name ?? "",
  }));
}

// True when a row carries usable input the user supplied (so it belongs in the POST).
export function rowHasInput(row: EnvSetupRow): boolean {
  return row.mode === "value"
    ? row.value.trim().length > 0
    : row.secretName.trim().length > 0;
}

// Build the env-setup mappings from the rows the user actually filled. A satisfied row the
// user left untouched is skipped (no needless re-write). Trims inputs; the backend is the
// authoritative validator.
export function envSetupMappings(rows: EnvSetupRow[]): ReluxMcpEnvSetupMapping[] {
  const out: ReluxMcpEnvSetupMapping[] = [];
  for (const row of rows) {
    if (!rowHasInput(row)) continue;
    if (row.mode === "value") {
      out.push({ env_var: row.envVar.trim(), value: row.value.trim() });
    } else {
      out.push({ env_var: row.envVar.trim(), secret_name: row.secretName.trim() });
    }
  }
  return out;
}

// The full POST body for reluxMcp.envSetup from the form rows. `expected` is the declared
// env var set (so the recomputed view is complete); `rediscover` re-runs the bounded probe.
export function envSetupBody(
  rows: EnvSetupRow[],
  rediscover: boolean,
): { mappings: ReluxMcpEnvSetupMapping[]; expected_env: string[]; rediscover: boolean } {
  return {
    mappings: envSetupMappings(rows),
    expected_env: rows.map((r) => r.envVar),
    rediscover,
  };
}
