import { useEffect, useState } from "react";
import { reluxWatchdog, type ReluxWatchdogConfig, ApiError } from "../api";

// Run Watchdog — the operator-tunable stall-recovery policy (relux_core::RunWatchdogConfig;
// RELUX_MASTER_PLAN §9.6.1 "Run Lifecycle & Stall Recovery"). A run is created Running the
// instant it starts; a real execution is expected to drive it to a terminal state. If nothing
// does — a start that never ran, an interrupted process, a restart — the watchdog recovers the
// run as a stale failure after this window so it never hangs silently. Genuinely-live runs
// (streaming output) are never flagged. Compact on purpose: an on/off toggle + the window.

// The window is clamped server-side to this band; mirror it here so the input guides the
// operator before the round-trip (the server is still the source of truth).
const MIN_WINDOW = 30;
const MAX_WINDOW = 21_600;

function humanWindow(secs: number): string {
  if (secs < 120) return `${secs}s`;
  const mins = Math.round(secs / 60);
  if (mins < 120) return `${mins} min`;
  return `${(secs / 3600).toFixed(1)} h`;
}

export function RunWatchdogPanel() {
  const [cfg, setCfg] = useState<ReluxWatchdogConfig | null>(null);
  const [draft, setDraft] = useState<ReluxWatchdogConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  async function refresh() {
    setLoading(true);
    try {
      const r = await reluxWatchdog.get();
      setCfg(r);
      setDraft(r);
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof ApiError ? e.message : "Could not load watchdog policy" });
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function save() {
    if (!draft) return;
    setBusy(true);
    setBanner(null);
    try {
      const r = await reluxWatchdog.set(draft);
      setCfg(r);
      setDraft(r);
      setBanner({
        kind: "ok",
        msg: `Watchdog saved — ${r.enabled ? `recovers a stalled run after ${humanWindow(r.stale_after_secs)} of no activity` : "disabled"}.`,
      });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof ApiError ? e.message : "Could not save watchdog policy" });
    } finally {
      setBusy(false);
    }
  }

  if (loading) {
    return <div className="card">Loading run watchdog…</div>;
  }
  if (!cfg || !draft) {
    return <div className="banner err">{banner?.msg ?? "No watchdog policy."}</div>;
  }

  const dirty = JSON.stringify(draft) !== JSON.stringify(cfg);

  return (
    <div className="card">
      <div className="row" style={{ alignItems: "center", marginBottom: 6 }}>
        <h3 style={{ margin: 0 }}>Run Watchdog</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <span
          className={"badge " + (cfg.enabled ? "done" : "todo")}
          style={{ fontSize: 9 }}
          title="Whether the periodic stale-run sweep is active"
        >
          {cfg.enabled ? `on · ${humanWindow(cfg.stale_after_secs)}` : "off"}
        </span>
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 10, fontSize: 12, lineHeight: 1.6 }}>
        Guarantees no run hangs silently. A run that stays <strong>running</strong> with no transcript
        activity for this window — and has no live process behind it — is recovered as a{" "}
        <strong>stalled failure</strong> with a clear retry / cancel / investigate path. A run that is
        genuinely executing (streaming output) is never flagged. Raise the window if a legitimately
        long, quiet run is being recovered too early.
      </p>

      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}

      <div className="row" style={{ alignItems: "center", gap: 12, flexWrap: "wrap" }}>
        <label className="row" style={{ alignItems: "center", gap: 6, fontSize: 13 }}>
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })}
          />
          Watchdog enabled
        </label>
        <label className="row" style={{ alignItems: "center", gap: 6, fontSize: 13 }}>
          Stall window (seconds)
          <input
            className="input sm"
            type="number"
            min={MIN_WINDOW}
            max={MAX_WINDOW}
            style={{ width: 90 }}
            value={draft.stale_after_secs}
            disabled={!draft.enabled}
            onChange={(e) =>
              setDraft({ ...draft, stale_after_secs: Number(e.target.value) || MIN_WINDOW })
            }
          />
          <span className="muted" style={{ fontSize: 11 }}>
            {humanWindow(draft.stale_after_secs)} · clamped to {MIN_WINDOW}–{MAX_WINDOW}s
          </span>
        </label>
        <div className="spacer" style={{ flex: 1 }} />
        <button className="btn sm" onClick={() => void save()} disabled={busy || !dirty}>
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}
