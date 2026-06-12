import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import {
  reluxAdapters,
  reluxMcp,
  reluxPluginRuntime,
  reluxPlugins,
  reluxSecrets,
  reluxTools,
  reluxWork,
  type ReluxAdapterStatus,
  type ReluxManagedStdioStatus,
  type ReluxSecretStatus,
  type ReluxManifestTemplate,
  type McpToolClassification,
  type ReluxMcpServer,
  type ReluxMcpToolsResult,
  type ReluxMcpResource,
  type ReluxMcpResourcesResult,
  type ReluxMcpResourceContent,
  type ReluxPlugin,
  type ReluxPluginHints,
  type ReluxPluginRuntime,
  type ReluxToolConfigInput,
  type ReluxToolDescriptor,
  type ReluxToolInvocationResult,
} from "../api";
import { useAsync } from "../components/common";
import {
  adapterStatusBadge,
  canConfigureTools,
  hintKindLabel,
  hintsNextStep,
  installResultSummary,
  managedStdioStatusBadge,
  mcpDraftFromProposal,
  mcpServerStatusBadge,
  pluginCategory,
  pluginKindLabel,
  pluginNextStep,
  pluginStatus,
  toolReadiness,
  validateMcpRegisterDraft,
  visibleTools,
  emptyMcpRegisterDraft,
  mcpRegisterBody,
  type McpRegisterDraft,
  type McpDraftTransport,
  type StatusVariant,
  type ToolReadiness,
} from "../plugins";
import {
  buildToolPickerOptions,
  buildToolRunTaskPayload,
  MAX_TOOL_RUN_STEPS,
  type McpServerDiscovery,
  type ToolRunStep,
} from "../toolruntask";

// Map a derived status variant to the shared badge palette (B&W + semantic
// accent only): ready=green, needs-config=amber, disabled=faint.
const BADGE_CLASS: Record<StatusVariant, string> = {
  ok: "done",
  warn: "in_progress",
  muted: "backlog",
};

// Plugins page (RELUX_MASTER_PLAN section 11.6): the installed-plugin surface for
// the local Relux control plane. It lists what is installed (id, kind, version,
// source, enabled, protected/bundled, description) and drives the durable
// install lifecycle through the `/v1/relux` API: a plus button opens an install
// panel with three sources (GitHub URL, ZIP upload, local folder path); a Remove
// button clears a non-protected plugin. Everything refreshes after an install or
// remove so the table never drifts from the backend.

type Source = "github" | "zip" | "dir";

export function Plugins() {
  const { data, loading, error, reload } = useAsync<ReluxPlugin[]>(
    () => reluxPlugins.list(),
    [],
  );
  const plugins = data ?? [];
  const [open, setOpen] = useState(false);

  // Live adapter runtime state, from the SAME probe the Crew adapters section
  // uses. Adapter rows show this inline so an operator sees whether Claude/Codex/
  // Local Prime is actually available — not just the plugin record's enabled flag.
  // A failed/loading probe is surfaced honestly (never faked as ready) per row.
  const adaptersAsync = useAsync<ReluxAdapterStatus[]>(
    () => reluxAdapters.list(),
    [],
  );
  const adapterByPlugin = new Map(
    (adaptersAsync.data ?? []).map((a) => [a.plugin_id, a] as const),
  );
  const adaptersLoading = adaptersAsync.loading && adaptersAsync.data == null;

  function reloadAll() {
    reload();
    adaptersAsync.reload();
  }

  return (
    <div className="grid">
      <div className="card">
        <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
          <h3 style={{ margin: 0 }}>Installed plugins</h3>
          <div className="spacer" style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={() => reloadAll()} disabled={loading}>
            {loading ? "Loading..." : "Refresh"}
          </button>
          <button
            className="btn sm"
            style={{ marginLeft: 8 }}
            onClick={() => setOpen((v) => !v)}
            aria-expanded={open}
            title="Install a plugin"
          >
            {open ? "Close" : "+ Install"}
          </button>
        </div>
        <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12 }}>
          Plugins installed in the local Relux control plane. They stay installed
          across restarts until removed. Bundled fixtures are protected and cannot
          be removed.
        </p>

        {open && (
          <InstallPanel
            onClose={() => setOpen(false)}
            onInstalled={() => {
              // Refresh the table so the new row appears, but KEEP the panel open
              // so the install result summary (what was discovered / generated and
              // the next step) stays visible until the operator dismisses it.
              reload();
            }}
          />
        )}

        {error ? (
          <div className="banner err" style={{ fontSize: 12 }}>
            Could not reach the Relux plugin API ({error}). Start it with{" "}
            <span className="mono">cargo run -p relux-kernel -- serve</span> (listens on{" "}
            <span className="mono">127.0.0.1:19891</span>).
          </div>
        ) : loading && data == null ? (
          <div className="loading">Loading plugins...</div>
        ) : plugins.length === 0 ? (
          <div className="empty">No plugins installed yet. Use + Install to add one.</div>
        ) : (
          <div className="table-scroll">
            <table className="table">
              <thead>
                <tr>
                  <th>Plugin</th>
                  <th>Kind</th>
                  <th>Version</th>
                  <th>Source</th>
                  <th>Status</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {plugins.map((p) => (
                  <PluginRow
                    key={p.id}
                    plugin={p}
                    onChanged={reloadAll}
                    adapterRuntime={adapterByPlugin.get(p.id)}
                    adapterRuntimeLoading={adaptersLoading}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <ToolsSection />

      <SecretsSection />

      <McpSection />
    </div>
  );
}

// Secrets & environment (docs/mcp.md "Local secrets & environment"; RELUX_MASTER_PLAN
// §17.5). A local, file-backed secret store: an operator adds named secrets (API keys /
// tokens) here, and a managed-stdio MCP server's env maps an env-var to a secret by
// NAME. The value is write-only — it is stored hardened to owner-only permissions and
// NEVER returned by the API (the listing shows only a tail preview like "…cdef"). A
// referenced secret is resolved into the child env at spawn; a missing one is an honest
// failed status that names the secret, never a value.
function SecretsSection() {
  const { data, loading, error, reload } = useAsync<ReluxSecretStatus[]>(
    () => reluxSecrets.list(),
    [],
  );
  const secrets = data ?? [];
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  async function addSecret() {
    const n = name.trim();
    if (!n) {
      setBanner({ kind: "err", msg: "Secret name is required." });
      return;
    }
    if (!/^[A-Za-z0-9._-]+$/.test(n)) {
      setBanner({
        kind: "err",
        msg: "Secret name may use only letters, digits, '.', '-' or '_'.",
      });
      return;
    }
    if (!value) {
      setBanner({ kind: "err", msg: "Secret value is required." });
      return;
    }
    setBusy(true);
    setBanner(null);
    try {
      await reluxSecrets.set(n, value);
      // Clear the value immediately — it is write-only and never shown again.
      setValue("");
      setName("");
      setBanner({ kind: "ok", msg: `Secret "${n}" stored (value is write-only — never shown again).` });
      reload();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Could not store secret" });
    } finally {
      setBusy(false);
    }
  }

  async function removeSecret(n: string) {
    setBusy(true);
    setBanner(null);
    try {
      await reluxSecrets.remove(n);
      reload();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Could not delete secret" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <h3 style={{ margin: 0 }}>Secrets &amp; environment</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={() => reload()} disabled={loading}>
          {loading ? "Loading..." : "Refresh"}
        </button>
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12 }}>
        Local API keys / tokens for managed-stdio MCP servers (and future adapters).
        Values are <strong>write-only</strong> — stored locally, hardened to owner-only
        file permissions, and <strong>never returned</strong> (only a redacted preview
        is shown). Reference a secret from a server's <strong>Environment</strong> field
        by NAME (e.g. <span className="mono">OPENAI_API_KEY=my_openai_key</span>).
      </p>
      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12, marginBottom: 8 }}>
          {banner.msg}
        </div>
      )}
      <div className="row wrap" style={{ gap: 8, alignItems: "flex-end", marginBottom: 12 }}>
        <label className="field" style={{ margin: 0, minWidth: 180 }}>
          <span style={{ fontSize: 12 }}>Name</span>
          <input
            className="input"
            value={name}
            placeholder="my_openai_key"
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <label className="field" style={{ margin: 0, minWidth: 220, flex: 1 }}>
          <span style={{ fontSize: 12 }}>Value (write-only)</span>
          <input
            className="input"
            type="password"
            value={value}
            placeholder="paste the secret value"
            autoComplete="off"
            onChange={(e) => setValue(e.target.value)}
          />
        </label>
        <button className="btn sm" disabled={busy} onClick={() => void addSecret()}>
          {busy ? "Saving..." : "Add secret"}
        </button>
      </div>
      {error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Could not load secrets ({error}).
        </div>
      ) : secrets.length === 0 ? (
        <p className="muted" style={{ fontSize: 12 }}>
          No secrets stored. Add one above, then reference it from a managed-stdio
          server's Environment field.
        </p>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Value</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {secrets.map((s) => (
                <tr key={s.name}>
                  <td className="mono">{s.name}</td>
                  <td className="mono muted">{s.preview ?? "—"}</td>
                  <td style={{ textAlign: "right" }}>
                    <button
                      className="btn ghost sm"
                      disabled={busy}
                      onClick={() => void removeSecret(s.name)}
                      title="Delete this secret"
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// MCP servers section (RELUX_MASTER_PLAN §8.2/§18; HERMES_OPENCLAW_DEEP_AUDIT §9;
// docs/mcp.md). The first relux-layer Model Context Protocol surface: register
// operator-run, loopback-ONLY MCP servers and run a live `tools/list` discovery
// against them. Honest by construction — Relux dials no remote host and spawns no
// command. MCP tool INVOCATION is now wired: a discovered tool classifies, invokes,
// or requests a per-call approval through the SAME kernel gates a plugin tool uses
// (permission, risk/approval, per-call approval, persistent grant, audit) against
// `plugin_id = "mcp:<server>"`. Resources are a read-only context surface
// (`resources/list` + `resources/read`); listing or reading one mutates nothing.
function McpSection() {
  const { data, loading, error, reload } = useAsync<ReluxMcpServer[]>(
    () => reluxMcp.list(),
    [],
  );
  const servers = data ?? [];
  const [open, setOpen] = useState(false);

  return (
    <div className="card">
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <h3 style={{ margin: 0 }}>MCP servers</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={() => reload()} disabled={loading}>
          {loading ? "Loading..." : "Refresh"}
        </button>
        <button
          className="btn sm"
          style={{ marginLeft: 8 }}
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          title="Register an MCP server"
        >
          {open ? "Close" : "+ Add server"}
        </button>
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12 }}>
        Model Context Protocol servers you run locally — a loopback HTTP endpoint
        (<span className="mono">http://127.0.0.1:&lt;port&gt;</span>,{" "}
        <span className="mono">http://localhost:&lt;port&gt;</span>, or{" "}
        <span className="mono">http://[::1]:&lt;port&gt;</span>) or a{" "}
        <strong>governed managed-stdio command</strong> (argv only, never a shell;
        spawned only on Discover/invoke, never on registration). Relux dials no
        remote host and runs no downloaded code on import. <strong>Discovery is live</strong>{" "}
        (a real <span className="mono">tools/list</span>), and a discovered tool is{" "}
        <strong>callable</strong> through the normal permission, risk/approval, and
        audit gates (against <span className="mono">plugin_id mcp:&lt;server&gt;</span>)
        — an unclassified tool stays gated until you set its risk. Resources are a{" "}
        <strong>read-only</strong> context surface (<span className="mono">resources/list</span>{" "}
        + <span className="mono">resources/read</span>); reading one mutates nothing.
      </p>

      {open && (
        <AddMcpServerForm
          onClose={() => setOpen(false)}
          onAdded={() => reload()}
        />
      )}

      {error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Could not reach the Relux MCP API ({error}). Start it with{" "}
          <span className="mono">cargo run -p relux-kernel -- serve</span>.
        </div>
      ) : loading && data == null ? (
        <div className="loading">Loading MCP servers...</div>
      ) : servers.length === 0 ? (
        <div className="empty">
          No MCP servers registered. Use “+ Add server” to register a loopback MCP
          server you run locally.
        </div>
      ) : (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th>Server</th>
                <th>Transport</th>
                <th>Status</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {servers.map((s) => (
                <McpServerRow key={s.id} server={s} onChanged={reload} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function McpServerRow({
  server,
  onChanged,
}: {
  server: ReluxMcpServer;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [resourcesOpen, setResourcesOpen] = useState(false);
  const status = mcpServerStatusBadge(server);

  async function remove() {
    setBusy(true);
    setErr(null);
    try {
      await reluxMcp.remove(server.id);
      onChanged();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Remove failed");
      setBusy(false);
    }
  }

  return (
    <>
      <tr>
        <td>
          <strong>{server.id}</strong>
          {server.description && (
            <div className="muted" style={{ fontSize: 12, marginTop: 2, maxWidth: 380 }}>
              {server.description}
            </div>
          )}
          {err && (
            <div className="banner err" style={{ fontSize: 11, marginTop: 6, marginBottom: 0 }}>
              {err}
            </div>
          )}
        </td>
        <td className="mono muted" style={{ fontSize: 11, wordBreak: "break-all", maxWidth: 240 }}>
          <span className="badge" style={{ marginRight: 6 }}>
            {server.transport === "managed_stdio" ? "stdio" : "http"}
          </span>
          {server.transport_display ??
            (server.transport === "managed_stdio"
              ? [server.command, ...(server.args ?? [])].filter(Boolean).join(" ")
              : server.endpoint)}
        </td>
        <td>
          <span className={"badge " + BADGE_CLASS[status.variant]} title={status.title}>
            {status.label}
          </span>
        </td>
        <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
          <button
            className="btn ghost sm"
            onClick={() => setToolsOpen((v) => !v)}
            aria-expanded={toolsOpen}
            title={
              server.transport === "managed_stdio"
                ? "Spawn the command and run a live tools/list discovery"
                : "Run a live tools/list discovery against this server"
            }
          >
            {toolsOpen ? "Close" : "Discover"}
          </button>
          <button
            className="btn ghost sm"
            style={{ marginLeft: 6 }}
            onClick={() => setResourcesOpen((v) => !v)}
            aria-expanded={resourcesOpen}
            title={
              server.transport === "managed_stdio"
                ? "Spawn the command (or reuse the running process) and run a live resources/list (read-only context)"
                : "Run a live resources/list against this server (read-only context)"
            }
          >
            {resourcesOpen ? "Close" : "Resources"}
          </button>
          <button
            className="btn ghost sm"
            style={{ marginLeft: 6 }}
            disabled={busy}
            onClick={() => void remove()}
          >
            {busy ? "..." : "Remove"}
          </button>
        </td>
      </tr>
      {server.transport === "managed_stdio" && (
        <tr>
          <td colSpan={4} style={{ background: "transparent", paddingTop: 0 }}>
            <ManagedStdioControls server={server} />
          </td>
        </tr>
      )}
      {toolsOpen && (
        <tr>
          <td colSpan={4} style={{ background: "transparent" }}>
            <McpDiscoverPanel server={server} />
          </td>
        </tr>
      )}
      {resourcesOpen && (
        <tr>
          <td colSpan={4} style={{ background: "transparent" }}>
            <McpResourcesPanel server={server} />
          </td>
        </tr>
      )}
    </>
  );
}

// The managed-stdio process lifecycle controls: the live process status (state, pid,
// start time, last error, redacted log tail) plus Start / Stop / Restart. A
// managed-stdio server is registered (config) independently of whether its process is
// running; starting one keeps a single initialized process warm so Discover and tool
// calls reuse it (no per-call spawn). Nothing is auto-started — the process spawns
// only on an explicit Start (or, when stopped, a per-operation Discover/invoke).
function ManagedStdioControls({ server }: { server: ReluxMcpServer }) {
  const { data, loading, error, reload } = useAsync<ReluxManagedStdioStatus>(
    () => reluxMcp.status(server.id),
    [server.id],
  );
  const [busy, setBusy] = useState<null | "start" | "stop" | "restart">(null);
  const [actionErr, setActionErr] = useState<string | null>(null);

  async function act(kind: "start" | "stop" | "restart") {
    setBusy(kind);
    setActionErr(null);
    try {
      if (kind === "start") await reluxMcp.start(server.id);
      else if (kind === "stop") await reluxMcp.stop(server.id);
      else await reluxMcp.restart(server.id);
      reload();
    } catch (e) {
      setActionErr(e instanceof Error ? e.message : `${kind} failed`);
    } finally {
      setBusy(null);
    }
  }

  const badge = data ? managedStdioStatusBadge(data) : null;
  const running = data?.state === "running";
  const startedAt = data?.started_at_ms
    ? new Date(data.started_at_ms).toLocaleString()
    : null;

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div className="row" style={{ alignItems: "center", gap: 8, flexWrap: "wrap" }}>
        <strong style={{ fontSize: 13 }}>Process</strong>
        {loading && <span className="muted" style={{ fontSize: 12 }}>checking…</span>}
        {badge && (
          <span className={"badge " + BADGE_CLASS[badge.variant]} title={badge.title}>
            {badge.label}
          </span>
        )}
        {data?.tools_count != null && (
          <span className="muted" style={{ fontSize: 11 }}>
            {data.tools_count} tool{data.tools_count === 1 ? "" : "s"} discovered
          </span>
        )}
        {startedAt && (
          <span className="muted" style={{ fontSize: 11 }}>started {startedAt}</span>
        )}
        <div className="spacer" style={{ flex: 1 }} />
        <button
          className="btn ghost sm"
          disabled={busy !== null}
          onClick={() => void act("start")}
          title="Spawn (or replace) the managed process and run its initialize handshake. Discover/calls then reuse it."
        >
          {busy === "start" ? "…" : "Start"}
        </button>
        <button
          className="btn ghost sm"
          style={{ marginLeft: 6 }}
          disabled={busy !== null || !running}
          onClick={() => void act("stop")}
          title="Kill + reap the managed process."
        >
          {busy === "stop" ? "…" : "Stop"}
        </button>
        <button
          className="btn ghost sm"
          style={{ marginLeft: 6 }}
          disabled={busy !== null}
          onClick={() => void act("restart")}
          title="Stop then start the managed process."
        >
          {busy === "restart" ? "…" : "Restart"}
        </button>
      </div>
      <p className="muted" style={{ marginTop: 8, marginBottom: 0, fontSize: 11 }}>
        Starting keeps one initialized process warm (argv only — never a shell; no env,
        no cwd). When stopped, Discover/invoke still work via a one-shot spawn-per-call.
      </p>
      {error && (
        <div className="banner err" style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
          Could not read status ({error}).
        </div>
      )}
      {actionErr && (
        <div className="banner err" style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
          {actionErr}
        </div>
      )}
      {data?.last_error && (
        <div className="banner err" style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
          Last error: <span className="mono">{data.last_error}</span>
        </div>
      )}
      {data?.log_tail && data.log_tail.length > 0 && (
        <pre
          className="mono"
          style={{
            marginTop: 8,
            marginBottom: 0,
            fontSize: 11,
            maxHeight: 140,
            overflow: "auto",
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
          }}
        >
          {data.log_tail.join("\n")}
        </pre>
      )}
    </div>
  );
}

// The live discovery panel: runs `tools/list` against the loopback MCP server and
// lists the discovered tools. Honest about every outcome — a disabled server, an
// unreachable server, or a server that isn't speaking MCP each shows its real
// reason, never a faked tool list. Each discovered tool shows its honest readiness
// (`needs approval` until classified, `ready` once classified low-risk + auto-
// approve) and can be classified, invoked, or sent through the per-call approval
// flow — all through the SAME kernel gates a plugin tool uses.
function McpDiscoverPanel({ server }: { server: ReluxMcpServer }) {
  const { data, loading, error, reload } = useAsync<ReluxMcpToolsResult>(
    () => reluxMcp.tools(server.id),
    [server.id, server.enabled],
  );
  const tools = data?.tools ?? [];

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>Discovered tools (live)</strong>
        <div className="spacer" style={{ flex: 1 }} />
        {!loading && !error && (
          <span className="badge done">{tools.length} discovered</span>
        )}
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 11 }}>
        Discovery runs a real <span className="mono">tools/list</span> against the
        loopback server. An unclassified tool is <strong>gated</strong> (needs
        approval) until you set its risk — every call still routes through the
        permission, approval/grant, and audit gates, against{" "}
        <span className="mono">plugin_id mcp:{server.id}</span>.
      </p>
      {error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Discovery failed ({error}). The server may be down, disabled, or not
          speaking MCP over this endpoint. Relux does not fake an empty list.
        </div>
      ) : loading ? (
        <div className="loading">Running tools/list…</div>
      ) : tools.length === 0 ? (
        <div className="empty" style={{ fontSize: 12 }}>
          The server returned no tools.
        </div>
      ) : (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th>Tool</th>
                <th>Risk</th>
                <th>Status</th>
                <th style={{ textAlign: "right" }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {tools.map((t) => (
                <McpToolRow
                  key={t.tool_name}
                  serverId={server.id}
                  tool={t}
                  onClassified={reload}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// The live resources panel: runs `resources/list` against the MCP server (loopback
// HTTP or managed-stdio command, dispatched server-side on the registered transport)
// and lists the read-only resources (files/records/docs) it advertises. Resources
// are inert context — listing or reading one performs NO action and mutates nothing,
// so there is no classification/approval gate here (unlike tools). Honest about every
// outcome — a disabled, unreachable, or non-MCP server shows its real reason, never a
// faked empty list. Each resource can be previewed (a read-only `resources/read`)
// inline; the returned text is sanitized + secret-redacted + bounded server-side.
function McpResourcesPanel({ server }: { server: ReluxMcpServer }) {
  const { data, loading, error } = useAsync<ReluxMcpResourcesResult>(
    () => reluxMcp.resources(server.id),
    [server.id, server.enabled],
  );
  const resources = data?.resources ?? [];

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>Resources (read-only context)</strong>
        <div className="spacer" style={{ flex: 1 }} />
        {!loading && !error && (
          <span className="badge done">{resources.length} listed</span>
        )}
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 11 }}>
        A real <span className="mono">resources/list</span> against the{" "}
        {server.transport === "managed_stdio" ? "managed-stdio command" : "loopback server"}.
        Resources are <strong>read-only context</strong> — reading one
        performs no action. A preview runs <span className="mono">resources/read</span>;
        the body is sanitized, secret-redacted, and bounded server-side.
      </p>
      {error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Resource listing failed ({error}). The server may be down, disabled, or
          not exposing resources over this endpoint. Relux does not fake a list.
        </div>
      ) : loading ? (
        <div className="loading">Running resources/list…</div>
      ) : resources.length === 0 ? (
        <div className="empty" style={{ fontSize: 12 }}>
          The server advertises no resources.
        </div>
      ) : (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th>Resource</th>
                <th>Type</th>
                <th style={{ textAlign: "right" }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {resources.map((r) => (
                <McpResourceRow key={r.uri} serverId={server.id} resource={r} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// One resource row: name/uri/description + a read-only Preview that fetches the
// shaped, secret-redacted body inline. Never mutates; never shows raw bytes.
function McpResourceRow({
  serverId,
  resource,
}: {
  serverId: string;
  resource: ReluxMcpResource;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [content, setContent] = useState<ReluxMcpResourceContent | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function preview() {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    if (content) return;
    setBusy(true);
    setErr(null);
    try {
      setContent(await reluxMcp.readResource(serverId, resource.uri));
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Read failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <tr>
        <td>
          <strong style={{ fontSize: 12 }}>{resource.title || resource.name || resource.uri}</strong>
          <div className="mono muted" style={{ fontSize: 11, wordBreak: "break-all", maxWidth: 360 }}>
            {resource.uri}
          </div>
          {resource.description && (
            <div className="muted" style={{ fontSize: 11, marginTop: 2, maxWidth: 360 }}>
              {resource.description}
            </div>
          )}
        </td>
        <td className="mono muted" style={{ fontSize: 11 }}>
          {resource.mime_type || "—"}
        </td>
        <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
          <button className="btn ghost sm" onClick={() => void preview()} aria-expanded={open}>
            {open ? "Hide" : "Preview"}
          </button>
        </td>
      </tr>
      {open && (
        <tr>
          <td colSpan={3} style={{ background: "transparent" }}>
            {busy ? (
              <div className="loading">Reading resource…</div>
            ) : err ? (
              <div className="banner err" style={{ fontSize: 12 }}>
                Read failed ({err}). Relux does not fake a body.
              </div>
            ) : content ? (
              <div className="card" style={{ padding: 10, margin: "4px 0" }}>
                {content.binary && (
                  <div className="muted" style={{ fontSize: 11, marginBottom: 4 }}>
                    Includes binary content (summarized, not shown).
                  </div>
                )}
                <pre
                  className="mono"
                  style={{
                    fontSize: 11,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                    maxHeight: 280,
                    overflow: "auto",
                    margin: 0,
                  }}
                >
                  {content.text || "(empty)"}
                </pre>
              </div>
            ) : null}
          </td>
        </tr>
      )}
    </>
  );
}

// One discovered MCP tool row: its honest readiness (the same `toolReadiness`
// classifier a plugin tool uses), plus three real actions — Classify (set its
// risk/approval), Invoke (when `ready`), or Why not? (the honest refusal panel,
// which itself offers the per-call approval flow for a `needs_approval` tool).
// Nothing is faked: a gated tool cannot be invoked directly, exactly as the kernel
// enforces.
function McpToolRow({
  serverId,
  tool,
  onClassified,
}: {
  serverId: string;
  tool: ReluxToolDescriptor;
  onClassified: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [classifyOpen, setClassifyOpen] = useState(false);
  const readiness = toolReadiness(tool);
  const ready = readiness.runnable;

  return (
    <>
      <tr>
        <td>
          <strong>{tool.tool_name}</strong>
          <div className="mono muted" style={{ fontSize: 11 }}>{tool.permission}</div>
          {tool.description && (
            <div className="muted" style={{ fontSize: 12, marginTop: 2, maxWidth: 420 }}>
              {tool.description}
            </div>
          )}
        </td>
        <td className="muted" style={{ fontSize: 12 }}>{tool.risk}</td>
        <td>
          <span className={`badge ${BADGE_CLASS[readiness.tone]}`} title={readiness.reason}>
            {readiness.label}
          </span>
        </td>
        <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
          <button
            className="btn ghost sm"
            onClick={() => {
              setClassifyOpen((v) => !v);
              setOpen(false);
            }}
            aria-expanded={classifyOpen}
          >
            {classifyOpen ? "Close" : "Classify"}
          </button>{" "}
          <button
            className="btn ghost sm"
            onClick={() => {
              setOpen((v) => !v);
              setClassifyOpen(false);
            }}
            aria-expanded={open}
          >
            {open ? "Close" : ready ? "Invoke" : "Why not?"}
          </button>
        </td>
      </tr>
      {(open || classifyOpen) && (
        <tr>
          <td colSpan={4} style={{ background: "transparent" }}>
            {classifyOpen && (
              <McpClassifyForm
                serverId={serverId}
                tool={tool}
                onDone={() => {
                  setClassifyOpen(false);
                  onClassified();
                }}
              />
            )}
            {open &&
              (ready ? (
                <InvokeTool tool={tool} />
              ) : (
                <ToolNotRunnable tool={tool} readiness={readiness} />
              ))}
          </td>
        </tr>
      )}
    </>
  );
}

// The MCP tool classification form: set the tool's risk + approval so it becomes
// directly runnable (low + auto-approve) or stays gated behind approval. This is
// the operator action that turns a discovered-but-gated MCP tool into a callable
// one — the same risk/approval model a plugin tool's manifest declares.
function McpClassifyForm({
  serverId,
  tool,
  onDone,
}: {
  serverId: string;
  tool: ReluxToolDescriptor;
  onDone: () => void;
}) {
  const [risk, setRisk] = useState<McpToolClassification["risk"]>(
    (tool.risk as McpToolClassification["risk"]) ?? "medium",
  );
  const [approval, setApproval] = useState<"never" | "required">(
    tool.executable === "ready" ? "never" : "required",
  );
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function save(clear: boolean) {
    setBusy(true);
    setErr(null);
    try {
      if (clear) {
        await reluxMcp.clearClassification(serverId, tool.tool_name);
      } else {
        await reluxMcp.setClassification(serverId, tool.tool_name, { risk, approval });
      }
      onDone();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Classification failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 8 }}>
        Classify <span className="mono">{tool.tool_name}</span>
      </div>
      <p className="muted" style={{ fontSize: 11, marginTop: 0, marginBottom: 10 }}>
        A discovered MCP tool's real risk is unknown, so it stays gated (needs
        approval) until you set it. <strong>Low + Auto-approve</strong> makes it
        directly callable; anything else keeps it behind the per-call approval flow.
      </p>
      <div className="row wrap" style={{ gap: 10 }}>
        <label className="field" style={{ margin: 0 }}>
          <span style={{ fontSize: 12 }}>Risk</span>
          <select
            className="input"
            value={risk}
            onChange={(e) => setRisk(e.target.value as McpToolClassification["risk"])}
          >
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
            <option value="critical">critical</option>
          </select>
        </label>
        <label className="field" style={{ margin: 0 }}>
          <span style={{ fontSize: 12 }}>Approval</span>
          <select
            className="input"
            value={approval}
            onChange={(e) => setApproval(e.target.value as "never" | "required")}
          >
            <option value="required">required (gated)</option>
            <option value="never">never (auto-approve)</option>
          </select>
        </label>
      </div>
      {approval === "never" && risk !== "low" && (
        <div className="muted" style={{ fontSize: 11, marginTop: 8 }}>
          Note: a non-low risk with auto-approve is still directly runnable — set it
          deliberately.
        </div>
      )}
      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn" disabled={busy} onClick={() => void save(false)}>
          {busy ? "Saving…" : "Save classification"}
        </button>
        <button className="btn ghost" disabled={busy} onClick={() => void save(true)}>
          Reset to gated default
        </button>
      </div>
      {err && <div className="banner err" style={{ fontSize: 12, marginTop: 10 }}>{err}</div>}
    </div>
  );
}

// The "Add an MCP server" form. Supports two transports:
//   - loopback HTTP endpoint (validated server-side as loopback-only), or
//   - a governed managed-stdio command + args (validated argv-only — no shell
//     metacharacters, bounded, no bypass/danger flag).
// No secrets are accepted (env is not stored). Registering a stdio server NEVER runs
// the command — it is spawned only on a later operator-driven Discover / gated call.
function AddMcpServerForm({
  onClose,
  onAdded,
  initial,
  title,
  advisory,
}: {
  onClose: () => void;
  onAdded: () => void;
  // A pre-filled review draft (from a detected MCP hint proposal). The form is
  // identical to the manual "Add an MCP server" form — it never auto-registers and
  // never runs the source; the operator still confirms every field.
  initial?: McpRegisterDraft;
  title?: string;
  // An optional advisory block (e.g. the detected stdio command + honest notes)
  // rendered above the fields. Display-only.
  advisory?: ReactNode;
}) {
  const seed = initial ?? emptyMcpRegisterDraft();
  const [id, setId] = useState(seed.id);
  const [transport, setTransport] = useState<McpDraftTransport>(seed.transport);
  const [endpoint, setEndpoint] = useState(seed.endpoint);
  const [command, setCommand] = useState(seed.command);
  const [argsText, setArgsText] = useState(seed.argsText);
  const [envText, setEnvText] = useState(seed.envText);
  const [cwd, setCwd] = useState(seed.cwd);
  const [description, setDescription] = useState(seed.description);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  const draft: McpRegisterDraft = {
    id,
    transport,
    endpoint,
    command,
    argsText,
    envText,
    cwd,
    description,
  };

  async function submit() {
    setBusy(true);
    setBanner(null);
    // Pre-check with the SAME fail-closed rules the kernel enforces, so the form
    // never sends a request the registry would reject.
    const problem = validateMcpRegisterDraft(draft);
    if (problem) {
      setBanner({ kind: "err", msg: problem });
      setBusy(false);
      return;
    }
    try {
      await reluxMcp.register(mcpRegisterBody(draft));
      setBanner({
        kind: "ok",
        msg: "MCP server registered. Find it under MCP servers above and click Discover to list its tools through the gate.",
      });
      if (!initial) {
        setId("");
        setEndpoint("");
        setCommand("");
        setArgsText("");
        setEnvText("");
        setCwd("");
        setDescription("");
      }
      onAdded();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Register failed" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card" style={{ marginBottom: 12, padding: 12 }}>
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>{title ?? "Add an MCP server"}</strong>
      </div>
      {advisory}
      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 12 }}>Server id</span>
        <input
          className="input"
          value={id}
          placeholder="fs-helper"
          onChange={(e) => setId(e.target.value)}
        />
        <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          Letters, digits, <span className="mono">.</span>, <span className="mono">-</span>,{" "}
          <span className="mono">_</span> only. Used as the{" "}
          <span className="mono">mcp:&lt;id&gt;</span> namespace for discovered tools.
        </span>
      </label>
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 12 }}>Transport</span>
        <select
          className="input"
          value={transport}
          onChange={(e) => setTransport(e.target.value as McpDraftTransport)}
        >
          <option value="http_loopback">Loopback HTTP endpoint</option>
          <option value="managed_stdio">Managed stdio command</option>
        </select>
        <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          Managed stdio runs a local command (argv only, never a shell). It is spawned
          only when you Discover or invoke a tool — never on registration.
        </span>
      </label>
      {transport === "http_loopback" ? (
        <label className="field" style={{ margin: "8px 0 0" }}>
          <span style={{ fontSize: 12 }}>Loopback endpoint</span>
          <input
            className="input"
            value={endpoint}
            placeholder="http://127.0.0.1:8000/mcp"
            onChange={(e) => setEndpoint(e.target.value)}
          />
          <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
            Loopback only. A remote or <span className="mono">https</span> endpoint is
            refused. Relux POSTs JSON-RPC (<span className="mono">initialize</span>,{" "}
            <span className="mono">tools/list</span>) here.
          </span>
        </label>
      ) : (
        <>
          <label className="field" style={{ margin: "8px 0 0" }}>
            <span style={{ fontSize: 12 }}>Command</span>
            <input
              className="input"
              value={command}
              placeholder="npx"
              onChange={(e) => setCommand(e.target.value)}
            />
            <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
              One program token (argv only, never a shell). No shell metacharacters; a
              full path is allowed. Bypass/danger flags are refused.
            </span>
          </label>
          <label className="field" style={{ margin: "8px 0 0" }}>
            <span style={{ fontSize: 12 }}>Args (one per line)</span>
            <textarea
              className="input"
              rows={3}
              value={argsText}
              placeholder={"-y\n@modelcontextprotocol/server-github"}
              onChange={(e) => setArgsText(e.target.value)}
            />
            <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
              Each line is one argv element (never split) — an arg may contain spaces
              or JSON. The child inherits the parent environment plus the env below.
            </span>
          </label>
          <label className="field" style={{ margin: "8px 0 0" }}>
            <span style={{ fontSize: 12 }}>Environment (one ENV_VAR=secret_name per line)</span>
            <textarea
              className="input"
              rows={2}
              value={envText}
              placeholder={"OPENAI_API_KEY=openrouter_api_key"}
              onChange={(e) => setEnvText(e.target.value)}
            />
            <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
              The right-hand side is a <strong>secret name</strong> — a reference to a
              stored secret, <strong>never</strong> a value. Add secrets under{" "}
              <strong>Secrets &amp; environment</strong> above; they resolve into the
              child env at spawn and are never echoed back.
            </span>
          </label>
          <label className="field" style={{ margin: "8px 0 0" }}>
            <span style={{ fontSize: 12 }}>Working directory (optional)</span>
            <input
              className="input"
              value={cwd}
              placeholder="workspace-a"
              onChange={(e) => setCwd(e.target.value)}
            />
            <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
              Relative to (or inside) the safe MCP workspace root. No{" "}
              <span className="mono">..</span> traversal; the path must exist at spawn.
            </span>
          </label>
        </>
      )}
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 12 }}>Description (optional)</span>
        <input
          className="input"
          value={description}
          placeholder="What this server provides."
          onChange={(e) => setDescription(e.target.value)}
        />
      </label>
      <div className="row wrap" style={{ gap: 8, marginTop: 12 }}>
        <button className="btn" disabled={busy} onClick={() => void submit()}>
          {busy ? "Registering..." : "Register server"}
        </button>
        <button className="btn ghost" disabled={busy} onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}

// Tools section (RELUX_MASTER_PLAN section 7.4; Relux spec section 20.2 Tools view):
// the honest tool-invocation surface. It lists installed plugin tools with their
// executable status - `ready`, `installed (runtime not implemented yet)`, or
// `missing permission` - and lets the operator invoke a ready tool with JSON
// input, showing the structured output or a clear error. Nothing is faked: a tool
// with no kernel runtime is shown as such, never hidden, never pretend-run.
function ToolsSection() {
  const { data, loading, error, reload } = useAsync<ReluxToolDescriptor[]>(
    () => reluxTools.list(),
    [],
  );
  const tools = data ?? [];
  // By default the list shows only runnable (ready) tools, so a metadata-only or
  // unconfigured plugin never looks usable. A toggle reveals the rest with their
  // honest non-runnable status; nothing is permanently hidden or faked.
  const [showAll, setShowAll] = useState(false);
  const { shown, hiddenCount } = visibleTools(tools, showAll);

  return (
    <div className="card">
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <h3 style={{ margin: 0 }}>Tools</h3>
        <div className="spacer" style={{ flex: 1 }} />
        {hiddenCount > 0 || showAll ? (
          <button
            className="btn ghost sm"
            onClick={() => setShowAll((v) => !v)}
            title="Reveal installed-but-not-runnable tools with their honest status"
          >
            {showAll
              ? "Show runnable only"
              : `Show ${hiddenCount} non-runnable`}
          </button>
        ) : null}
        <button className="btn ghost sm" style={{ marginLeft: 8 }} onClick={() => reload()} disabled={loading}>
          {loading ? "Loading..." : "Refresh"}
        </button>
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12 }}>
        Callable capabilities from installed plugins. By default only runnable
        tools are listed; an installed tool with no kernel runtime is not hidden
        permanently — reveal it above to see its honest status. Invocations are
        permission-checked and audited.
      </p>

      <CreateToolRunTask tools={tools} />

      {error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Could not reach the Relux tools API ({error}). Start it with{" "}
          <span className="mono">cargo run -p relux-kernel -- serve</span>.
        </div>
      ) : loading && data == null ? (
        <div className="loading">Loading tools...</div>
      ) : tools.length === 0 ? (
        <div className="empty">No tools available from installed plugins.</div>
      ) : shown.length === 0 ? (
        <div className="empty">
          No runnable tools yet. {hiddenCount} installed tool
          {hiddenCount === 1 ? " is" : "s are"} not runnable.{" "}
          <button
            className="btn ghost sm"
            style={{ marginLeft: 4 }}
            onClick={() => setShowAll(true)}
          >
            Show {hiddenCount === 1 ? "it" : "them"}
          </button>
        </div>
      ) : (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th>Tool</th>
                <th>Risk</th>
                <th>Status</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {shown.map((t) => (
                <ToolRow key={`${t.plugin_id}/${t.tool_name}`} tool={t} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// Compact operator form to create a "tool-run task": a task whose run drives one
// gated tool call (a `tool_call` directive) or a bounded sequence of them (a
// `tool_plan`, ≤5 steps, run in order, stopping on the first failure). It posts the
// SAME `POST /v1/relux/tasks` the Work page uses, with the optional directive body
// the kernel already accepts — no new backend. (`docs/mcp.md` "Run-driven MCP tool
// call" + "Run-driven multi-tool plan".)
//
// HONEST about approval: a step whose tool is gated (`needs_approval`) can be put in
// a plan, but the RUN will block/fail on that step unless a standing allow-always
// grant exists — the form labels such steps and never pretends the run will
// auto-approve. The task is created and assigned to Prime; run it from Work with
// "Run (Assigned)".
function CreateToolRunTask({ tools }: { tools: ReluxToolDescriptor[] }) {
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState("");
  const [steps, setSteps] = useState<ToolRunStep[]>([{ plugin: "", tool: "", argsText: "" }]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [created, setCreated] = useState<string | null>(null);

  // Live MCP discovery for the picker: when the form is open, list the registered
  // MCP servers and run a real `tools/list` against each ENABLED one, so the picker
  // can offer MCP-discovered tools alongside the installed plugin tools. Gated on
  // `open` so merely loading the page never dials the operator's loopback servers;
  // each open re-discovers (fresh truth, never a cached/faked list). A disabled
  // server is recorded (not discovered); a failed discovery is recorded as failed,
  // and both surface as honest notes below — neither silently vanishes.
  const mcp = useAsync<McpServerDiscovery[]>(async () => {
    if (!open) return [];
    const servers = await reluxMcp.list();
    return Promise.all(
      servers.map(async (s): Promise<McpServerDiscovery> => {
        if (!s.enabled) return { serverId: s.id, enabled: false };
        try {
          const r = await reluxMcp.tools(s.id);
          return { serverId: s.id, enabled: true, tools: r.tools ?? [] };
        } catch (e) {
          return {
            serverId: s.id,
            enabled: true,
            failed: true,
            error: e instanceof Error ? e.message : String(e),
          };
        }
      }),
    );
  }, [open]);

  // The tool options the operator picks from: every installed plugin tool PLUS every
  // tool a live discovery surfaced from an enabled MCP server (each keyed by the
  // stable plugin id `mcp:<server>`). A gated (non-"ready") tool is offered too — the
  // run will simply need an approval grant — so the dropdown label flags it honestly
  // rather than hiding it. The merge + honest notes (failed/disabled servers) come
  // from the React-free `buildToolPickerOptions`; gating reuses `toolReadiness`.
  const picker = buildToolPickerOptions(
    tools,
    mcp.data ?? [],
    (t) => !toolReadiness(t).runnable,
  );
  const options = picker.options;

  // Does any chosen step reference a gated tool? Drives the honest approval caveat.
  const anyGated = steps.some((s) => {
    if (!s.plugin || !s.tool) return false;
    const opt = options.find((o) => o.plugin === s.plugin && o.tool === s.tool);
    return opt ? opt.gated : false;
  });

  function setStep(i: number, patch: Partial<ToolRunStep>) {
    setSteps((prev) => prev.map((s, idx) => (idx === i ? { ...s, ...patch } : s)));
  }
  function addStep() {
    setSteps((prev) => (prev.length >= MAX_TOOL_RUN_STEPS ? prev : [...prev, { plugin: "", tool: "", argsText: "" }]));
  }
  function removeStep(i: number) {
    setSteps((prev) => (prev.length <= 1 ? prev : prev.filter((_, idx) => idx !== i)));
  }

  async function submit() {
    setErr(null);
    setCreated(null);
    const built = buildToolRunTaskPayload(title, steps);
    if (!built.ok) {
      setErr(built.error);
      return;
    }
    const { title: builtTitle, ...directive } = built.payload;
    setBusy(true);
    try {
      const task = await reluxWork.createTask(builtTitle, directive);
      setCreated(task.id);
      // Reset the form for the next one; keep it open so the operator sees success.
      setTitle("");
      setSteps([{ plugin: "", tool: "", argsText: "" }]);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not create the task.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card" style={{ margin: "0 0 12px", padding: 12 }}>
      <div className="row" style={{ alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>Create a tool-run task</strong>
        <div className="spacer" style={{ flex: 1 }} />
        <button
          className="btn ghost sm"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          title="Create a task whose run drives one or more gated tool calls"
        >
          {open ? "Close" : "New"}
        </button>
      </div>
      {open && (
        <div style={{ marginTop: 10 }}>
          <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 12 }}>
            One step creates a single <span className="mono">tool_call</span>; two-to-{MAX_TOOL_RUN_STEPS} steps
            create a <span className="mono">tool_plan</span> that runs in order and stops on the first
            failure. The task is assigned to Prime — run it from{" "}
            <Link to="/work">Work</Link> with “Run (Assigned)”.
          </p>

          <p className="muted" style={{ marginTop: -4, marginBottom: 10, fontSize: 11 }}>
            The tool picker lists installed plugin tools and tools discovered live
            from your enabled MCP servers (keyed{" "}
            <span className="mono">mcp:&lt;server&gt;</span>). A tool that needs
            approval is labelled, never hidden.
          </p>

          {mcp.loading && (
            <p className="muted" style={{ marginTop: -4, marginBottom: 10, fontSize: 11 }}>
              Discovering MCP tools…
            </p>
          )}
          {mcp.error && (
            <div className="banner err" style={{ fontSize: 12, marginBottom: 10 }}>
              Could not list MCP servers ({mcp.error}); only installed plugin tools
              are shown. Start the kernel with{" "}
              <span className="mono">cargo run -p relux-kernel -- serve</span>.
            </div>
          )}
          {picker.failures.length > 0 && (
            <div className="banner err" style={{ fontSize: 12, marginBottom: 10 }}>
              Live discovery failed for {picker.failures.length} enabled MCP server
              {picker.failures.length === 1 ? "" : "s"} (
              <span className="mono">
                {picker.failures.map((f) => f.serverId).join(", ")}
              </span>
              ); their tools are not listed. The server may be down, stopped
              mid-flight, or not speaking MCP — Relux does not fake a tool list.
            </div>
          )}
          {picker.disabledServers.length > 0 && (
            <p className="muted" style={{ marginTop: -4, marginBottom: 10, fontSize: 11 }}>
              {picker.disabledServers.length} disabled MCP server
              {picker.disabledServers.length === 1 ? "" : "s"} not included (
              <span className="mono">{picker.disabledServers.join(", ")}</span>) —
              enable one in the “MCP servers” section above to discover its tools.
            </p>
          )}

          <label className="field" style={{ margin: 0 }}>
            <span style={{ fontSize: 12 }}>Task title</span>
            <input
              className="input"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. search the docs index"
            />
          </label>

          {steps.map((step, i) => (
            <div
              key={i}
              className="card"
              style={{ margin: "10px 0 0", padding: 10, background: "transparent" }}
            >
              <div className="row" style={{ alignItems: "center", marginBottom: 6 }}>
                <span className="muted" style={{ fontSize: 12 }}>
                  Step {i + 1}
                  {steps.length > 1 ? ` of ${steps.length}` : ""}
                </span>
                <div className="spacer" style={{ flex: 1 }} />
                {steps.length > 1 && (
                  <button className="btn ghost sm" onClick={() => removeStep(i)} title="Remove this step">
                    Remove
                  </button>
                )}
              </div>
              <label className="field" style={{ margin: 0 }}>
                <span style={{ fontSize: 12 }}>Tool</span>
                <select
                  className="input"
                  value={step.plugin && step.tool ? `${step.plugin} ${step.tool}` : ""}
                  onChange={(e) => {
                    const [plugin, tool] = e.target.value.split(" ");
                    setStep(i, { plugin: plugin ?? "", tool: tool ?? "" });
                  }}
                >
                  <option value="">
                    {options.length === 0 ? "No tools discovered yet" : "Choose a tool…"}
                  </option>
                  {options.map((o) => (
                    <option key={o.key} value={o.key}>
                      {o.label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field" style={{ margin: "8px 0 0" }}>
                <span style={{ fontSize: 12 }}>JSON arguments (blank = {"{}"})</span>
                <textarea
                  className="input"
                  style={{ minHeight: 64, fontFamily: "monospace", fontSize: 12 }}
                  value={step.argsText}
                  onChange={(e) => setStep(i, { argsText: e.target.value })}
                  placeholder='{ "query": "files" }'
                />
              </label>
            </div>
          ))}

          <div className="row wrap" style={{ gap: 8, marginTop: 10, alignItems: "center" }}>
            <button
              className="btn ghost sm"
              onClick={addStep}
              disabled={steps.length >= MAX_TOOL_RUN_STEPS}
              title={
                steps.length >= MAX_TOOL_RUN_STEPS
                  ? `A plan may have at most ${MAX_TOOL_RUN_STEPS} steps`
                  : "Add another step (creates a tool_plan)"
              }
            >
              Add step
            </button>
            <span className="muted" style={{ fontSize: 11 }}>
              {steps.length}/{MAX_TOOL_RUN_STEPS} steps
            </span>
            <div className="spacer" style={{ flex: 1 }} />
            <button className="btn" disabled={busy} onClick={() => void submit()}>
              {busy ? "Creating…" : "Create task"}
            </button>
          </div>

          {anyGated && (
            <div className="banner" style={{ fontSize: 12, marginTop: 10 }}>
              A chosen tool needs approval. The plan can be created, but the run will
              <strong> block or fail</strong> on that step unless a standing
              allow-always grant exists — Relux never auto-approves it.
            </div>
          )}
          {err && (
            <div className="banner err" style={{ fontSize: 12, marginTop: 10 }}>
              {err}
            </div>
          )}
          {created && (
            <div className="banner" style={{ fontSize: 12, marginTop: 10 }}>
              Created task <span className="mono">{created}</span>, assigned to Prime. Run it
              from <Link to="/work">Work</Link> with “Run (Assigned)”.
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ToolRow({ tool }: { tool: ReluxToolDescriptor }) {
  const [open, setOpen] = useState(false);
  const readiness = toolReadiness(tool);
  const ready = readiness.runnable;

  return (
    <>
      <tr>
        <td>
          <div>
            <strong>{tool.tool_name}</strong>
          </div>
          <div className="mono muted" style={{ fontSize: 11 }}>{tool.plugin_id}</div>
          {tool.description && (
            <div className="muted" style={{ fontSize: 12, marginTop: 2, maxWidth: 420 }}>
              {tool.description}
            </div>
          )}
          <div className="mono muted" style={{ fontSize: 11, marginTop: 2 }}>
            {tool.permission}
          </div>
        </td>
        <td className="muted" style={{ fontSize: 12 }}>{tool.risk}</td>
        <td>
          <span className={`badge ${BADGE_CLASS[readiness.tone]}`} title={readiness.reason}>
            {readiness.label}
          </span>
        </td>
        <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
          {/* Every row has a real, non-blank action: a ready tool toggles its
              invoke form; a non-ready tool toggles an honest "Why not?" panel
              that states the refusal/disabled reason and the next step — never a
              dead-end "not callable" with nothing behind it. */}
          <button
            className="btn ghost sm"
            onClick={() => setOpen((v) => !v)}
            aria-expanded={open}
          >
            {open ? "Close" : ready ? "Invoke" : "Why not?"}
          </button>
        </td>
      </tr>
      {open && (
        <tr>
          <td colSpan={4} style={{ background: "transparent" }}>
            {ready ? (
              <InvokeTool tool={tool} />
            ) : (
              <ToolNotRunnable tool={tool} readiness={readiness} />
            )}
          </td>
        </tr>
      )}
    </>
  );
}

// The honest, non-blank panel for a tool the kernel will NOT run directly. It
// states WHY (the same refusal/disabled reason the kernel enforces in
// `call_tool`/`invoke_tool`) and the concrete next step — so an operator is never
// left at a dead-end or a blank page, and the UI never pretends a gated tool ran.
// For a `needs_approval` tool it also offers a real Request-approval form (the
// per-call approval flow), never a pretend run.
function ToolNotRunnable({
  tool,
  readiness,
}: {
  tool: ReluxToolDescriptor;
  readiness: ToolReadiness;
}) {
  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div style={{ fontSize: 12, marginBottom: readiness.nextStep ? 6 : 0 }}>
        <strong>Not runnable: </strong>
        {readiness.reason}
      </div>
      {readiness.nextStep && (
        <div className="muted" style={{ fontSize: 12 }}>
          <strong>Next step: </strong>
          {readiness.nextStep}
        </div>
      )}
      {readiness.canRequestApproval && <RequestApproval tool={tool} />}
    </div>
  );
}

// The per-call approval request form for a gated (`needs_approval`) tool. The
// operator supplies the exact JSON arguments for ONE invocation; the kernel binds
// the approval to that snapshot (tool id + args hash + requester). Nothing runs
// here — it creates a Pending approval an operator decides on the Approvals page,
// where the approved call can be executed once. This never bypasses the gate.
function RequestApproval({ tool }: { tool: ReluxToolDescriptor }) {
  const [input, setInput] = useState('{\n  "example": "value"\n}');
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function request() {
    setBusy(true);
    setErr(null);
    setCreated(null);
    let parsed: unknown = {};
    const trimmed = input.trim();
    if (trimmed) {
      try {
        parsed = JSON.parse(trimmed);
      } catch {
        setErr("Arguments must be valid JSON (or empty).");
        setBusy(false);
        return;
      }
    }
    try {
      const appr = await reluxTools.requestApproval({
        plugin_id: tool.plugin_id,
        tool_name: tool.tool_name,
        input: parsed,
      });
      setCreated(appr.id);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Request failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{ marginTop: 10, borderTop: "1px solid var(--line, #333)", paddingTop: 10 }}>
      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>
        Request a per-call approval
      </div>
      <label className="field" style={{ margin: 0 }}>
        <span style={{ fontSize: 12 }}>JSON arguments for this one invocation (as Prime)</span>
        <textarea
          className="input"
          style={{ minHeight: 80, fontFamily: "monospace", fontSize: 12 }}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
      </label>
      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn" disabled={busy} onClick={() => void request()}>
          {busy ? "Requesting..." : "Request approval"}
        </button>
      </div>
      {err && <div className="banner err" style={{ fontSize: 12, marginTop: 10 }}>{err}</div>}
      {created && (
        <div className="banner" style={{ fontSize: 12, marginTop: 10 }}>
          Approval <span className="mono">{created}</span> created and pending. Decide and
          execute it on the <Link to="/approvals">Approvals</Link> page.
        </div>
      )}
    </div>
  );
}

function InvokeTool({ tool }: { tool: ReluxToolDescriptor }) {
  const [input, setInput] = useState('{\n  "message": "hello relux"\n}');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ReluxToolInvocationResult | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function run() {
    setBusy(true);
    setErr(null);
    setResult(null);
    let parsed: unknown = {};
    const trimmed = input.trim();
    if (trimmed) {
      try {
        parsed = JSON.parse(trimmed);
      } catch {
        setErr("Input must be valid JSON (or empty).");
        setBusy(false);
        return;
      }
    }
    try {
      const res = await reluxTools.invoke({
        plugin_id: tool.plugin_id,
        tool_name: tool.tool_name,
        input: parsed,
      });
      setResult(res);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Invocation failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <label className="field" style={{ margin: 0 }}>
        <span style={{ fontSize: 12 }}>JSON input (invoked as Prime)</span>
        <textarea
          className="input"
          style={{ minHeight: 90, fontFamily: "monospace", fontSize: 12 }}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
      </label>
      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn" disabled={busy} onClick={() => void run()}>
          {busy ? "Invoking..." : "Invoke"}
        </button>
      </div>
      {err && (
        <div className="banner err" style={{ fontSize: 12, marginTop: 10 }}>{err}</div>
      )}
      {result && (
        <div style={{ marginTop: 10 }}>
          <div className="muted" style={{ fontSize: 11, marginBottom: 4 }}>
            Output (permission {result.permission}, agent {result.agent_id})
          </div>
          <pre
            className="mono"
            style={{
              fontSize: 12,
              background: "var(--panel, #111)",
              padding: 10,
              borderRadius: 6,
              overflowX: "auto",
              margin: 0,
            }}
          >
            {JSON.stringify(result.output, null, 2)}
          </pre>
        </div>
      )}
    </div>
  );
}

function PluginRow({
  plugin,
  onChanged,
  adapterRuntime,
  adapterRuntimeLoading,
}: {
  plugin: ReluxPlugin;
  onChanged: () => void;
  adapterRuntime: ReluxAdapterStatus | undefined;
  adapterRuntimeLoading: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [runtimeOpen, setRuntimeOpen] = useState(false);
  const [manifestOpen, setManifestOpen] = useState(false);

  const status = pluginStatus(plugin);
  const next = pluginNextStep(plugin);
  // A 0-tool generated wrapper: still shows the inline "metadata only" banner.
  const isWrapper = next.kind === "add-manifest";
  // Whether the operator can add/edit tool definitions in-UI, and whether a
  // loopback runtime is worth configuring yet (only once tools exist).
  const configurable = canConfigureTools(plugin);
  const hasTools = (plugin.tool_count ?? 0) > 0;
  // Adapter rows show LIVE runtime state (available/disabled/missing-binary/…),
  // not the static plugin-record enabled flag. Non-adapter rows keep pluginStatus.
  const isAdapter = pluginCategory(plugin) === "adapter";

  async function remove() {
    setBusy(true);
    setErr(null);
    try {
      await reluxPlugins.remove(plugin.id);
      onChanged();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Remove failed");
      setBusy(false);
    }
  }

  return (
    <>
      <tr>
        <td>
          <div>
            <strong>{plugin.name || plugin.id}</strong>
          </div>
          <div className="mono muted" style={{ fontSize: 11 }}>{plugin.id}</div>
          {plugin.description && (
            <div className="muted" style={{ fontSize: 12, marginTop: 2, maxWidth: 380 }}>
              {plugin.description}
            </div>
          )}
          {isWrapper && (
            // Actionable, not just a warning: state the dead-end honestly (a
            // runtime alone runs nothing) and put the "Set up" next step inline.
            <div
              className="banner info banner-action"
              style={{ fontSize: 11, marginTop: 6 }}
            >
              <span>
                Installed as metadata only — Relux generated a wrapper because the
                source had no <span className="mono">relux-plugin.json</span>. It
                declares no tools, so a runtime alone runs nothing. Next: add a tool
                definition below.
              </span>
              <button
                className="banner-cta"
                onClick={() => setManifestOpen((v) => !v)}
                aria-expanded={manifestOpen}
              >
                {manifestOpen ? "Hide setup" : "Configure"}
              </button>
            </div>
          )}
          {err && (
            <div className="banner err" style={{ fontSize: 11, marginTop: 6, marginBottom: 0 }}>
              {err}
            </div>
          )}
        </td>
        <td className="muted" style={{ fontSize: 12 }}>
          <div>{pluginKindLabel(plugin)}</div>
          {next.kind === "configure-runtime" && (
            <div className="mono muted" style={{ fontSize: 11 }}>
              {plugin.tool_count ?? 0} tool{(plugin.tool_count ?? 0) === 1 ? "" : "s"}
            </div>
          )}
        </td>
        <td className="mono" style={{ fontSize: 12 }}>v{plugin.version}</td>
        <td className="muted" style={{ fontSize: 12, maxWidth: 240 }}>
          <div>{plugin.source_kind}</div>
          <div className="mono muted" style={{ fontSize: 11, wordBreak: "break-all" }}>
            {plugin.source_label}
          </div>
        </td>
        <td>
          {isAdapter ? (
            adapterRuntimeLoading ? (
              <span className="badge backlog" title="Reading live adapter runtime status…">
                checking…
              </span>
            ) : (
              // `adapterRuntime` is undefined when the probe errored or no row
              // matched; adapterStatusBadge renders that as an honest muted
              // "status unavailable" — never a faked "ready".
              (() => {
                const live = adapterStatusBadge(adapterRuntime);
                return (
                  <span className={"badge " + BADGE_CLASS[live.variant]} title={live.title}>
                    {live.label}
                  </span>
                );
              })()
            )
          ) : (
            <span className={"badge " + BADGE_CLASS[status.variant]} title={status.title}>
              {status.label}
            </span>
          )}
          {plugin.protected && (
            <span className="badge" style={{ marginLeft: 6 }} title="Bundled fixture; cannot be removed">
              protected
            </span>
          )}
        </td>
        <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
          {next.kind === "configure-adapter" ? (
            // Adapters — including the bundled, protected Claude/Codex CLIs — always
            // expose a real Configure path (to the Crew page) so they never read as
            // mysterious "locked" rows with no way to use them. A protected adapter
            // is locked against REMOVAL only; it omits the Remove button.
            <>
              <Link className="btn ghost sm" to="/crew" title={next.detail}>
                Configure
              </Link>
              {!plugin.protected && (
                <button
                  className="btn ghost sm"
                  style={{ marginLeft: 6 }}
                  disabled={busy}
                  onClick={() => void remove()}
                >
                  {busy ? "..." : "Remove"}
                </button>
              )}
            </>
          ) : plugin.protected ? (
            <span className="muted" style={{ fontSize: 11 }} title="Bundled plugins are locked">
              locked
            </span>
          ) : (
            <>
              {configurable && (
                <button
                  className="btn ghost sm"
                  onClick={() => setManifestOpen((v) => !v)}
                  aria-expanded={manifestOpen}
                  title="Add or edit tool definitions for this plugin"
                >
                  {manifestOpen ? "Close" : "Configure"}
                </button>
              )}
              {hasTools && (
                <button
                  className="btn ghost sm"
                  style={{ marginLeft: configurable ? 6 : 0 }}
                  onClick={() => setRuntimeOpen((v) => !v)}
                  aria-expanded={runtimeOpen}
                  title="Configure an HTTP loopback runtime so the tools can run"
                >
                  {runtimeOpen ? "Close" : "Runtime"}
                </button>
              )}
              <button
                className="btn ghost sm"
                style={{ marginLeft: 6 }}
                disabled={busy}
                onClick={() => void remove()}
              >
                {busy ? "..." : "Remove"}
              </button>
            </>
          )}
        </td>
      </tr>
      {manifestOpen && !plugin.protected && (
        <tr>
          <td colSpan={6} style={{ background: "transparent" }}>
            <ManifestPanel plugin={plugin} onChanged={onChanged} />
          </td>
        </tr>
      )}
      {runtimeOpen && !plugin.protected && (
        <tr>
          <td colSpan={6} style={{ background: "transparent" }}>
            <RuntimePanel plugin={plugin} />
          </td>
        </tr>
      )}
    </>
  );
}

// Tool configuration panel for a user-installed ToolSet / metadata-only wrapper
// (RELUX_MASTER_PLAN §7.4 Plugin Kernel Layer, §8.2 ToolSet Plugins). A generated
// wrapper declares NO tools, so a loopback runtime alone surfaces nothing — the
// honest way to make it useful is to ADD a tool definition. This panel lets the
// operator do that in-UI: it lists the configured tools (with remove), an "Add a
// tool" form, and an Advanced collapsible with the full hand-edit manifest
// template. The kernel derives the permission and the risk-driven approval; Relux
// never infers tools or runs downloaded code.
function ManifestPanel({
  plugin,
  onChanged,
}: {
  plugin: ReluxPlugin;
  onChanged: () => void;
}) {
  const toolsAsync = useAsync<ReluxToolDescriptor[]>(() => reluxTools.list(), [plugin.id]);
  const myTools = (toolsAsync.data ?? []).filter((t) => t.plugin_id === plugin.id);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  function reloadAfterChange() {
    toolsAsync.reload();
    // Refresh the parent table so the tool count + status track the manifest.
    onChanged();
  }

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>Configure tools</strong>
        <div className="spacer" style={{ flex: 1 }} />
        <span className={"badge " + (myTools.length > 0 ? "done" : "in_progress")}>
          {myTools.length} tool{myTools.length === 1 ? "" : "s"}
        </span>
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 11 }}>
        Add the tools this plugin exposes. Relux never infers tools from downloaded
        code and never runs it — a tool runs only through an HTTP loopback server
        you run locally (set that up with <strong>Runtime</strong> once a tool
        exists). A low-risk tool can be auto-approved; a higher-risk tool always
        requires approval and stays non-runnable until you lower its risk.
      </p>

      <DetectedHints plugin={plugin} />

      <ConfiguredToolsList
        plugin={plugin}
        tools={myTools}
        loading={toolsAsync.loading && toolsAsync.data == null}
        onChanged={reloadAfterChange}
      />

      <AddToolForm plugin={plugin} onAdded={reloadAfterChange} />

      <div style={{ marginTop: 12 }}>
        <button
          className="btn ghost sm"
          onClick={() => setAdvancedOpen((v) => !v)}
          aria-expanded={advancedOpen}
          title="Hand-edit a full relux-plugin.json instead (advanced)"
        >
          {advancedOpen ? "Hide advanced" : "Advanced: hand-edit a full manifest"}
        </button>
        {advancedOpen && <ManifestTemplate plugin={plugin} />}
      </div>
    </div>
  );
}

// The list of tools already configured on this plugin, each with a Remove button.
// Only operator-removable (non-bundled) tools are shown here with a control; the
// list is empty for a fresh wrapper.
function ConfiguredToolsList({
  plugin,
  tools,
  loading,
  onChanged,
}: {
  plugin: ReluxPlugin;
  tools: ReluxToolDescriptor[];
  loading: boolean;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function remove(toolName: string) {
    setBusy(toolName);
    setErr(null);
    try {
      await reluxPlugins.removeTool(plugin.id, toolName);
      onChanged();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Remove failed");
    } finally {
      setBusy(null);
    }
  }

  if (loading) return <div className="loading">Loading tools...</div>;
  if (tools.length === 0) {
    return (
      <div className="empty" style={{ fontSize: 12, marginBottom: 8 }}>
        No tools configured yet. Add one below.
      </div>
    );
  }

  return (
    <div style={{ marginBottom: 8 }}>
      {err && (
        <div className="banner err" style={{ fontSize: 12 }}>{err}</div>
      )}
      <div className="table-scroll">
        <table className="table">
          <tbody>
            {tools.map((t) => (
              <tr key={t.tool_name}>
                <td>
                  <strong>{t.tool_name}</strong>
                  <div className="mono muted" style={{ fontSize: 11 }}>{t.permission}</div>
                  {t.description && (
                    <div className="muted" style={{ fontSize: 12, marginTop: 2, maxWidth: 380 }}>
                      {t.description}
                    </div>
                  )}
                </td>
                <td className="muted" style={{ fontSize: 12 }}>{t.risk}</td>
                <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
                  <button
                    className="btn ghost sm"
                    disabled={busy === t.tool_name}
                    onClick={() => void remove(t.tool_name)}
                  >
                    {busy === t.tool_name ? "..." : "Remove"}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

type RiskChoice = NonNullable<ReluxToolConfigInput["risk"]>;

// The "Add a tool" form. Minimal, validated fields only: name (required),
// description, risk, an auto-approve toggle (low risk only), and a per-call
// timeout. The kernel derives the permission (`tool:<id>:<verb>`) and the approval
// requirement from the risk — this form never sends a raw permission.
function AddToolForm({
  plugin,
  onAdded,
}: {
  plugin: ReluxPlugin;
  onAdded: () => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [risk, setRisk] = useState<RiskChoice>("low");
  const [autoApprove, setAutoApprove] = useState(false);
  const [timeout, setTimeoutSecs] = useState("");
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  const lowRisk = risk === "low";

  async function submit() {
    setBusy(true);
    setBanner(null);
    if (!name.trim()) {
      setBanner({ kind: "err", msg: "Tool name is required." });
      setBusy(false);
      return;
    }
    const body: ReluxToolConfigInput = { name: name.trim(), risk };
    if (description.trim()) body.description = description.trim();
    // auto_approve only matters for low risk; the server ignores it otherwise.
    if (lowRisk) body.auto_approve = autoApprove;
    if (timeout.trim()) {
      const n = Number(timeout.trim());
      if (!Number.isFinite(n) || n <= 0) {
        setBanner({ kind: "err", msg: "Timeout must be a positive number of seconds." });
        setBusy(false);
        return;
      }
      body.timeout_secs = Math.floor(n);
    }
    try {
      await reluxPlugins.configureTool(plugin.id, body);
      setBanner({
        kind: "ok",
        msg: lowRisk && autoApprove
          ? "Tool added. Enable a loopback Runtime to make it runnable."
          : "Tool added. It requires approval (higher risk or auto-approve off), so it stays non-runnable until you lower its risk.",
      });
      setName("");
      setDescription("");
      setTimeoutSecs("");
      onAdded();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Add failed" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card" style={{ padding: 12, marginTop: 4 }}>
      <strong style={{ fontSize: 12 }}>Add a tool</strong>
      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12, marginTop: 8 }}>
          {banner.msg}
        </div>
      )}
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 12 }}>Tool name</span>
        <input
          className="input"
          value={name}
          placeholder="report.fetch"
          onChange={(e) => setName(e.target.value)}
        />
        <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          A dotted id like <span className="mono">report.fetch</span>. The permission{" "}
          <span className="mono">tool:{plugin.id}:&lt;verb&gt;</span> is derived from it.
        </span>
      </label>
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 12 }}>Description (optional)</span>
        <input
          className="input"
          value={description}
          placeholder="What this tool does."
          onChange={(e) => setDescription(e.target.value)}
        />
      </label>
      <div className="row wrap" style={{ gap: 12, marginTop: 8, alignItems: "flex-end" }}>
        <label className="field" style={{ margin: 0 }}>
          <span style={{ fontSize: 12 }}>Risk</span>
          <select
            className="input"
            value={risk}
            onChange={(e) => setRisk(e.target.value as RiskChoice)}
          >
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
            <option value="critical">critical</option>
          </select>
        </label>
        <label className="field" style={{ margin: 0 }}>
          <span style={{ fontSize: 12 }}>Timeout (s, optional)</span>
          <input
            className="input"
            value={timeout}
            placeholder="5"
            inputMode="numeric"
            onChange={(e) => setTimeoutSecs(e.target.value)}
          />
        </label>
      </div>
      <label className="row" style={{ gap: 8, marginTop: 10, alignItems: "center", fontSize: 12 }}>
        <input
          type="checkbox"
          checked={lowRisk && autoApprove}
          disabled={!lowRisk}
          onChange={(e) => setAutoApprove(e.target.checked)}
        />
        <span className={lowRisk ? "" : "muted"}>
          Auto-approve (low risk only) — make it runnable once a loopback runtime is
          enabled. Higher-risk tools always require approval.
        </span>
      </label>
      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn" disabled={busy} onClick={() => void submit()}>
          {busy ? "Adding..." : "Add tool"}
        </button>
      </div>
    </div>
  );
}

// Read-only "what's in this source" panel. When an arbitrary repo/zip is imported
// without a relux-plugin.json, Relux scaffolds a metadata-only wrapper that runs
// nothing — but the operator still needs to know what the source IS to wire it up.
// This fetches safe, never-executed hints (a possible MCP server, an npm/python
// package, an entrypoint, scripts) and an advisory next step. It explicitly states
// these are hints only: Relux never turns one into a runnable tool and never runs
// the source. (RELUX_MASTER_PLAN §7.4 Plugin Kernel Layer, §8.)
function DetectedHints({ plugin }: { plugin: ReluxPlugin }) {
  const { data, loading, error } = useAsync<ReluxPluginHints>(
    () => reluxPlugins.hints(plugin.id),
    [plugin.id],
  );
  // The "Register MCP server…" review form is opened on demand; never auto-shown,
  // never auto-registered.
  const [registering, setRegistering] = useState(false);
  const [registered, setRegistered] = useState(false);

  if (loading && !data) {
    return <div className="loading" style={{ fontSize: 12 }}>Inspecting source…</div>;
  }
  if (error) {
    // Honest, non-blocking: hints are advisory, so a probe failure is informational.
    return (
      <div className="muted" style={{ fontSize: 11, marginBottom: 10 }}>
        Could not inspect the source ({error}).
      </div>
    );
  }
  if (!data) return null;

  const nextStep = hintsNextStep(data.hints);

  return (
    <div
      className="card"
      style={{ margin: "0 0 12px", padding: 10, background: "transparent" }}
    >
      <div className="row" style={{ alignItems: "center", marginBottom: 6 }}>
        <strong style={{ fontSize: 12 }}>Detected in source</strong>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="badge backlog" title="Read-only detection; nothing is executed">
          hints only
        </span>
      </div>
      {!data.scanned ? (
        <p className="muted" style={{ margin: 0, fontSize: 11 }}>
          Nothing to inspect — this plugin’s source is not in the local plugins
          directory.
        </p>
      ) : data.hints.length === 0 ? (
        <p className="muted" style={{ margin: 0, fontSize: 11 }}>
          No runnable signals detected. Add a tool definition below to make anything
          runnable — Relux never infers tools or runs downloaded code.
        </p>
      ) : (
        <>
          <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 11 }}>
            What Relux found in the imported source. These are read-only hints —
            Relux never runs any of this and never turns a hint into a tool. Use
            them to decide how to wire the plugin up.
          </p>
          <ul style={{ margin: "0 0 8px", paddingLeft: 0, listStyle: "none" }}>
            {data.hints.map((h, i) => (
              <li
                key={`${h.kind}-${i}`}
                className="row"
                style={{ alignItems: "baseline", gap: 8, marginBottom: 4 }}
              >
                <span className="badge" style={{ flexShrink: 0 }}>
                  {hintKindLabel(h.kind)}
                </span>
                <span className="mono muted" style={{ fontSize: 11, wordBreak: "break-all" }}>
                  {h.detail}
                </span>
              </li>
            ))}
          </ul>
          {nextStep && (
            <p className="banner info" style={{ fontSize: 11, margin: 0 }}>
              {nextStep}
            </p>
          )}
          {data.mcp_proposal && (
            <div style={{ marginTop: 10 }}>
              {registered ? (
                <p className="banner ok" style={{ fontSize: 11, margin: 0 }}>
                  MCP server registered. Open the <strong>MCP servers</strong>{" "}
                  section above and click <strong>Discover</strong> to list its
                  tools through the gate — discovery dials your loopback server only
                  when you ask.
                </p>
              ) : registering ? (
                <AddMcpServerForm
                  title="Register MCP server"
                  initial={mcpDraftFromProposal(data.mcp_proposal)}
                  advisory={<McpProposalAdvisory proposal={data.mcp_proposal} />}
                  onClose={() => setRegistering(false)}
                  onAdded={() => {
                    setRegistering(false);
                    setRegistered(true);
                  }}
                />
              ) : (
                <button
                  className="btn sm"
                  onClick={() => setRegistering(true)}
                  title="Open a review form pre-filled from what was detected; nothing is registered or run until you confirm"
                >
                  Register MCP server…
                </button>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

// The advisory block shown above the pre-filled MCP registration form: the honest
// notes from detection, plus any detected stdio command — INFORMATIONAL ONLY. Relux
// never runs the command and never uses it as the endpoint; it is shown so the
// operator knows what to start themselves as a loopback server.
function McpProposalAdvisory({
  proposal,
}: {
  proposal: NonNullable<ReluxPluginHints["mcp_proposal"]>;
}) {
  const command = proposal.detected_command;
  const args = proposal.detected_args ?? [];
  return (
    <div style={{ marginBottom: 8 }}>
      <p className="muted" style={{ marginTop: 0, marginBottom: 6, fontSize: 11 }}>
        Pre-filled from the detected source. Relux never runs the source — review and
        confirm, then it registers through the same loopback-only MCP registry and
        gates as any other server.
      </p>
      {command && (
        <div className="banner warn" style={{ fontSize: 11, marginBottom: 6 }}>
          Detected stdio command (not run by Relux):{" "}
          <span className="mono" style={{ wordBreak: "break-all" }}>
            {[command, ...args].join(" ")}
          </span>
        </div>
      )}
      {proposal.notes.map((n, i) => (
        <p
          key={i}
          className="muted"
          style={{ margin: "0 0 4px", fontSize: 11 }}
        >
          {n}
        </p>
      ))}
    </div>
  );
}

// Advanced affordance: the full hand-edit `relux-plugin.json` template (copy or
// download) for operators who prefer to author the manifest directly and
// re-install. This is the prior, lower-level path; the form above is the default.
function ManifestTemplate({ plugin }: { plugin: ReluxPlugin }) {
  const { data, loading, error } = useAsync<ReluxManifestTemplate>(
    () => reluxPlugins.manifestTemplate(plugin.id),
    [plugin.id],
  );
  const [copied, setCopied] = useState(false);

  async function copy() {
    if (!data) return;
    try {
      await navigator.clipboard.writeText(data.manifest_json);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  }

  function download() {
    if (!data) return;
    const blob = new Blob([data.manifest_json], { type: "application/json" });
    const href = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = href;
    a.download = data.filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(href);
  }

  if (error) {
    return (
      <div className="banner err" style={{ fontSize: 12, marginTop: 8 }}>
        Could not load a manifest template ({error}).
      </div>
    );
  }
  if (loading || !data) return <div className="loading">Loading template...</div>;

  return (
    <div style={{ marginTop: 8 }}>
      <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 11 }}>
        To author tools by hand instead: add this file to the plugin folder as{" "}
        <span className="mono">relux-plugin.json</span>, fill in the real tools, then
        re-install it (Local folder).
      </p>
      <div className="muted" style={{ fontSize: 11, marginBottom: 6 }}>
        Install directory:{" "}
        <span className="mono" style={{ wordBreak: "break-all" }}>
          {data.install_dir}
        </span>
      </div>
      <pre
        className="mono"
        style={{
          fontSize: 12,
          background: "var(--panel, #111)",
          padding: 10,
          borderRadius: 6,
          overflowX: "auto",
          margin: 0,
          maxHeight: 280,
        }}
      >
        {data.manifest_json}
      </pre>
      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn ghost sm" onClick={() => void copy()}>
          {copied ? "Copied" : "Copy manifest"}
        </button>
        <button className="btn ghost sm" onClick={download}>
          Download {data.filename}
        </button>
      </div>
    </div>
  );
}

// Per-plugin HTTP loopback runtime config (RELUX_MASTER_PLAN section 8.2, 18).
// Relux never auto-runs downloaded plugin code: a ToolSet plugin becomes
// executable only when the operator points it at a loopback HTTP server they run
// themselves. This panel shows the current status and lets the operator set the
// loopback URL + timeout, disable, or clear it. No secrets are stored.
function RuntimePanel({ plugin }: { plugin: ReluxPlugin }) {
  const { data, loading, error, reload } = useAsync<ReluxPluginRuntime>(
    () => reluxPluginRuntime.get(plugin.id),
    [plugin.id],
  );
  const [url, setUrl] = useState("");
  const [timeout, setTimeoutMs] = useState("");
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  const configured = data?.configured ?? false;

  // Seed the inputs from the loaded config when it (re)loads.
  useEffect(() => {
    if (!data) return;
    if (data.base_url) setUrl(data.base_url);
    if (data.timeout_ms != null) setTimeoutMs(String(data.timeout_ms));
  }, [data]);

  async function save(enabled: boolean) {
    setBusy(true);
    setBanner(null);
    const body: { base_url?: string; enabled?: boolean; timeout_ms?: number } = {
      enabled,
    };
    if (url.trim()) body.base_url = url.trim();
    if (timeout.trim()) {
      const n = Number(timeout.trim());
      if (!Number.isFinite(n) || n <= 0) {
        setBanner({ kind: "err", msg: "Timeout must be a positive number of ms." });
        setBusy(false);
        return;
      }
      body.timeout_ms = Math.floor(n);
    }
    try {
      await reluxPluginRuntime.set(plugin.id, body);
      setBanner({
        kind: "ok",
        msg: enabled ? "Runtime configured and enabled." : "Runtime disabled.",
      });
      reload();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Save failed" });
    } finally {
      setBusy(false);
    }
  }

  async function clear() {
    setBusy(true);
    setBanner(null);
    try {
      await reluxPluginRuntime.remove(plugin.id);
      setBanner({ kind: "ok", msg: "Runtime config cleared." });
      setUrl("");
      setTimeoutMs("");
      reload();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Clear failed" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>HTTP loopback runtime</strong>
        <div className="spacer" style={{ flex: 1 }} />
        {loading ? (
          <span className="muted" style={{ fontSize: 11 }}>Loading...</span>
        ) : configured ? (
          <span className={"badge " + (data?.enabled ? "done" : "backlog")}>
            {data?.enabled ? "enabled" : "disabled"}
          </span>
        ) : (
          <span className="badge backlog">not configured</span>
        )}
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 11 }}>
        Relux does not run downloaded plugin code. To make this plugin's tools
        executable, run your own plugin server locally and point Relux at it. Only
        loopback URLs are allowed: <span className="mono">http://127.0.0.1:&lt;port&gt;</span>,{" "}
        <span className="mono">http://localhost:&lt;port&gt;</span>, or{" "}
        <span className="mono">http://[::1]:&lt;port&gt;</span>. Relux POSTs{" "}
        <span className="mono">{"{ plugin_id, tool_name, input }"}</span> to{" "}
        <span className="mono">&lt;base_url&gt;/invoke</span>.
      </p>

      {error && (
        <div className="banner err" style={{ fontSize: 12 }}>
          Could not load runtime config ({error}).
        </div>
      )}
      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}

      <label className="field" style={{ margin: 0 }}>
        <span style={{ fontSize: 12 }}>Loopback base URL</span>
        <input
          className="input"
          value={url}
          placeholder="http://127.0.0.1:19999"
          onChange={(e) => setUrl(e.target.value)}
        />
      </label>
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 12 }}>Per-call timeout (ms, optional)</span>
        <input
          className="input"
          value={timeout}
          placeholder="5000"
          inputMode="numeric"
          onChange={(e) => setTimeoutMs(e.target.value)}
        />
      </label>

      <div className="row wrap" style={{ gap: 8, marginTop: 10 }}>
        <button className="btn" disabled={busy} onClick={() => void save(true)}>
          {busy ? "Saving..." : configured ? "Save & enable" : "Enable runtime"}
        </button>
        {configured && data?.enabled && (
          <button className="btn ghost" disabled={busy} onClick={() => void save(false)}>
            Disable
          </button>
        )}
        {configured && (
          <button className="btn ghost" disabled={busy} onClick={() => void clear()}>
            Clear
          </button>
        )}
      </div>
    </div>
  );
}

function InstallPanel({
  onClose,
  onInstalled,
}: {
  onClose: () => void;
  onInstalled: (p: ReluxPlugin) => void;
}) {
  const [source, setSource] = useState<Source>("github");
  const [url, setUrl] = useState("");
  const [dir, setDir] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);
  // The just-installed plugin. While set, the panel shows an honest result
  // summary (tools discovered / wrapper generated / adapter) instead of the form.
  const [result, setResult] = useState<ReluxPlugin | null>(null);

  async function submit() {
    setBusy(true);
    setBanner(null);
    try {
      let installed: ReluxPlugin;
      if (source === "github") {
        if (!url.trim()) throw new Error("Enter a GitHub URL.");
        installed = await reluxPlugins.installGithub(url.trim());
      } else if (source === "zip") {
        if (!file) throw new Error("Choose a .zip file to upload.");
        installed = await reluxPlugins.installZip(file);
      } else {
        if (!dir.trim()) throw new Error("Enter a local folder path.");
        installed = await reluxPlugins.installDir(dir.trim());
      }
      setResult(installed);
      setBusy(false);
      onInstalled(installed);
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Install failed" });
      setBusy(false);
    }
  }

  if (result) {
    const summary = installResultSummary(result);
    return (
      <div className="card" style={{ marginBottom: 12, padding: 12 }}>
        <div className={"banner " + (summary.tone === "ok" ? "ok" : "info")} style={{ fontSize: 13 }}>
          <strong>{summary.headline}</strong>
          <div style={{ marginTop: 4, fontSize: 12 }}>{summary.detail}</div>
        </div>
        <div className="muted" style={{ fontSize: 11, marginBottom: 10 }}>
          {pluginKindLabel(result)} · {result.id} · v{result.version} · source{" "}
          {result.source_kind}
          {result.generated
            ? " · 0 tools discovered"
            : ` · ${result.tool_count ?? 0} tool${(result.tool_count ?? 0) === 1 ? "" : "s"} discovered`}
        </div>
        <div className="row wrap" style={{ gap: 8 }}>
          <button
            className="btn ghost"
            onClick={() => {
              setResult(null);
              setBanner(null);
              setUrl("");
              setDir("");
              setFile(null);
            }}
          >
            Install another
          </button>
          <button className="btn" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="card" style={{ marginBottom: 12, padding: 12 }}>
      <div className="row" style={{ marginBottom: 10, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>Install a plugin</strong>
        <div className="spacer" style={{ flex: 1 }} />
        <div className="seg">
          <button
            className={"seg-btn" + (source === "github" ? " active" : "")}
            onClick={() => setSource("github")}
          >
            GitHub URL
          </button>
          <button
            className={"seg-btn" + (source === "zip" ? " active" : "")}
            onClick={() => setSource("zip")}
          >
            ZIP upload
          </button>
          <button
            className={"seg-btn" + (source === "dir" ? " active" : "")}
            onClick={() => setSource("dir")}
          >
            Local folder
          </button>
        </div>
      </div>

      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}

      {source === "github" && (
        <label className="field" style={{ margin: 0 }}>
          <span>GitHub repository URL</span>
          <input
            className="input"
            value={url}
            placeholder="https://github.com/owner/repo"
            onChange={(e) => setUrl(e.target.value)}
          />
          <p className="muted" style={{ fontSize: 11, marginTop: 6 }}>
            Cloned with <span className="mono">git clone --depth 1</span> on the Relux host.
            If the repo has a <span className="mono">relux-plugin.json</span> manifest it is used
            directly; if not, Relux generates a safe <em>metadata-only</em> wrapper manifest
            (no runnable tools) you can configure afterward.
          </p>
        </label>
      )}

      {source === "zip" && (
        <label className="field" style={{ margin: 0 }}>
          <span>Plugin .zip archive</span>
          <input
            className="input"
            type="file"
            accept=".zip,application/zip"
            onChange={(e) => setFile(e.target.files?.[0] ?? null)}
          />
          <p className="muted" style={{ fontSize: 11, marginTop: 6 }}>
            The archive is uploaded, extracted, and validated on the Relux host;
            path-traversal entries are refused. If it has a{" "}
            <span className="mono">relux-plugin.json</span> manifest it is used
            directly; if not, Relux imports it as a safe <em>metadata-only</em>{" "}
            wrapper (no runnable tools) you can configure afterward.
          </p>
        </label>
      )}

      {source === "dir" && (
        <label className="field" style={{ margin: 0 }}>
          <span>Local folder path</span>
          <input
            className="input"
            value={dir}
            placeholder="/path/to/plugin-folder"
            onChange={(e) => setDir(e.target.value)}
          />
          <p className="muted" style={{ fontSize: 11, marginTop: 6 }}>
            Browser folder picking is not available yet; this path is read on the
            Relux process host, not your machine. If the folder (or its single
            plugin subfolder) has a <span className="mono">relux-plugin.json</span>{" "}
            manifest it is used directly; if not, Relux imports it as a safe{" "}
            <em>metadata-only</em> wrapper (no runnable tools) you can configure
            afterward.
          </p>
        </label>
      )}

      <div className="row wrap" style={{ gap: 8, marginTop: 12 }}>
        <button className="btn" disabled={busy} onClick={() => void submit()}>
          {busy ? "Installing..." : "Install"}
        </button>
        <button className="btn ghost" disabled={busy} onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}
