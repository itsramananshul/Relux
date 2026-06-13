import { useCallback, useEffect, useState } from "react";
import {
  reluxAi,
  reluxAdapters,
  type ReluxAiStatus,
  type ReluxAdapterStatus,
  type ReluxBrainProbe,
  type ReluxLiveBrainProbe,
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
  // A real conversational-Prime path the product recommends.
  recommended?: boolean;
  // Test/fallback plumbing — grounded but not the product chat experience.
  fallback?: boolean;
}

// Recommended brains first (a real conversational Prime), with the Local
// fallback last and clearly labelled as test plumbing, so the obvious choice is
// the product path — not the deterministic stand-in (RELUX_MASTER_PLAN §10.1 /
// §14: the LLM brain is the primary surface; Local is the fallback rail).
const OPTIONS: BrainOption[] = [
  {
    brain: "claude_cli",
    label: "Claude CLI",
    blurb: "Delegate chat to your local `claude` CLI (uses your Claude login). Recommended.",
    adapterId: CLAUDE_ADAPTER_ID,
    bin: "claude",
    recommended: true,
  },
  {
    brain: "codex_cli",
    label: "Codex CLI",
    blurb: "Delegate chat to your local `codex` CLI (uses your ChatGPT login).",
    adapterId: CODEX_ADAPTER_ID,
    bin: "codex",
    recommended: true,
  },
  {
    brain: "openrouter",
    label: "OpenRouter",
    blurb: "Use an OpenRouter API key for conversational replies (stored as a write-only secret).",
    recommended: true,
  },
  {
    brain: "local",
    label: "Local (deterministic)",
    blurb:
      "Grounded, rule-based replies — fallback / test plumbing, not the product chat path. " +
      "Used automatically only when no real brain is set up.",
    fallback: true,
  },
];

export function PrimeBrainPanel() {
  const [status, setStatus] = useState<ReluxAiStatus | null>(null);
  const [adapters, setAdapters] = useState<ReluxAdapterStatus[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);
  // The latest safe probe result per brain, plus which brain is mid-probe.
  const [probes, setProbes] = useState<Record<string, ReluxBrainProbe>>({});
  const [probing, setProbing] = useState<ReluxPrimeBrain | null>(null);
  // The latest LIVE chat-probe result per brain, plus which brain is mid-live-probe.
  // The live probe is explicit-only (see `testBrainLive`) and may use the real
  // provider / CLI, so it is never run on load — only on a deliberate click.
  const [liveProbes, setLiveProbes] = useState<Record<string, ReluxLiveBrainProbe>>({});
  const [liveProbing, setLiveProbing] = useState<ReluxPrimeBrain | null>(null);

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

  // Whether a LIVE chat probe can do anything useful for this brain yet. When it
  // cannot, the live button is disabled with the reason shown — so we never invite
  // a pointless (and for OpenRouter, billable) attempt on an unconfigured brain.
  function liveApplicable(opt: BrainOption): { ok: boolean; reason: string } {
    if (opt.brain === "local") {
      return { ok: true, reason: "Deterministic local brain — safe to test (no provider call)." };
    }
    if (opt.brain === "openrouter") {
      return status?.configured
        ? { ok: true, reason: "Sends one small, billable OpenRouter request." }
        : { ok: false, reason: "Configure an OpenRouter key first, then live-test it." };
    }
    const adapter = adapterFor(opt.adapterId);
    return adapter?.state === "available"
      ? { ok: true, reason: "Runs one real CLI chat turn (uses your CLI login)." }
      : { ok: false, reason: "Enable the adapter and get the CLI on PATH first, then live-test it." };
  }

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

  // Safely test a brain: a read-only probe that never runs an agent turn, never
  // bypasses permissions, and never sends a billable OpenRouter request.
  async function testBrain(brain: ReluxPrimeBrain) {
    setProbing(brain);
    setBanner(null);
    try {
      const probe = await reluxAi.probe(brain);
      setProbes((prev) => ({ ...prev, [brain]: probe }));
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Probe failed" });
    } finally {
      setProbing(null);
    }
  }

  // EXPLICIT live chat probe: actually completes one tiny bounded chat turn
  // through the brain. This MAY use the real provider / CLI and MAY incur
  // provider usage, so it only ever runs on a deliberate click (never on load).
  async function testBrainLive(brain: ReluxPrimeBrain) {
    setLiveProbing(brain);
    setBanner(null);
    try {
      const probe = await reluxAi.probeLive(brain);
      setLiveProbes((prev) => ({ ...prev, [brain]: probe }));
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof Error ? e.message : "Live probe failed" });
    } finally {
      setLiveProbing(null);
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
      <p className="muted" style={{ marginTop: -6, marginBottom: 12, fontSize: 11, lineHeight: 1.6 }}>
        This choice also decides how Prime runs <strong>work</strong>: when you start a free-form
        goal assigned to Prime, it executes on the selected <strong>Claude</strong> or{" "}
        <strong>Codex</strong> CLI adapter. With <strong>Local</strong> (or{" "}
        <strong>OpenRouter</strong>, which is conversational only), such a run <strong>fails
        closed</strong> with a setup prompt instead of silently doing nothing — so enable and select
        a CLI brain here to give Prime's work runs a real adapter.
      </p>
      <p className="muted" style={{ marginTop: -6, marginBottom: 12, fontSize: 11, lineHeight: 1.6 }}>
        <strong>Quick probe</strong> is safe and free — it checks availability only (a CLI's{" "}
        <span className="mono">--version</span>, or that an OpenRouter key resolves), so it cannot
        prove a chat turn actually works. <strong>Test live chat</strong> proves it: it sends one
        tiny message through the brain. It runs <strong>only when you click it</strong> and{" "}
        <strong>may use the real provider / CLI and may incur provider usage</strong>. It never
        creates a task or run.
      </p>

      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}
      {status && (
        <div className="muted" style={{ fontSize: 12, marginBottom: 10 }} title={status.reason}>
          {status.reason}
        </div>
      )}
      {status && status.brain === "local" && (
        <div className="banner info" style={{ fontSize: 12 }}>
          Prime is on the <strong>Local fallback</strong> — grounded, but not a real conversational
          agent. Pick <strong>Claude CLI</strong>, <strong>Codex CLI</strong>, or{" "}
          <strong>OpenRouter</strong> below for a real chat brain, then <strong>Test</strong> it.
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
              <div className="row wrap" style={{ alignItems: "center", gap: 8 }}>
                <strong style={{ fontSize: 13 }}>{opt.label}</strong>
                {active && <span className="badge done" style={{ fontSize: 9 }}>selected</span>}
                {opt.recommended && (
                  <span className="badge todo" style={{ fontSize: 9 }} title="A real conversational Prime — the product chat path">
                    recommended
                  </span>
                )}
                {opt.fallback && (
                  <span className="badge backlog" style={{ fontSize: 9 }} title="Test/fallback plumbing — not the product chat path">
                    fallback / test
                  </span>
                )}
                {opt.adapterId && adapter && (
                  <AdapterBadge adapter={adapter} />
                )}
                <div className="spacer" style={{ flex: 1 }} />
                <button
                  className="btn ghost sm"
                  disabled={probing === opt.brain}
                  onClick={() => void testBrain(opt.brain)}
                  title="Quick probe: safely check whether this brain is usable (read-only; no agent run, no bypass, no billable call)"
                >
                  {probing === opt.brain ? "Testing…" : "Quick probe"}
                </button>
                {(() => {
                  const live = liveApplicable(opt);
                  const isLocal = opt.brain === "local";
                  return (
                    <button
                      className="btn ghost sm"
                      disabled={liveProbing === opt.brain || !live.ok}
                      onClick={() => void testBrainLive(opt.brain)}
                      title={
                        live.ok
                          ? isLocal
                            ? `Live chat test — ${live.reason}`
                            : `Live chat test — sends a real message through this brain. ${live.reason} Runs only when you click.`
                          : `Live chat test unavailable — ${live.reason}`
                      }
                    >
                      {liveProbing === opt.brain ? "Testing live…" : "Test live chat"}
                    </button>
                  );
                })()}
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

              {probes[opt.brain] && <ProbeResult probe={probes[opt.brain]} />}
              {liveProbes[opt.brain] && <LiveProbeResult probe={liveProbes[opt.brain]} />}

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

// The outcome of a safe brain probe: a ready/blocked badge, the secret-free
// detail (with the next step on failure), and the captured CLI version line.
function ProbeResult({ probe }: { probe: ReluxBrainProbe }) {
  const tone = probe.ok ? "ok" : probe.status === "failed" ? "err" : "info";
  const label = probe.ok
    ? "ready"
    : probe.status === "disabled"
      ? "disabled"
      : probe.status === "missing_binary"
        ? "not on PATH"
        : probe.status === "not_configured"
          ? "not enabled"
          : probe.status === "missing_key"
            ? "no key"
            : "failed";
  return (
    <div className={"banner " + tone} style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
      <strong>{probe.ok ? "✓ " : "⚠ "}{label}</strong> — {probe.detail}
      {probe.version && (
        <>
          {" "}
          <span className="mono">{probe.version}</span>
        </>
      )}
    </div>
  );
}

// The outcome of an explicit LIVE chat probe: a ready/failed badge, the
// secret-free detail (with the next step on failure), a redacted sample of the
// real reply when one came back, and how long the turn took.
function LiveProbeResult({ probe }: { probe: ReluxLiveBrainProbe }) {
  const tone = probe.ok
    ? "ok"
    : probe.status === "not_configured" || probe.status === "missing_key"
      ? "info"
      : "err";
  const label = probe.ok
    ? "live chat OK"
    : probe.status === "auth_failed"
      ? "auth failed"
      : probe.status === "timeout"
        ? "timed out"
        : probe.status === "missing_key"
          ? "no key"
          : probe.status === "not_configured"
            ? "not set up"
            : probe.status === "unsupported"
              ? "unsupported"
              : "failed";
  const seconds = probe.duration_ms > 0 ? ` · ${(probe.duration_ms / 1000).toFixed(1)}s` : "";
  return (
    <div className={"banner " + tone} style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
      <strong>{probe.ok ? "✓ " : "⚠ "}Live: {label}</strong>{seconds} — {probe.detail}
      {probe.sample && (
        <div style={{ marginTop: 4 }}>
          Reply: <span className="mono">{probe.sample}</span>
        </div>
      )}
    </div>
  );
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
