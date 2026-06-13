import { useEffect, useState } from "react";
import {
  reluxMcp,
  reluxSecrets,
  type ReluxMcpServerSetup,
  type ReluxSecretStatus,
  type ReluxMcpPostActivationDiscovery,
} from "../api";
import {
  rowsFromSetup,
  envSetupBody,
  envSetupMappings,
  requirementStatusLabel,
  setupNeedsWork,
  type EnvSetupRow,
} from "../mcpEnvSetup";

// The guided secret/env setup form for a managed-stdio MCP server (docs/mcp.md "Guided
// env/secret setup"). It turns a value-free requirement view ("this needs OPENAI_API_KEY")
// into one row per env var where the user EITHER types a value (stored write-only as a
// secret) OR picks an existing stored secret to reference — then maps them onto the server
// and re-discovers, all through the single governed POST .../env-setup route. It never
// shows a stored secret's value (only its name + redacted status). Exported plain so a
// render test can mount it with a fabricated setup.
// A self-loading section for surfaces that have a registered server id + the source's
// declared env var names but not a setup object yet (the Plugins page after a one-click
// register). Fetches the value-free requirement view, then renders the form (or an honest
// "all mapped" when nothing is outstanding). Never fetches or shows a value.
export function McpEnvSetupSection({
  serverId,
  expected,
}: {
  serverId: string;
  expected: string[];
}) {
  const [setup, setSetup] = useState<ReluxMcpServerSetup | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    reluxMcp
      .envSetupStatus(serverId, expected)
      .then((s) => {
        if (live) setSetup(s);
      })
      .catch((e) => {
        if (live) setErr(e instanceof Error ? e.message : "Could not load the required secrets");
      });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverId]);

  if (err) {
    return (
      <div className="banner err" style={{ fontSize: 11, margin: "6px 0 0" }}>{err}</div>
    );
  }
  if (!setup) {
    return (
      <div className="loading" style={{ fontSize: 11, margin: "6px 0 0" }}>
        Checking required secrets…
      </div>
    );
  }
  if (!setupNeedsWork(setup)) {
    return (
      <p className="banner ok" style={{ fontSize: 11, margin: "6px 0 0" }}>
        All required secrets are mapped. Click <strong>Discover</strong> above to list this
        server's tools through the gate.
      </p>
    );
  }
  return <McpEnvSetupForm serverId={serverId} setup={setup} />;
}

export function McpEnvSetupForm({
  serverId,
  setup,
  onResolved,
}: {
  serverId: string;
  setup: ReluxMcpServerSetup;
  // Called with the recomputed setup after a successful save (so a parent can refresh).
  onResolved?: (next: ReluxMcpServerSetup, discovery?: ReluxMcpPostActivationDiscovery) => void;
}) {
  const [rows, setRows] = useState<EnvSetupRow[]>(() => rowsFromSetup(setup));
  const [secrets, setSecrets] = useState<ReluxSecretStatus[]>([]);
  const [current, setCurrent] = useState<ReluxMcpServerSetup>(setup);
  const [discovery, setDiscovery] = useState<ReluxMcpPostActivationDiscovery | null>(null);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    reluxSecrets
      .list()
      .then((s) => {
        if (live) setSecrets(s);
      })
      .catch(() => {
        /* the dropdown is a convenience; the value mode always works */
      });
    return () => {
      live = false;
    };
  }, []);

  function patch(i: number, next: Partial<EnvSetupRow>) {
    setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...next } : r)));
  }

  const hasInput = envSetupMappings(rows).length > 0;

  async function save() {
    if (saving || !hasInput) return;
    setErr(null);
    setSaving(true);
    try {
      const result = await reluxMcp.envSetup(serverId, envSetupBody(rows, true));
      setCurrent(result.setup);
      setRows(rowsFromSetup(result.setup));
      setDiscovery(result.discovery ?? null);
      onResolved?.(result.setup, result.discovery);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not save the secrets");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="card" style={{ margin: "8px 0 0", padding: 10, background: "transparent" }}>
      <div className="row" style={{ alignItems: "baseline", gap: 8, marginBottom: 6 }}>
        <strong style={{ fontSize: 12 }}>Set up the secrets this server needs</strong>
        <span
          className={`badge ${current.ready ? "done" : "blocked"}`}
          style={{ fontSize: 8 }}
          title={current.ready ? "Every required secret is mapped and present" : "Some required secrets are still missing"}
        >
          {current.ready ? "ready" : `${current.missing.length} missing`}
        </span>
      </div>
      <p className="muted" style={{ margin: "0 0 8px", fontSize: 11 }}>
        Enter a value (stored write-only — never shown again) or map an existing stored
        secret. Relux references the secret by name; the value lives only in the local,
        permission-hardened store and is handed to the server at spawn.
      </p>

      {current.requirements.map((req, i) => {
        const row = rows[i];
        if (!row) return null;
        return (
          <div
            key={req.env_var}
            className="card"
            style={{ margin: "0 0 6px", padding: 8, background: "transparent" }}
          >
            <div className="row wrap" style={{ alignItems: "baseline", gap: 8 }}>
              <span className="mono" style={{ fontSize: 12, fontWeight: 600 }}>{req.env_var}</span>
              {req.required && (
                <span className="badge backlog" style={{ fontSize: 8 }}>required</span>
              )}
              <span
                className={`badge ${req.secret_present ? "done" : "in_progress"}`}
                style={{ fontSize: 8 }}
                title={req.secret_name ? `Mapped to secret "${req.secret_name}"` : "No secret mapped yet"}
              >
                {requirementStatusLabel(req)}
              </span>
              {req.secret_name && (
                <span className="muted mono" style={{ fontSize: 10 }} title="The mapped secret name (never the value)">
                  → {req.secret_name}
                </span>
              )}
            </div>

            <div className="row wrap" style={{ gap: 8, marginTop: 6, alignItems: "center" }}>
              <select
                className="input"
                style={{ maxWidth: 160 }}
                value={row.mode}
                onChange={(e) => patch(i, { mode: e.target.value as EnvSetupRow["mode"] })}
                aria-label={`How to supply ${req.env_var}`}
              >
                <option value="value">Enter a value</option>
                <option value="existing">Use an existing secret</option>
              </select>
              {row.mode === "value" ? (
                <input
                  className="input"
                  type="password"
                  autoComplete="off"
                  placeholder={`value for ${req.env_var}`}
                  value={row.value}
                  onChange={(e) => patch(i, { value: e.target.value })}
                  style={{ flex: 1, minWidth: 160 }}
                  aria-label={`Value for ${req.env_var}`}
                />
              ) : (
                <select
                  className="input"
                  value={row.secretName}
                  onChange={(e) => patch(i, { secretName: e.target.value })}
                  style={{ flex: 1, minWidth: 160 }}
                  aria-label={`Existing secret for ${req.env_var}`}
                >
                  <option value="">— pick a stored secret —</option>
                  {secrets.map((s) => (
                    <option key={s.name} value={s.name}>
                      {s.name}
                      {s.preview ? ` (${s.preview})` : ""}
                    </option>
                  ))}
                </select>
              )}
            </div>
          </div>
        );
      })}

      {err && <div className="banner err" style={{ fontSize: 11, margin: "6px 0 0" }}>{err}</div>}

      <div className="row wrap" style={{ gap: 8, marginTop: 8 }}>
        <button
          className="btn"
          style={{ fontSize: 12, padding: "4px 12px" }}
          disabled={saving || !hasInput}
          onClick={save}
          title="Store + map the secrets, then re-discover the server's tools"
        >
          {saving ? "Saving…" : "Save secrets & discover"}
        </button>
        {!hasInput && (
          <span className="muted" style={{ fontSize: 11, alignSelf: "center" }}>
            Enter a value or pick a secret for at least one variable.
          </span>
        )}
      </div>

      {discovery && (
        <div className="banner" style={{ fontSize: 11, marginTop: 8 }}>
          <span
            className={`badge ${discovery.reachable ? "done" : "blocked"}`}
            style={{ fontSize: 8, marginRight: 6 }}
          >
            {discovery.reachable
              ? `${discovery.tool_count} tool${discovery.tool_count === 1 ? "" : "s"} found`
              : "not reachable yet"}
          </span>
          <span className="muted">{discovery.guidance}</span>
          {!discovery.reachable && discovery.error && (
            <div className="muted mono" style={{ fontSize: 10, marginTop: 4 }}>{discovery.error}</div>
          )}
        </div>
      )}
    </div>
  );
}
