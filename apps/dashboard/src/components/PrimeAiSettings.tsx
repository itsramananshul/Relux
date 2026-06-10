import { useEffect, useState } from "react";
import { reluxAi, type ReluxAiStatus } from "../api";

// Prime AI settings (RELUX_MASTER_PLAN "Optional LLM-backed Prime"): configure
// Prime's LLM provider/key from the dashboard, with NO environment variables.
// Only OpenRouter takes an API key today; Claude and Codex adapters authenticate
// through their own local CLI login (no key here). The key is stored locally
// (gitignored) and is NEVER returned by the API — this panel only ever sees the
// key-free status (mode / configured / model). Actions stay deterministic and
// kernel-grounded regardless of the model.

export function PrimeAiSettings() {
  const [status, setStatus] = useState<ReluxAiStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  async function refresh() {
    try {
      const s = await reluxAi.status();
      setStatus(s);
      setModel(s.model ?? "");
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
      const body: { provider: string; api_key?: string; model?: string } = {
        provider: "openrouter",
      };
      if (apiKey.trim()) body.api_key = apiKey.trim();
      if (model.trim()) body.model = model.trim();
      const s = await reluxAi.setConfig(body);
      setStatus(s);
      setApiKey("");
      setBanner({
        kind: "ok",
        msg: s.configured
          ? "Saved. Prime will use OpenRouter for conversational replies."
          : "Saved. (No key set yet — Prime stays deterministic.)",
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
      const s = await reluxAi.clearConfig();
      setStatus(s);
      setApiKey("");
      setBanner({ kind: "ok", msg: "Cleared. Prime is back to deterministic mode." });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Clear failed" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
        <h3 style={{ margin: 0 }}>Prime AI settings</h3>
        <div className="spacer" style={{ flex: 1 }} />
        {status && (
          <span className={"badge " + (status.configured && !status.disabled ? "done" : "backlog")}>
            {status.mode === "openrouter" ? `OpenRouter${status.configured ? "" : " (no key)"}` : "deterministic"}
          </span>
        )}
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12, lineHeight: 1.6 }}>
        Configure Prime's LLM provider here — no environment variables needed. Only{" "}
        <strong>OpenRouter</strong> uses an API key. Claude and Codex are run as{" "}
        <strong>adapters</strong> and authenticate through their own local CLI login
        (<span className="mono">claude</span> / <span className="mono">codex</span>) — there is no
        key to paste for them. The key is stored locally on this machine, never committed, and
        never returned by the API. Prime's actions stay deterministic and grounded regardless of
        the model.
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
          <label className="field" style={{ margin: 0 }}>
            <span style={{ fontSize: 12 }}>
              OpenRouter API key {status?.configured && <em>(a key is configured — leave blank to keep it)</em>}
            </span>
            <input
              className="input"
              type="password"
              autoComplete="off"
              value={apiKey}
              placeholder={status?.configured ? "•••••••• (configured)" : "sk-or-..."}
              onChange={(e) => setApiKey(e.target.value)}
            />
          </label>
          <label className="field" style={{ margin: "8px 0 0" }}>
            <span style={{ fontSize: 12 }}>Model (optional)</span>
            <input
              className="input"
              value={model}
              placeholder="openai/gpt-4o-mini"
              onChange={(e) => setModel(e.target.value)}
            />
          </label>
          <div className="row wrap" style={{ gap: 8, marginTop: 12 }}>
            <button className="btn" disabled={busy} onClick={() => void save()}>
              {busy ? "Saving…" : "Save"}
            </button>
            {status?.configured && (
              <button className="btn ghost" disabled={busy} onClick={() => void clearKey()}>
                Clear key
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
