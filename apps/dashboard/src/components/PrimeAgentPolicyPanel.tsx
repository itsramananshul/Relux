import { useEffect, useState } from "react";
import {
  reluxPrimeAgentPolicy,
  type ReluxPrimeAgentPolicy,
  type ReluxPrimeAgentPolicyResponse,
  ApiError,
} from "../api";

// Prime Brain / Autonomy — the CONFIGURABLE limits that bound Prime's chat agent loop
// (docs/mcp.md "Prime Agent Loop"; RELUX_MASTER_PLAN §10.5/§17.1). Replaces the old fixed
// v1 caps with an operator-set policy: a practical Standard profile and a higher Extended
// profile used when the user explicitly asks Prime to "keep working". Even Extended is
// bounded — there is no infinite setting; when a limit is hit Prime says so and offers to
// continue. Compact on purpose: two small editable rows + a one-line active-policy summary.

type Draft = Pick<
  ReluxPrimeAgentPolicy,
  | "max_tool_calls"
  | "max_brain_rounds"
  | "max_duration_secs"
  | "extended_max_tool_calls"
  | "extended_max_brain_rounds"
  | "extended_max_duration_secs"
  | "max_tool_plan_steps"
  | "extended_max_tool_plan_steps"
  | "max_orchestration_steps"
  | "extended_max_orchestration_steps"
  | "max_context_rounds"
  | "extended_max_context_rounds"
  | "max_active_jobs"
  | "extended_max_active_jobs"
>;

function toDraft(c: ReluxPrimeAgentPolicy): Draft {
  return {
    max_tool_calls: c.max_tool_calls,
    max_brain_rounds: c.max_brain_rounds,
    max_duration_secs: c.max_duration_secs,
    extended_max_tool_calls: c.extended_max_tool_calls,
    extended_max_brain_rounds: c.extended_max_brain_rounds,
    extended_max_duration_secs: c.extended_max_duration_secs,
    max_tool_plan_steps: c.max_tool_plan_steps,
    extended_max_tool_plan_steps: c.extended_max_tool_plan_steps,
    max_orchestration_steps: c.max_orchestration_steps,
    extended_max_orchestration_steps: c.extended_max_orchestration_steps,
    max_context_rounds: c.max_context_rounds,
    extended_max_context_rounds: c.extended_max_context_rounds,
    max_active_jobs: c.max_active_jobs,
    extended_max_active_jobs: c.extended_max_active_jobs,
  };
}

export function PrimeAgentPolicyPanel() {
  const [resp, setResp] = useState<ReluxPrimeAgentPolicyResponse | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [banner, setBanner] = useState<{ kind: string; msg: string } | null>(null);

  async function refresh() {
    setLoading(true);
    try {
      const r = await reluxPrimeAgentPolicy.get();
      setResp(r);
      setDraft(toDraft(r.config));
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof ApiError ? e.message : "Could not load policy" });
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  function set<K extends keyof Draft>(key: K, value: number) {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
  }

  async function save() {
    if (!draft) return;
    setBusy(true);
    setBanner(null);
    try {
      const r = await reluxPrimeAgentPolicy.update(draft);
      setResp(r);
      setDraft(toDraft(r.config));
      setBanner({ kind: "ok", msg: "Autonomy limits saved (clamped to safe ranges)." });
    } catch (e) {
      setBanner({ kind: "err", msg: e instanceof ApiError ? e.message : "Could not save policy" });
    } finally {
      setBusy(false);
    }
  }

  if (loading) {
    return <div className="card">Loading Prime autonomy limits…</div>;
  }
  if (!resp || !draft) {
    return <div className="banner err">{banner?.msg ?? "No autonomy policy."}</div>;
  }

  const dirty = JSON.stringify(draft) !== JSON.stringify(toDraft(resp.config));

  return (
    <div className="card">
      <div className="row" style={{ alignItems: "center", marginBottom: 6 }}>
        <h3 style={{ margin: 0 }}>Prime Autonomy Limits</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="badge done" style={{ fontSize: 9 }} title="Resolved standard profile">
          std {resp.standard.max_tool_calls}t · {resp.standard.max_brain_rounds}r ·{" "}
          {resp.standard.max_duration_secs}s
        </span>
        <span className="badge backlog" style={{ fontSize: 9, marginLeft: 6 }} title="Resolved extended profile">
          ext {resp.extended.max_tool_calls}t · {resp.extended.max_brain_rounds}r ·{" "}
          {resp.extended.max_duration_secs}s
        </span>
        <span
          className="badge todo"
          style={{ fontSize: 9, marginLeft: 6 }}
          title="Resolved multi-tool-plan step limit (standard / extended)"
        >
          plan {resp.standard.max_tool_plan_steps}/{resp.extended.max_tool_plan_steps} steps
        </span>
        <span
          className="badge todo"
          style={{ fontSize: 9, marginLeft: 6 }}
          title="Resolved orchestration fan-out width — briefs per goal (standard / extended)"
        >
          orch {resp.standard.max_orchestration_steps}/{resp.extended.max_orchestration_steps} briefs
        </span>
        <span
          className="badge todo"
          style={{ fontSize: 9, marginLeft: 6 }}
          title="Resolved read-only context-loop rounds (standard / extended)"
        >
          ctx {resp.standard.max_context_rounds}/{resp.extended.max_context_rounds} rounds
        </span>
        <span
          className="badge backlog"
          style={{ fontSize: 9, marginLeft: 6 }}
          title="Resolved concurrent background-job admission cap — max active run-jobs across the fleet (standard / extended)"
        >
          jobs {resp.standard.max_active_jobs}/{resp.extended.max_active_jobs} active
        </span>
      </div>
      <p className="muted" style={{ marginTop: -2, marginBottom: 10, fontSize: 12, lineHeight: 1.6 }}>
        How far Prime's chat <strong>agent loop</strong> may go in one turn before it stops and asks.
        The <strong>Extended</strong> profile kicks in when you tell Prime to "keep working" or use
        "extended mode". Even Extended is bounded — there's no infinite loop; when a limit is reached
        Prime says which one and offers to continue. Approvals always still pause.
      </p>

      {banner && (
        <div className={"banner " + banner.kind} style={{ fontSize: 12 }}>{banner.msg}</div>
      )}

      <LimitRow
        label="Standard"
        toolCalls={draft.max_tool_calls}
        brainRounds={draft.max_brain_rounds}
        durationSecs={draft.max_duration_secs}
        disabled={busy}
        onToolCalls={(v) => set("max_tool_calls", v)}
        onBrainRounds={(v) => set("max_brain_rounds", v)}
        onDuration={(v) => set("max_duration_secs", v)}
      />
      <LimitRow
        label="Extended"
        toolCalls={draft.extended_max_tool_calls}
        brainRounds={draft.extended_max_brain_rounds}
        durationSecs={draft.extended_max_duration_secs}
        disabled={busy}
        onToolCalls={(v) => set("extended_max_tool_calls", v)}
        onBrainRounds={(v) => set("extended_max_brain_rounds", v)}
        onDuration={(v) => set("extended_max_duration_secs", v)}
      />

      {/* The configurable multi-tool-PLAN step limit (operator-authored / Prime-proposed
          plans), replacing the retired hard-coded 5. Standard bounds an ordinary tool-run
          task; Extended a long-work plan. Both are clamped to a safe ceiling on the server. */}
      <div className="row wrap" style={{ alignItems: "center", gap: 8, marginTop: 2, marginBottom: 4 }}>
        <strong style={{ fontSize: 12, width: 72 }}>Tool plan</strong>
        <Field
          label="std steps"
          value={draft.max_tool_plan_steps}
          disabled={busy}
          onChange={(v) => set("max_tool_plan_steps", Math.max(0, parseInt(v, 10) || 0))}
        />
        <Field
          label="ext steps"
          value={draft.extended_max_tool_plan_steps}
          disabled={busy}
          onChange={(v) => set("extended_max_tool_plan_steps", Math.max(0, parseInt(v, 10) || 0))}
        />
      </div>

      {/* The configurable orchestration fan-out width (briefs a single goal decomposes into),
          replacing the bare module constant. Standard bounds an ordinary orchestration; Extended a
          long-work fan-out. Both clamped to a safe ceiling on the server; overflow is reported in an
          honest note, never silently dropped. */}
      <div className="row wrap" style={{ alignItems: "center", gap: 8, marginTop: 2, marginBottom: 4 }}>
        <strong style={{ fontSize: 12, width: 72 }}>Orchestration</strong>
        <Field
          label="std briefs"
          value={draft.max_orchestration_steps}
          disabled={busy}
          onChange={(v) => set("max_orchestration_steps", Math.max(0, parseInt(v, 10) || 0))}
        />
        <Field
          label="ext briefs"
          value={draft.extended_max_orchestration_steps}
          disabled={busy}
          onChange={(v) => set("extended_max_orchestration_steps", Math.max(0, parseInt(v, 10) || 0))}
        />
      </div>

      {/* The configurable read-only context-loop round budget (how many times Prime may inspect
          live state before answering), replacing the bare MAX_TOOL_ROUNDS constant. The loop changes
          nothing; this only bounds brain-call count. Both clamped to a safe ceiling on the server. */}
      <div className="row wrap" style={{ alignItems: "center", gap: 8, marginTop: 2, marginBottom: 4 }}>
        <strong style={{ fontSize: 12, width: 72 }}>Context loop</strong>
        <Field
          label="std rounds"
          value={draft.max_context_rounds}
          disabled={busy}
          onChange={(v) => set("max_context_rounds", Math.max(0, parseInt(v, 10) || 0))}
        />
        <Field
          label="ext rounds"
          value={draft.extended_max_context_rounds}
          disabled={busy}
          onChange={(v) => set("extended_max_context_rounds", Math.max(0, parseInt(v, 10) || 0))}
        />
      </div>

      {/* The configurable concurrent background-job admission cap (the async run-async fleet
          limit), replacing the retired hidden MAX_ACTIVE_JOBS=4. A REAL resource guardrail —
          each active job drives live adapter processes — so Standard stays conservative and
          Extended (opt-in per start) admits more for a busy operator. Both clamped to a safe
          ceiling on the server; the over-limit response names the configured limit and how to
          raise it. */}
      <div className="row wrap" style={{ alignItems: "center", gap: 8, marginTop: 2, marginBottom: 4 }}>
        <strong style={{ fontSize: 12, width: 72 }}>Active jobs</strong>
        <Field
          label="std jobs"
          value={draft.max_active_jobs}
          disabled={busy}
          onChange={(v) => set("max_active_jobs", Math.max(0, parseInt(v, 10) || 0))}
        />
        <Field
          label="ext jobs"
          value={draft.extended_max_active_jobs}
          disabled={busy}
          onChange={(v) => set("extended_max_active_jobs", Math.max(0, parseInt(v, 10) || 0))}
        />
      </div>

      <div className="row" style={{ gap: 8, marginTop: 8 }}>
        <button className="btn sm" onClick={() => void save()} disabled={busy || !dirty}>
          {busy ? "Saving…" : "Save limits"}
        </button>
        {dirty && (
          <button
            className="btn ghost sm"
            disabled={busy}
            onClick={() => setDraft(toDraft(resp.config))}
          >
            Reset
          </button>
        )}
      </div>
    </div>
  );
}

function LimitRow({
  label,
  toolCalls,
  brainRounds,
  durationSecs,
  disabled,
  onToolCalls,
  onBrainRounds,
  onDuration,
}: {
  label: string;
  toolCalls: number;
  brainRounds: number;
  durationSecs: number;
  disabled: boolean;
  onToolCalls: (v: number) => void;
  onBrainRounds: (v: number) => void;
  onDuration: (v: number) => void;
}) {
  const num = (v: string) => Math.max(0, parseInt(v, 10) || 0);
  return (
    <div className="row wrap" style={{ alignItems: "center", gap: 8, marginBottom: 8 }}>
      <strong style={{ fontSize: 12, width: 72 }}>{label}</strong>
      <Field label="tool calls" value={toolCalls} disabled={disabled} onChange={(v) => onToolCalls(num(v))} />
      <Field label="rounds" value={brainRounds} disabled={disabled} onChange={(v) => onBrainRounds(num(v))} />
      <Field label="sec" value={durationSecs} disabled={disabled} onChange={(v) => onDuration(num(v))} />
    </div>
  );
}

function Field({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: number;
  disabled: boolean;
  onChange: (v: string) => void;
}) {
  return (
    <label className="muted" style={{ fontSize: 11, display: "inline-flex", alignItems: "center", gap: 4 }}>
      <input
        type="number"
        min="0"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className="input"
        style={{ width: 64 }}
      />
      {label}
    </label>
  );
}
