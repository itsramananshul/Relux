import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  reluxAdapters,
  reluxPluginRuntime,
  reluxPlugins,
  reluxTools,
  type ReluxAdapterStatus,
  type ReluxManifestTemplate,
  type ReluxPlugin,
  type ReluxPluginRuntime,
  type ReluxToolDescriptor,
  type ReluxToolInvocationResult,
} from "../api";
import { useAsync } from "../components/common";
import {
  adapterStatusBadge,
  installResultSummary,
  pluginCategory,
  pluginKindLabel,
  pluginNextStep,
  pluginStatus,
  visibleTools,
  type StatusVariant,
} from "../plugins";

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

function ToolRow({ tool }: { tool: ReluxToolDescriptor }) {
  const [open, setOpen] = useState(false);
  const ready = tool.executable === "ready";

  const statusBadge =
    tool.executable === "ready" ? (
      <span className="badge done">ready</span>
    ) : tool.executable === "missing_permission" ? (
      <span className="badge backlog" title="The default agent lacks this tool's permission">
        missing permission
      </span>
    ) : tool.executable === "runtime_not_configured" ? (
      <span
        className="badge backlog"
        title="Installed, but no runtime is configured. Configure an HTTP loopback endpoint for the plugin to make it executable."
      >
        runtime not configured
      </span>
    ) : tool.executable === "runtime_disabled" ? (
      <span
        className="badge backlog"
        title="An HTTP loopback runtime is configured for this plugin but it is disabled."
      >
        runtime disabled
      </span>
    ) : (
      <span className="badge" title="Installed as metadata; the kernel has no runtime for it yet">
        installed, runtime not implemented yet
      </span>
    );

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
        <td>{statusBadge}</td>
        <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
          {ready ? (
            <button className="btn ghost sm" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
              {open ? "Close" : "Invoke"}
            </button>
          ) : (
            <span className="muted" style={{ fontSize: 11 }}>not callable</span>
          )}
        </td>
      </tr>
      {open && ready && (
        <tr>
          <td colSpan={4} style={{ background: "transparent" }}>
            <InvokeTool tool={tool} />
          </td>
        </tr>
      )}
    </>
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
  const isWrapper = next.kind === "add-manifest";
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
                declares no tools, so a runtime alone runs nothing. Next: add tool
                definitions.
              </span>
              <button
                className="banner-cta"
                onClick={() => setManifestOpen((v) => !v)}
                aria-expanded={manifestOpen}
              >
                {manifestOpen ? "Hide setup" : "Set up"}
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
              {isWrapper ? (
                <button
                  className="btn ghost sm"
                  onClick={() => setManifestOpen((v) => !v)}
                  aria-expanded={manifestOpen}
                  title="Add tool definitions so this plugin can become runnable"
                >
                  {manifestOpen ? "Close" : "Set up"}
                </button>
              ) : (
                <button
                  className="btn ghost sm"
                  onClick={() => setRuntimeOpen((v) => !v)}
                  aria-expanded={runtimeOpen}
                  title="Configure an HTTP loopback runtime for this plugin"
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
            <ManifestPanel plugin={plugin} />
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

// Manifest setup panel for a generated metadata-only wrapper (RELUX_MASTER_PLAN
// "Status after v0.1.1" item 2). A wrapper declares NO tools, so a loopback
// runtime alone surfaces nothing — the only way to make it runnable is to add
// tool definitions. This panel is honest about that architecture limit and hands
// the operator a ready-to-edit `relux-plugin.json` (copy or download) plus the
// exact path where it goes. Relux never infers tools or runs downloaded code.
function ManifestPanel({ plugin }: { plugin: ReluxPlugin }) {
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

  return (
    <div className="card" style={{ margin: "6px 0", padding: 12 }}>
      <div className="row" style={{ marginBottom: 8, alignItems: "center" }}>
        <strong style={{ fontSize: 13 }}>Make this plugin runnable</strong>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="badge in_progress">metadata only</span>
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 11 }}>
        Relux installed this plugin as <strong>metadata only</strong> because the
        source had no <span className="mono">relux-plugin.json</span>. Relux never
        infers tools from downloaded code, and configuring a loopback runtime alone
        will not surface anything — there are no tool definitions to run. To make it
        runnable: (1) add the manifest below to the plugin folder as{" "}
        <span className="mono">relux-plugin.json</span> and fill in the real tools,
        (2) re-install it (Local folder), then (3) point a loopback runtime at your
        local server.
      </p>

      {error ? (
        <div className="banner err" style={{ fontSize: 12 }}>
          Could not load a manifest template ({error}).
        </div>
      ) : loading || !data ? (
        <div className="loading">Loading template...</div>
      ) : (
        <>
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
        </>
      )}
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
            The archive is uploaded, extracted, and validated on the Relux host.
            Path-traversal entries are refused.
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
            Relux process host, not your machine. The folder (or its single plugin
            subfolder) must contain a <span className="mono">relux-plugin.json</span>.
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
