import { useCallback, useEffect, useState } from "react";
import {
  reluxAi,
  reluxAdapters,
  type ReluxAiStatus,
  type ReluxAdapterStatus,
  type ReluxPrimeBrain,
} from "../api";
import { PrimeAiSettings } from "./PrimeAiSettings";

// Prime Brain / AI Runtime (RELUX_MASTER_PLAN section 8.1 Adapter Plugins,
// section 10 Prime Behavior). The single discoverable place to choose WHO answers
// Prime's conversational turns: the local deterministic operator, OpenRouter, or
// a local coding-agent CLI (Claude / Codex). Picking a CLI brain shows its live
// adapter status and the exact next step to make it usable. Prime's *actions*
// always stay deterministic and kernel-grounded regardless of the brain.

const CLAUDE_ADAPTER_ID = "relux-adapter-claude-cli";
const CODEX_ADAPTER_ID = "relux-adapter-codex-cli";

interface BrainOption {
  brain: ReluxPrimeBrain;
  label: string;
  blurb: string;
  adapterId?: string;
  bin?: string;
}

const OPTIONS: BrainOption[] = [
  {
    brain: "local",
    label: "Local (deterministic)",
    blurb: "Grounded, rule-based replies. Always available, no external call.",
  },
  {
    brain: "claude_cli",
    label: "Claude CLI",
    blurb: "Delegate chat to your local `claude` CLI (uses your Claude login).",
    adapterId: CLAUDE_ADAPTER_ID,
    bin: "claude",
  },
  {
    brain: "codex_cli",
    label: "Codex CLI",
    blurb: "Delegate chat to your local `codex` CLI (uses your ChatGPT login).",
    adapterId: CODEX_ADAPTER_ID,
    bin: "codex",
  },
  {
    brain: "openrouter",
    label: "OpenRouter",
    blurb: "Use an OpenRouter API key for conversational replies.",
  },
];

export function PrimeBrainPanel() {
  const [status, setStatus] = useState<ReluxAiStatus | null>(null);
  const [adapters, setAdapters] = useState<ReluxAdapterStatus[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [s, a] = await Promise.all([
        reluxAi.status(),
        reluxAdapters.list().catch(() => null),
      ]);
      setStatus(s);
      setAdapters(a);
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Could not load AI status" });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const adapterFor = (id?: string) =>
    id ? (adapters ?? []).find((a) => a.plugin_id === id) ?? null : null;

  async function selectBrain(brain: ReluxPrimeBrain) {
    setBusy(true);
    setBanner(null);
    try {
      const s = await reluxAi.setConfig({ brain });
      setStatus(s);
      setBanner({ kind: "ok", msg: `Prime's brain is now ${brainLabel(brain)}.` });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Could not change brain" });
    } finally {
      setBusy(false);
    }
  }

  // "Use Claude/Codex for Prime": select the brain AND enable its adapter in one
  // click, so a user can go from zero to a CLI-backed Prime without hunting.
  async function useCli(opt: BrainOption) {
    if (!opt.adapterId) return;
    setBusy(true);
    setBanner(null);
    try {
      await reluxAdapters.set(opt.adapterId, { enabled: true });
      const s = await reluxAi.setConfig({ brain: opt.brain });
      setStatus(s);
      await refresh();
      setBanner({
        kind: "ok",
        msg: `${opt.label} enabled and selected as Prime's brain. Ask Prime a normal message to try it.`,
      });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Could not enable adapter" });
    } finally {
      setBusy(false);
    }
  }

  async function setAdapterEnabled(id: string, enabled: boolean) {
    setBusy(true);
    setBanner(null);
    try {
      await reluxAdapters.set(id, { enabled });
      await refresh();
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Adapter change failed" });
    } finally {
      setBusy(false);
    }
  }

  const selected = status?.brain ?? "local";

  return (
    <div className="card">
      <div className="row" style={{ alignItems: "center", marginBottom: 8 }}>
        <h3 style={{ margin: 0 }}>Prime Brain / AI Runtime</h3>
        <div className="spacer" style={{ flex: 1 }} />
        {status && <span className="badge done">{brainLabel(status.brain)}</span>}
        <button
          className="btn ghost sm"
          style={{ marginLeft: 8 }}
          disabled={loading || busy}
          onClick={() => void refresh()}
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 12, fontSize: 12, lineHeight: 1.6 }}>
        Choose who answers Prime's <strong>conversational</strong> turns. Prime's{" "}
        <strong>actions</strong> (creating tasks, starting runs, approvals) stay deterministic and
        kernel-grounded no matter which brain is selected. Claude and Codex run as local CLIs and
        use their own login — no API key is stored in Relux for them.
      </p>

      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}
      {status && (
        <div className="muted" style={{ fontSize: 12, marginBottom: 10 }} title={status.reason}>
          {status.reason}
        </div>
      )}

      <div className="grid" style={{ gap: 8 }}>
        {OPTIONS.map((opt) => {
          const active = selected === opt.brain;
          const adapter = adapterFor(opt.adapterId);
          return (
            <div
              key={opt.brain}
              className="card"
              style={{
                padding: 12,
                border: active ? "1px solid var(--accent, #4ade80)" : undefined,
              }}
            >
              <div className="row" style={{ alignItems: "center", gap: 8 }}>
                <strong style={{ fontSize: 13 }}>{opt.label}</strong>
                {active && <span className="badge done" style={{ fontSize: 9 }}>selected</span>}
                {opt.adapterId && adapter && (
                  <AdapterBadge adapter={adapter} />
                )}
                <div className="spacer" style={{ flex: 1 }} />
                {!active && (
                  <button
                    className="btn ghost sm"
                    disabled={busy}
                    onClick={() => void selectBrain(opt.brain)}
                  >
                    Select
                  </button>
                )}
              </div>
              <p className="muted" style={{ fontSize: 12, margin: "6px 0 0" }}>{opt.blurb}</p>

              {opt.adapterId && (
                <CliAdapterControls
                  opt={opt}
                  adapter={adapter}
                  active={active}
                  busy={busy}
                  onUse={() => void useCli(opt)}
                  onEnable={() => void setAdapterEnabled(opt.adapterId!, true)}
                  onDisable={() => void setAdapterEnabled(opt.adapterId!, false)}
                />
              )}

              {opt.brain === "openrouter" && active && (
                <div style={{ marginTop: 10 }}>
                  <PrimeAiSettings />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function brainLabel(b: ReluxPrimeBrain): string {
  switch (b) {
    case "openrouter":
      return "OpenRouter";
    case "claude_cli":
      return "Claude CLI";
    case "codex_cli":
      return "Codex CLI";
    default:
      return "Local";
  }
}

function AdapterBadge({ adapter }: { adapter: ReluxAdapterStatus }) {
  const tone =
    adapter.state === "available"
      ? "done"
      : adapter.state === "missing_binary"
        ? "blocked"
        : "backlog";
  const text =
    adapter.state === "available"
      ? "ready"
      : adapter.state === "missing_binary"
        ? "binary missing"
        : adapter.state === "disabled"
          ? "disabled"
          : "not enabled";
  return <span className={"badge " + tone} style={{ fontSize: 9 }}>{text}</span>;
}

function CliAdapterControls({
  opt,
  adapter,
  active,
  busy,
  onUse,
  onEnable,
  onDisable,
}: {
  opt: BrainOption;
  adapter: ReluxAdapterStatus | null;
  active: boolean;
  busy: boolean;
  onUse: () => void;
  onEnable: () => void;
  onDisable: () => void;
}) {
  const onPath = adapter?.available_on_path ?? false;
  const enabled = adapter?.enabled ?? false;
  const ready = adapter?.state === "available";

  return (
    <div style={{ marginTop: 8 }}>
      {adapter ? (
        <div className="muted" style={{ fontSize: 11, lineHeight: 1.6 }}>
          Binary <span className="mono">{adapter.command ?? opt.bin}</span>{" "}
          {onPath ? "(on PATH)" : "(NOT on PATH)"} · runtime{" "}
          {enabled ? "enabled" : "disabled"}
          {adapter.timeout_seconds != null && <> · timeout {adapter.timeout_seconds}s</>}
          <div style={{ marginTop: 2 }}>{adapter.detail}</div>
        </div>
      ) : (
        <div className="muted" style={{ fontSize: 11 }}>
          Adapter not installed.
        </div>
      )}

      {!onPath && (
        <div className="banner" style={{ fontSize: 11, marginTop: 6, marginBottom: 0 }}>
          {opt.bin === "claude" ? (
            <>
              Install the Claude CLI and sign in:{" "}
              <span className="mono">npm i -g @anthropic-ai/claude-code</span> then{" "}
              <span className="mono">claude</span> (it walks you through login). Make sure{" "}
              <span className="mono">claude</span> is on your PATH, then Refresh.
            </>
          ) : (
            <>
              Install the Codex CLI and sign in:{" "}
              <span className="mono">npm i -g @openai/codex</span> then{" "}
              <span className="mono">codex</span> (sign in with your ChatGPT account). Make sure{" "}
              <span className="mono">codex</span> is on your PATH, then Refresh.
            </>
          )}
        </div>
      )}

      <div className="row wrap" style={{ gap: 8, marginTop: 8 }}>
        {!(active && enabled) && (
          <button className="btn sm" disabled={busy} onClick={onUse} title={`Enable ${opt.label} and use it for Prime`}>
            {`Use ${opt.label} for Prime`}
          </button>
        )}
        {adapter && enabled ? (
          <button className="btn ghost sm" disabled={busy} onClick={onDisable}>
            Disable adapter
          </button>
        ) : adapter ? (
          <button className="btn ghost sm" disabled={busy} onClick={onEnable}>
            Enable adapter
          </button>
        ) : null}
      </div>

      {active && enabled && ready && (
        <div className="muted" style={{ fontSize: 11, marginTop: 6 }}>
          ✓ Selected and ready. Ask Prime a normal message (e.g. “explain what you can do”).
        </div>
      )}
    </div>
  );
}
