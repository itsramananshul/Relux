import { useEffect, useState } from "react";
import { reluxAi, reluxSecrets, type ReluxAiStatus, type ReluxSecretStatus } from "../api";

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
