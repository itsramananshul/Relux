import { useCallback, useEffect, useState } from "react";
import {
  reluxAi,
  reluxSecrets,
  type ReluxAiStatus,
  type ReluxModelCatalog,
  type ReluxOpenRouterModel,
  type ReluxSecretStatus,
} from "../api";
import {
  filterModels,
  modelDisplayName,
  modelMetaLine,
  orderModels,
} from "../modelcatalog";

// Prime AI settings (RELUX_MASTER_PLAN "Optional LLM-backed Prime"): configure
// Prime's LLM provider/key from the dashboard, with NO environment variables and
// NO plaintext key in the UI or config. Only OpenRouter takes an API key today;
// Claude and Codex adapters authenticate through their own local CLI login (no
// key here). The key is supplied by REFERENCE: it lives write-only in the local
// secret store (`docs/mcp.md` "Local secrets & environment") and Prime resolves
// it at request time. This panel only ever sees the key-free status (mode /
// configured / referenced secret name / missing-secret) — never the key itself.
// Actions stay deterministic and kernel-grounded regardless of the model.

const NEW_SECRET = "__new__";

export function PrimeAiSettings() {
  const [status, setStatus] = useState<ReluxAiStatus | null>(null);
  const [secrets, setSecrets] = useState<ReluxSecretStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [model, setModel] = useState("");
  // The secret the operator picks to hold the key: an existing name, or the
  // sentinel that reveals the inline "create a new secret" inputs.
  const [pick, setPick] = useState("");
  const [newName, setNewName] = useState("openrouter_api_key");
  const [newValue, setNewValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);
  // The OpenRouter model catalog for the picker (loaded independently of status so
  // an offline catalog never blocks configuring the key). `null` until first load.
  const [catalog, setCatalog] = useState<ReluxModelCatalog | null>(null);
  const [catalogLoading, setCatalogLoading] = useState(false);
  // The model-picker search box query.
  const [search, setSearch] = useState("");

  // Load the model catalog. Bounded + key-free server-side; on failure the body
  // carries ok:false + a reason, which we keep so the UI shows an honest fallback
  // (manual slug field + retry) rather than going blank.
  const loadCatalog = useCallback(async () => {
    setCatalogLoading(true);
    try {
      const c = await reluxAi.models();
      setCatalog(c);
    } catch (e) {
      // A transport/HTTP error (the route itself failed) — synthesize the same
      // honest fallback shape so the picker degrades the same way.
      setCatalog({
        ok: false,
        source: "openrouter",
        models: [],
        error: e instanceof Error ? e.message : "could not load model list",
      });
    } finally {
      setCatalogLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadCatalog();
  }, [loadCatalog]);

  async function refresh() {
    try {
      const [s, secs] = await Promise.all([
        reluxAi.status(),
        reluxSecrets.list().catch(() => [] as ReluxSecretStatus[]),
      ]);
      setStatus(s);
      setSecrets(secs);
      setModel(s.model ?? "");
      setPick(s.api_key_secret ?? "");
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Could not load AI status" });
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function save() {
    setBusy(true);
    setBanner(null);
    try {
      let secretName = pick.trim();
      // Creating a new secret: store the value write-only first, then reference it.
      if (pick === NEW_SECRET) {
        secretName = newName.trim();
        if (!secretName) throw new Error("Give the secret a name (e.g. openrouter_api_key).");
        if (!newValue.trim()) throw new Error("Paste the API key value to store.");
        await reluxSecrets.set(secretName, newValue.trim());
      }
      // The dropdown is authoritative: a chosen/created name sets the reference,
      // and "— none —" (empty) clears it. The model is updated when provided.
      const body: {
        provider: string;
        api_key_secret: string;
        model?: string;
      } = { provider: "openrouter", api_key_secret: secretName };
      if (model.trim()) body.model = model.trim();
      const s = await reluxAi.setConfig(body);
      setStatus(s);
      setNewValue("");
      await refresh();
      setBanner({
        kind: "ok",
        msg: s.configured
          ? `Saved. Prime uses OpenRouter with the key from secret "${s.api_key_secret ?? secretName}".`
          : s.secret_missing
            ? `Saved the reference, but secret "${s.api_key_secret ?? secretName}" is not set yet — set its value to activate it.`
            : "Saved. (No key referenced yet — Prime stays deterministic.)",
      });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Save failed" });
    } finally {
      setBusy(false);
    }
  }

  async function clearKey() {
    setBusy(true);
    setBanner(null);
    try {
      // Clears only the reference — the secret value (if any) stays in the store
      // for reuse and can be deleted from the Secrets panel.
      const s = await reluxAi.setConfig({ api_key_secret: "" });
      setStatus(s);
      setPick("");
      await refresh();
      setBanner({ kind: "ok", msg: "Cleared the key reference. Prime is back to deterministic mode." });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Clear failed" });
    } finally {
      setBusy(false);
    }
  }

  const keyState: "ok" | "missing" | "none" =
    status?.configured ? "ok" : status?.secret_missing ? "missing" : "none";

  return (
    <div className="card">
      <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
        <h3 style={{ margin: 0 }}>Prime AI settings</h3>
        <div className="spacer" style={{ flex: 1 }} />
        {status && (
          <span
            className={
              "badge " + (keyState === "ok" ? "done" : keyState === "missing" ? "blocked" : "backlog")
            }
          >
            {keyState === "ok"
              ? "OpenRouter ready"
              : keyState === "missing"
                ? "secret missing"
                : "no key"}
          </span>
        )}
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12, lineHeight: 1.6 }}>
        Configure Prime's LLM provider here — no environment variables needed. Only{" "}
        <strong>OpenRouter</strong> uses an API key. The key is supplied by{" "}
        <strong>reference</strong>: store it once in the write-only{" "}
        <strong>Secrets</strong> store, then point Prime at it by name. Relux never
        keeps the key in this page or in its config — only the secret's name. Claude and
        Codex run as <strong>adapters</strong> and use their own local CLI login (no key
        here). Prime's actions stay deterministic and grounded regardless of the model.
      </p>

      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}

      {loading ? (
        <div className="muted">Loading AI status…</div>
      ) : (
        <>
          {status && (
            <div className="muted" style={{ fontSize: 12, marginBottom: 10 }} title={status.reason}>
              {status.reason}
            </div>
          )}
          {keyState === "missing" && status?.api_key_secret && (
            <div className="banner blocked" style={{ fontSize: 12 }}>
              The referenced secret <span className="mono">{status.api_key_secret}</span> is not set.
              Set its value below (or in the Secrets panel) to activate the key.
            </div>
          )}

          <label className="field" style={{ margin: 0 }}>
            <span style={{ fontSize: 12 }}>API key secret</span>
            <select
              className="input"
              value={pick}
              onChange={(e) => setPick(e.target.value)}
            >
              <option value="">— none (Prime stays deterministic) —</option>
              {secrets.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                  {s.preview ? ` (${s.preview})` : ""}
                  {s.scheme === "dpapi_current_user"
                    ? " · encrypted (DPAPI)"
                    : s.scheme === "plaintext_file_v1"
                      ? " · plaintext (file-locked)"
                      : ""}
                </option>
              ))}
              <option value={NEW_SECRET}>+ Create a new secret…</option>
            </select>
          </label>

          {pick === NEW_SECRET && (
            <div className="card" style={{ padding: 10, marginTop: 8 }}>
              <label className="field" style={{ margin: 0 }}>
                <span style={{ fontSize: 12 }}>New secret name</span>
                <input
                  className="input"
                  value={newName}
                  placeholder="openrouter_api_key"
                  onChange={(e) => setNewName(e.target.value)}
                />
              </label>
              <label className="field" style={{ margin: "8px 0 0" }}>
                <span style={{ fontSize: 12 }}>API key value (write-only — stored, never shown again)</span>
                <input
                  className="input"
                  type="password"
                  autoComplete="off"
                  value={newValue}
                  placeholder="sk-or-..."
                  onChange={(e) => setNewValue(e.target.value)}
                />
              </label>
            </div>
          )}

          <ModelPicker
            catalog={catalog}
            loading={catalogLoading}
            model={model}
            search={search}
            onSearch={setSearch}
            onPick={setModel}
            onRetry={() => void loadCatalog()}
          />

          <div className="row wrap" style={{ gap: 8, marginTop: 12 }}>
            <button className="btn" disabled={busy} onClick={() => void save()}>
              {busy ? "Saving…" : "Save"}
            </button>
            {status?.api_key_secret && (
              <button className="btn ghost" disabled={busy} onClick={() => void clearKey()}>
                Clear key reference
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

// The OpenRouter model picker (RELUX_MASTER_PLAN "Optional LLM-backed Prime":
// pick a model by real name/price, not an opaque slug). It shows a searchable,
// selectable list of live models from the public catalog and writes the chosen
// slug into the same `model` field the existing save path persists. Manual slug
// entry remains as the advanced/fallback path and is the ONLY path when the
// catalog can't load — so the UI is never blank and always offers a retry.
function ModelPicker({
  catalog,
  loading,
  model,
  search,
  onSearch,
  onPick,
  onRetry,
}: {
  catalog: ReluxModelCatalog | null;
  loading: boolean;
  model: string;
  search: string;
  onSearch: (q: string) => void;
  onPick: (id: string) => void;
  onRetry: () => void;
}) {
  const current = model.trim();
  const ok = catalog?.ok ?? false;
  const all = catalog?.models ?? [];
  // Current model first, then OpenRouter's server order; then the search filter.
  const visible: ReluxOpenRouterModel[] = filterModels(orderModels(all, current), search);
  const selectedInList = current ? all.some((m) => m.id === current) : false;

  return (
    <div style={{ marginTop: 12 }}>
      <div className="row" style={{ alignItems: "center", gap: 8 }}>
        <span style={{ fontSize: 12, fontWeight: 600 }}>Model</span>
        <div className="spacer" style={{ flex: 1 }} />
        {ok && all.length > 0 && (
          <span className="muted" style={{ fontSize: 11 }}>{all.length} available</span>
        )}
        <button
          className="btn ghost sm"
          disabled={loading}
          onClick={onRetry}
          title="Reload the OpenRouter model catalog"
        >
          {loading ? "Loading…" : "Refresh list"}
        </button>
      </div>
      <p className="muted" style={{ fontSize: 11, margin: "4px 0 6px", lineHeight: 1.6 }}>
        Pick a model from the live OpenRouter catalog — no need to know the slug.
        Prices are shown per million tokens (in = prompt, out = completion). You can
        still type a slug manually below.
      </p>

      {loading && all.length === 0 && (
        <div className="muted" style={{ fontSize: 12 }}>Loading the model list…</div>
      )}

      {/* Honest fallback: the catalog could not load. Keep the manual field (below)
          and explain why + offer a retry, rather than leaving the picker blank. */}
      {!loading && catalog && !ok && (
        <div className="banner info" style={{ fontSize: 11 }}>
          Couldn't load the model list{catalog.error ? ` (${catalog.error})` : ""}. You can
          still enter a model slug manually below, or{" "}
          <button
            className="btn ghost sm"
            style={{ padding: "0 6px" }}
            onClick={onRetry}
          >
            retry
          </button>
          .
        </div>
      )}

      {ok && all.length > 0 && (
        <>
          <input
            className="input"
            value={search}
            placeholder="Search models (e.g. gpt-4o, claude, llama)…"
            onChange={(e) => onSearch(e.target.value)}
            style={{ marginBottom: 6 }}
          />
          {visible.length === 0 ? (
            <div className="muted" style={{ fontSize: 12 }}>
              No models match "{search}".
            </div>
          ) : (
            <select
              className="input"
              size={8}
              value={selectedInList ? current : ""}
              onChange={(e) => onPick(e.target.value)}
              style={{ width: "100%", fontFamily: "inherit" }}
            >
              {visible.map((m) => {
                const meta = modelMetaLine(m);
                const name = modelDisplayName(m);
                const label = meta ? `${name} — ${meta}` : name;
                return (
                  <option key={m.id} value={m.id} title={m.description ?? m.id}>
                    {label}
                  </option>
                );
              })}
            </select>
          )}
        </>
      )}

      {/* Manual slug entry — the advanced path, and the fallback when the catalog
          is offline. Always reflects/sets the same `model` value the save path uses. */}
      <label className="field" style={{ margin: "8px 0 0" }}>
        <span style={{ fontSize: 11 }}>Model ID (advanced / manual)</span>
        <input
          className="input"
          value={model}
          placeholder="openai/gpt-4o-mini"
          onChange={(e) => onPick(e.target.value)}
        />
      </label>
      <p className="muted" style={{ fontSize: 11, marginTop: 4 }}>
        Leave blank to use the server default.
      </p>
    </div>
  );
}
