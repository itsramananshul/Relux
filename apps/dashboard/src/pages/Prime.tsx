import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  reluxAi,
  reluxPrime,
  type ReluxAiStatus,
  type ReluxPrimeProposal,
  type ReluxPrimeSuggestion,
  type ReluxPrimeTurn,
} from "../api";
import { afterActionLabel, boundedContextReads, brainSourceLabel, contextReadDetail, contextReadsHadMiss, contextReadsUsedLabel, decisionSourceLabel, hasSteps, intentProvenance, pendingClarificationLabel, polishProvenance, proposalDisplaySummary, replyPolishLabel, requestedToolLabel, slotProvenance, stepDisplayTitle, updateProvenance } from "../prime";
import { workTaskHref, workRunHref } from "../routing";
import { PrimeAutonomyPanel } from "../components/PrimeAutonomyPanel";
import { OrchestrationPanel } from "../components/OrchestrationPanel";

// Prime page (RELUX_MASTER_PLAN section 10 Prime Behavior, section 11.1 Prime Chat): the
// conversational command surface for the local Relux control plane. It POSTs
// each message to /v1/relux/prime, which runs the SAME grounded `prime_turn`
// the kernel uses - so a greeting stays a greeting and "create a task to X"
// creates that task. Prime never turns a casual hello into a plan (section 17.1),
// and risky actions come back as a proposal awaiting approval, never silently
// done (section 10.3). This page only renders what Prime returned; it invents nothing.

// One line in the conversation: the user's message, a Prime turn, or an error.
type Entry =
  | { role: "user"; text: string }
  | { role: "prime"; turn: ReluxPrimeTurn }
  | { role: "error"; text: string };

// Tone for the disposition badge — grounded in the kernel's PrimeDisposition.
const DISPOSITION_TONE: Record<string, string> = {
  answered: "done",
  executed: "in_progress",
  awaiting_approval: "in_review",
  needs_clarification: "backlog",
};

const SUGGESTIONS = [
  "what tools can you use?",
  "what is going on?",
  "create a task to summarize the README",
  "create an agent named researcher",
  "orchestrate research the options, build a prototype, and write the docs",
  "assign task_0001 to researcher",
  "start it",
  "why did it fail?",
];

// Human label for which provider produced a reply, shown on each Prime turn so
// the answer's source is always transparent.
function providerLabel(mode: ReluxPrimeTurn["ai_mode"]): string {
  switch (mode) {
    case "openrouter":
      return "via OpenRouter";
    case "claude_cli":
      return "via Claude CLI";
    case "codex_cli":
      return "via Codex CLI";
    case "deterministic_for_action":
      return "deterministic (action)";
    default:
      return "deterministic";
  }
}

const GREETING =
  "I am Prime, the local Relux operator. Ask me what is going on, tell me to create a task, " +
  "or start a run. I act through the control plane and ask before anything risky.";

export function Prime() {
  const [log, setLog] = useState<Entry[]>([]);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [aiStatus, setAiStatus] = useState<ReluxAiStatus | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  async function refreshAi() {
    try {
      const s = await reluxAi.status();
      setAiStatus(s);
    } catch {
      setAiStatus(null);
    }
  }

  useEffect(() => {
    void refreshAi();
  }, []);

  function scroll() {
    requestAnimationFrame(() => logRef.current?.scrollTo(0, logRef.current.scrollHeight));
  }

  async function send(override?: string) {
    const message = (override ?? text).trim();
    if (!message || busy) return;
    if (override === undefined) setText("");
    setLog((l) => [...l, { role: "user", text: message }]);
    setBusy(true);
    try {
      const turn = await reluxPrime.send(message);
      setLog((l) => [...l, { role: "prime", turn }]);
      void refreshAi();
    } catch (e) {
      setLog((l) => [
        ...l,
        { role: "error", text: e instanceof Error ? e.message : "Prime request failed" },
      ]);
    } finally {
      setBusy(false);
      scroll();
    }
  }

  // Clear the conversation: drop the on-screen log AND the kernel's bounded memory
  // for this conversation (recent-turn history + any pending clarification), so the
  // next message starts fresh with no carried context. Advisory only — no task, run,
  // or agent is touched (`docs/prime-processing-audit.md` "Bounded conversation memory").
  async function clearConversation() {
    if (busy) return;
    try {
      await reluxPrime.reset();
    } catch {
      // Even if the server reset fails (e.g. kernel down), clear the local view so the
      // user still gets a fresh start; the next turn re-syncs.
    }
    setLog([]);
    setText("");
    inputRef.current?.focus();
  }

  // Act on a suggested next action (RELUX_MASTER_PLAN §11.1). A `send` suggestion
  // is dispatched immediately; otherwise we pre-fill the input so the user
  // completes or confirms the command (e.g. naming the task) before sending —
  // nothing happens on the kernel until they hit Send.
  function handleSuggestion(s: ReluxPrimeSuggestion) {
    if (busy) return;
    if (s.send) {
      void send(s.message);
      return;
    }
    setText(s.message);
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      const len = el.value.length;
      el.setSelectionRange(len, len);
    });
  }

  return (
    <div className="chat" style={{ height: "calc(100vh - 96px)" }}>
      <AiStatusBanner status={aiStatus} />
      {/* Chat-first (RELUX_MASTER_PLAN §11.1 "Prime Chat — the main page or primary
          surface"): the conversation is the page. The honest contract up front so
          a user knows musing is safe — brainstorming stays a conversation; only an
          explicit command creates or runs work (§10.5 Conversation Rules). The
          autonomy/orchestration controls live in the Advanced section below the
          input, so they never push the chat down. */}
      <div className="prime-hint muted">
        Brainstorming stays a conversation — think out loud or ask anything freely, and Prime won't
        create or run work. When an idea is worth pursuing, use the buttons under Prime's reply (like{" "}
        <span className="mono">Turn this into a task</span> or <span className="mono">Start the run</span>)
        to act on it.
      </div>
      <div className="chat-log" ref={logRef}>
        <div className="msg assistant">{GREETING}</div>
        {log.map((m, i) => {
          if (m.role === "user") {
            return (
              <div key={i} className="msg user">
                {m.text}
              </div>
            );
          }
          if (m.role === "error") {
            return (
              <div key={i} className="banner err" style={{ alignSelf: "flex-start", maxWidth: 720 }}>
                Could not reach Prime ({m.text}). Make sure{" "}
                <span className="mono">relux-kernel serve</span> is running.
              </div>
            );
          }
          return (
            <PrimeTurnCard key={i} turn={m.turn} busy={busy} onSuggestion={handleSuggestion} />
          );
        })}
        {busy && <div className="msg assistant muted">...thinking</div>}
      </div>

      {/* Discoverable, grounded example messages. */}
      <div className="chat-chips" style={{ display: "flex", gap: 8, flexWrap: "wrap", padding: "10px 0" }}>
        {SUGGESTIONS.map((s) => (
          <button key={s} className="chip" disabled={busy} onClick={() => void send(s)}>
            {s}
          </button>
        ))}
      </div>

      <div className="chat-input">
        <input
          ref={inputRef}
          className="input"
          placeholder="Message Prime - e.g. 'create a task to summarize the README'"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button className="btn" onClick={() => void send()} disabled={busy || !text.trim()}>
          Send
        </button>
        <button
          className="btn ghost"
          onClick={() => void clearConversation()}
          disabled={busy || log.length === 0}
          title="Clear this conversation's memory (history + any pending question). No tasks, runs, or agents are affected."
        >
          Clear
        </button>
      </div>

      {/* Advanced controls: Prime Autonomy (the self-driving tick loop) and
          multi-agent Orchestration. Collapsed by default so they never block the
          chat (§11.1) — still one click away below the input. */}
      <details className="prime-advanced">
        <summary>⚙ Advanced — Prime Autonomy &amp; multi-agent Orchestration</summary>
        <div className="prime-advanced-body">
          <PrimeAutonomyPanel />
          <OrchestrationPanel />
        </div>
      </details>
    </div>
  );
}

// The tool a Prime turn actually ran (with its real JSON output) or the honest
// reason a requested tool did NOT run. Rendered straight from the turn — the UI
// never fabricates a tool result. Nothing renders for a turn that touched no tool.
function ToolResult({ turn }: { turn: ReluxPrimeTurn }) {
  if (turn.invoked_tool) {
    let output = "";
    if (turn.tool_output !== undefined && turn.tool_output !== null) {
      try {
        output = JSON.stringify(turn.tool_output, null, 2);
      } catch {
        output = String(turn.tool_output);
      }
    }
    return (
      <div style={{ marginTop: 8 }}>
        <div className="row wrap" style={{ gap: 6, alignItems: "center", fontSize: 11 }}>
          <span className="badge done" style={{ fontSize: 9 }} title="Tool invoked through the kernel">
            tool
          </span>
          <span className="mono muted">{turn.invoked_tool}</span>
        </div>
        {output && (
          <pre
            className="mono"
            style={{
              margin: "6px 0 0",
              padding: "6px 8px",
              fontSize: 11,
              maxHeight: 220,
              overflow: "auto",
              border: "1px solid var(--border)",
              borderRadius: 4,
              whiteSpace: "pre-wrap",
            }}
          >
            {output}
          </pre>
        )}
      </div>
    );
  }
  if (turn.tool_error) {
    return (
      <div className="row wrap" style={{ gap: 6, marginTop: 8, alignItems: "center", fontSize: 11 }}>
        <span className="badge todo" style={{ fontSize: 9 }} title="Requested tool did not run">
          tool not run
        </span>
        <span className="muted">{turn.tool_error}</span>
      </div>
    );
  }
  return null;
}

function AiStatusBanner({ status }: { status: ReluxAiStatus | null }) {
  if (!status) return null;
  const brain = status.brain ?? "local";
  let icon = "🤖";
  let label = "Prime: Local (deterministic)";
  if (brain === "openrouter") {
    icon = "✨";
    label = status.configured ? `Prime: OpenRouter (${status.model})` : "Prime: OpenRouter (no key)";
  } else if (brain === "claude_cli") {
    icon = "✦";
    label = "Prime: Claude CLI";
  } else if (brain === "codex_cli") {
    icon = "✦";
    label = "Prime: Codex CLI";
  }
  return (
    <div className="row wrap muted" style={{ gap: 8, fontSize: 10, padding: "4px 8px", borderBottom: "1px solid var(--border)", marginBottom: 8, alignItems: "center" }} title={status.reason}>
      <span>{icon} {label}</span>
      {status.disabled && status.configured && <span className="badge todo" style={{fontSize: 8}}>LLM disabled</span>}
      <div className="spacer" style={{ flex: 1 }} />
      <Link to="/health" className="link" style={{ fontSize: 10 }} title="Choose Prime's brain (Local / OpenRouter / Claude CLI / Codex CLI)">
        Prime Brain settings →
      </Link>
    </div>
  );
}

// One Prime turn rendered as a compact card: the reply text, an intent +
// disposition chip, and any durable artifact (task created, run started, or an
// approval that is now pending). All of it is read straight from the turn — the
// UI never fabricates an outcome Prime did not report.
function PrimeTurnCard({
  turn,
  busy,
  onSuggestion,
}: {
  turn: ReluxPrimeTurn;
  busy: boolean;
  onSuggestion: (s: ReluxPrimeSuggestion) => void;
}) {
  const tone = DISPOSITION_TONE[turn.disposition] ?? "todo";
  const suggestions = turn.suggested_actions ?? [];
  return (
    <div className="msg assistant" style={{ maxWidth: 720 }}>
      <div className="row wrap" style={{ gap: 6, marginBottom: 6, alignItems: "center" }}>
        <span className="badge todo" style={{ fontSize: 9 }} title="What Prime understood">
          {turn.intent.replace(/_/g, " ")}
        </span>
        {intentProvenance(turn.intent_source) && (
          <span
            className="badge done"
            style={{ fontSize: 9 }}
            title="Prime's brain understood this intent — not keyword rules"
          >
            🧠 {intentProvenance(turn.intent_source)}
          </span>
        )}
        <span className={"badge " + tone} style={{ fontSize: 9 }} title="How the turn resolved">
          {turn.disposition.replace(/_/g, " ")}
        </span>
        {/* A small chip when a configured brain re-WORDED this clarify/brainstorm turn
            through the validated wording path (one schema-checked question / short
            summary). The turn is action-free; the wording was validated server-side. */}
        {replyPolishLabel(turn.reply_polish) && (
          <span
            className="badge done"
            style={{ fontSize: 9 }}
            title="Prime's brain phrased this reply — validated wording only, no action"
          >
            🧠 {replyPolishLabel(turn.reply_polish)}
          </span>
        )}
        {/* One concise chip when a SINGLE unified brain decision produced this turn's
            intent + slots + wording in one call (vs. the prior serial calls). The
            per-section chips above still attribute each piece. */}
        {decisionSourceLabel(turn.decision_source) && (
          <span
            className="badge done"
            style={{ fontSize: 9 }}
            title="Intent, slots, and wording came from one validated brain decision"
          >
            🧠 {decisionSourceLabel(turn.decision_source)}
          </span>
        )}
        {/* A small governed-tool chip when the brain REQUESTED a write-capable tool that
            genuinely drove this turn (a real action / approval). The mutation still flowed
            through the unchanged decide → execute / approval path; the brain wrote nothing. */}
        {requestedToolLabel(turn.requested_tool) && (
          <span
            className="badge done"
            style={{ fontSize: 9 }}
            title="The brain requested this governed write tool; Prime routed it through the normal validation/approval path"
          >
            🛠 {requestedToolLabel(turn.requested_tool)}
          </span>
        )}
        {/* A small chip when the brain re-worded an ACTIONFUL turn's confirmation AFTER the
            kernel already executed (or proposed) the action — grounded in a sanitized result
            envelope and validated against it (no claim of unexecuted work, no invented id). The
            action ran through the unchanged decide → execute / approval path; the brain changed
            no state, only the wording. */}
        {afterActionLabel(turn.after_action_source) && (
          <span
            className="badge done"
            style={{ fontSize: 9 }}
            title="Prime's brain phrased this confirmation after the action ran — grounded in the real result, no state changed"
          >
            🧠 {afterActionLabel(turn.after_action_source)}
          </span>
        )}
        <span className="muted" style={{ fontSize: 9, marginLeft: "auto" }} title="Which provider produced this reply">
          {providerLabel(turn.ai_mode)}
        </span>
      </div>
      <div style={{ whiteSpace: "pre-wrap" }}>{turn.reply}</div>

      {/* Multi-turn clarify memory: a small "waiting for: …" chip while Prime is still
          expecting an answer to this clarifying question. The NEXT message is read as the
          answer and continues the original request through the same grounded pipeline;
          the cancel button just sends "never mind" (a normal user message) to drop the
          pending context. Present only when the kernel left a clarification pending. */}
      {pendingClarificationLabel(turn.pending_clarification) && (
        <div className="row wrap" style={{ gap: 6, marginTop: 8, alignItems: "center", fontSize: 11 }}>
          <span
            className="badge todo"
            style={{ fontSize: 9 }}
            title="Prime is waiting for your answer — your next message continues this request"
          >
            ⏳ {pendingClarificationLabel(turn.pending_clarification)}
          </span>
          <button
            className="chip"
            disabled={busy}
            style={{ fontSize: 10, padding: "1px 8px" }}
            title="Drop this pending request"
            onClick={() => onSuggestion({ label: "Cancel", message: "never mind", send: true })}
          >
            Cancel
          </button>
        </div>
      )}

      {/* An actionable note — e.g. a CLI brain that was unavailable and fell back,
          with the exact next step. Surfaced so the user is never left guessing. */}
      {turn.ai_note &&
        turn.ai_mode !== "openrouter" &&
        !turn.ai_note.includes("Action executed") && (
          <div className="banner" style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
            {turn.ai_note}
          </div>
        )}

      <ToolResult turn={turn} />

      {/* The reviewable plan proposal (RELUX_MASTER_PLAN §10 planning layer, §11.1):
          a compact card showing the proposed shape — goal, steps, roles, and the
          agents work would land on. It is informational only; nothing runs from
          showing it. The explicit commit is the "Create these tasks" button below
          (from suggested_actions), so the card never acts on its own (§10.5, §17.1). */}
      {turn.proposal && <ProposalCard proposal={turn.proposal} />}

      {/* Brain-assisted task slots (RELUX_MASTER_PLAN §10.1 Intent Layer, §10.2
          Action Layer, §17.1). A compact, B&W card surfacing the normalized title,
          optional details, the honored assignee/priority, and a small provenance
          chip — present ONLY when a configured brain genuinely sharpened the slots
          and the kernel validated them. It is informational: the task was already
          created through the deterministic execute path; this just shows what the
          brain contributed, never a fresh authority. */}
      {turn.slots && (
        <div
          style={{
            marginTop: 10,
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "8px 10px",
            fontSize: 12,
          }}
        >
          <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
            <span
              className="badge done"
              style={{ fontSize: 9 }}
              title="Prime's brain extracted and normalized these task slots — not keyword slicing"
            >
              🧠 {slotProvenance(turn.slots)}
            </span>
            <span className="muted" style={{ fontSize: 9 }}>brain-extracted slots</span>
          </div>
          <div>
            <strong>{turn.slots.title}</strong>
          </div>
          {turn.slots.details && (
            <div className="muted" style={{ marginTop: 2 }}>{turn.slots.details}</div>
          )}
          {(turn.slots.assignee || turn.slots.priority != null) && (
            <div className="muted" style={{ marginTop: 4, fontSize: 11 }}>
              {turn.slots.assignee && (
                <span>
                  assignee <span className="mono">{turn.slots.assignee}</span>
                </span>
              )}
              {turn.slots.assignee && turn.slots.priority != null && <span> · </span>}
              {turn.slots.priority != null && <span>priority {turn.slots.priority}</span>}
            </div>
          )}
        </div>
      )}

      {/* Brain-assisted AGENT slots (RELUX_MASTER_PLAN §10.1, §10.2, §17.1). A compact
          chip surfacing the normalized name/id, role, and adapter the brain proposed
          and the kernel validated (duplicate id rejected, adapter checked against the
          live roster) — present ONLY when the kernel attached them. The agent was
          already created through the deterministic execute path; this shows what the
          brain contributed, never a fresh authority. */}
      {turn.agent_slots && (
        <div
          style={{
            marginTop: 10,
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "8px 10px",
            fontSize: 12,
          }}
        >
          <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
            <span
              className="badge done"
              style={{ fontSize: 9 }}
              title="Prime's brain extracted and normalized this agent — not keyword slicing"
            >
              🧠 {brainSourceLabel(turn.agent_slots.source)}
            </span>
            <span className="muted" style={{ fontSize: 9 }}>brain-extracted agent</span>
          </div>
          <div>
            <strong>{turn.agent_slots.name}</strong>{" "}
            <span className="mono muted" style={{ fontSize: 11 }}>{turn.agent_slots.id}</span>
          </div>
          {turn.agent_slots.description && (
            <div className="muted" style={{ marginTop: 2 }}>{turn.agent_slots.description}</div>
          )}
          {turn.agent_slots.persona && (
            <div className="muted" style={{ marginTop: 4, fontSize: 11, fontStyle: "italic" }}>
              persona: {turn.agent_slots.persona}
            </div>
          )}
          {(turn.agent_slots.adapter || turn.agent_slots.notes) && (
            <div className="muted" style={{ marginTop: 4, fontSize: 11 }}>
              {turn.agent_slots.adapter && (
                <span>
                  adapter <span className="mono">{turn.agent_slots.adapter}</span>
                </span>
              )}
              {turn.agent_slots.adapter && turn.agent_slots.notes && <span> · </span>}
              {turn.agent_slots.notes && <span>{turn.agent_slots.notes}</span>}
            </div>
          )}
        </div>
      )}

      {/* Brain-assisted ADMIN slots (RELUX_MASTER_PLAN §10.3, §17.1). A risky plugin
          install / permission grant the brain SHARPENED — but the action stays gated
          behind the human approval below; this chip is advisory provenance only. The
          permission subject was validated against the live agent roster; a plugin id
          is normalized. Nothing changes until the user approves. */}
      {turn.admin_slots && (
        <div
          style={{
            marginTop: 10,
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "8px 10px",
            fontSize: 12,
          }}
        >
          <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
            <span
              className="badge done"
              style={{ fontSize: 9 }}
              title="Prime's brain sharpened the subject of this risky action — it still needs your approval"
            >
              🧠 {brainSourceLabel(turn.admin_slots.source)}
            </span>
            <span className="muted" style={{ fontSize: 9 }}>
              {turn.admin_slots.kind === "plugin_install" ? "brain-extracted plugin" : "brain-extracted approval subject"}
            </span>
          </div>
          {turn.admin_slots.kind === "plugin_install" && turn.admin_slots.plugin_id && (
            <div>
              install plugin <span className="mono">{turn.admin_slots.plugin_id}</span>
            </div>
          )}
          {turn.admin_slots.kind === "permission_grant" && (
            <div>
              grant{" "}
              {turn.admin_slots.permission && <span className="mono">{turn.admin_slots.permission}</span>}
              {turn.admin_slots.subject_id && (
                <span> to <span className="mono">{turn.admin_slots.subject_id}</span></span>
              )}
            </div>
          )}
          <div className="muted" style={{ marginTop: 4, fontSize: 10 }}>
            Advisory — requires your approval before anything changes.
          </div>
        </div>
      )}

      {turn.assign_slots && (
        <div
          style={{
            marginTop: 10,
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "8px 10px",
            fontSize: 12,
          }}
        >
          <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
            <span
              className="badge done"
              style={{ fontSize: 9 }}
              title="Prime's brain resolved the task and agent from your request — both validated against the live board"
            >
              🧠 {brainSourceLabel(turn.assign_slots.source)}
            </span>
            <span className="muted" style={{ fontSize: 9 }}>brain-resolved assignment</span>
          </div>
          <div>
            assign <span className="mono">{turn.assign_slots.task_id}</span> to{" "}
            <span className="mono">{turn.assign_slots.agent_id}</span>
          </div>
        </div>
      )}

      {turn.update && (
        <div
          style={{
            marginTop: 10,
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "8px 10px",
            fontSize: 12,
          }}
        >
          <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
            <span className="muted" style={{ fontSize: 9 }}>updated</span>
            <Link to={workTaskHref(turn.update.task_id)} className="mono" title="Open this task on the Work board">
              {turn.update.task_id}
            </Link>
            {updateProvenance(turn.update) && (
              <span
                className="badge done"
                style={{ fontSize: 9 }}
                title="Prime's brain resolved this change from your request — validated against the live board"
              >
                🧠 {updateProvenance(turn.update)}
              </span>
            )}
          </div>
          <div className="col" style={{ gap: 2 }}>
            {turn.update.changes.map((c) => (
              <div key={c.field}>
                <span className="muted">{c.field}</span> → <span className="mono">{c.value}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* READ-ONLY context provenance: when a configured brain inspected live state
          through the governed read-only tool loop before answering (a task, the crew,
          the runs), surface what it looked at. The summary chip is the lone always-on
          part; the detail is collapsed and bounded so the chat is never flooded and no
          raw JSON / provider envelope is dumped. Every read changed nothing — this is
          pure provenance (RELUX_MASTER_PLAN §10.1, §17.1). */}
      {contextReadsUsedLabel(turn.context_reads) && (
        <details
          style={{
            marginTop: 10,
            border: "1px solid var(--border)",
            borderRadius: 6,
            padding: "6px 10px",
            fontSize: 11,
          }}
        >
          <summary style={{ cursor: "pointer" }} title="Prime inspected live state through the governed READ-ONLY tool loop before answering — nothing was changed">
            <span
              className={"badge " + (contextReadsHadMiss(turn.context_reads) ? "blocked" : "done")}
              style={{ fontSize: 9 }}
            >
              🔎 {contextReadsUsedLabel(turn.context_reads)}
            </span>
            {contextReadsHadMiss(turn.context_reads) && (
              <span className="muted" style={{ fontSize: 9, marginLeft: 6 }}>some lookups found nothing</span>
            )}
          </summary>
          <div className="col" style={{ gap: 2, marginTop: 6 }}>
            {(() => {
              const { shown, hidden } = boundedContextReads(turn.context_reads);
              return (
                <>
                  {shown.map((r, i) => (
                    <div key={i} className="row" style={{ gap: 6, alignItems: "baseline" }}>
                      <span
                        title={r.ok ? "found" : "not found"}
                        style={{ color: r.ok ? "var(--ok)" : "var(--err)", fontSize: 10, width: 10, flex: "0 0 auto" }}
                      >
                        {r.ok ? "✓" : "!"}
                      </span>
                      <span className="mono" style={{ fontSize: 10, flex: "0 0 auto" }}>{r.tool}</span>
                      <span className="muted">{contextReadDetail(r)}</span>
                    </div>
                  ))}
                  {hidden > 0 && (
                    <div className="muted" style={{ fontSize: 10 }}>
                      +{hidden} more read{hidden === 1 ? "" : "s"}
                    </div>
                  )}
                </>
              );
            })()}
          </div>
        </details>
      )}

      {(turn.created_task || turn.started_run || turn.created_agent || turn.approval) && (
        <div className="row wrap" style={{ gap: 10, marginTop: 10, fontSize: 11 }}>
          {turn.created_task && (
            <span className="muted">
              task <Link to={workTaskHref(turn.created_task)} className="mono" title="Open this task on the Work board">{turn.created_task}</Link>
            </span>
          )}
          {turn.started_run && (
            <span className="muted">
              run <Link to={workRunHref(turn.started_run)} className="mono" title="Open this run on the Work board">{turn.started_run}</Link>
            </span>
          )}
          {turn.created_agent && (
            <span className="muted">
              agent <Link to="/crew" className="mono" title="View the crew">{turn.created_agent}</Link>
            </span>
          )}
          {turn.approval && (
            <span className="muted">
              approval <span className="mono">{turn.approval}</span>
            </span>
          )}
        </div>
      )}

      {/* Prime suggested next actions (RELUX_MASTER_PLAN §11.1): one-click
          buttons that replace telling the user what to type. Each just routes a
          pre-written message through the normal turn, so a button can do nothing
          the user could not type. A non-`send` suggestion pre-fills the input. */}
      {suggestions.length > 0 && (
        <div className="row wrap" style={{ gap: 8, marginTop: 12 }}>
          {suggestions.map((s, i) => (
            <button
              key={i}
              className="btn"
              style={{ fontSize: 12, padding: "4px 12px" }}
              disabled={busy}
              onClick={() => onSuggestion(s)}
              title={s.send ? `Send: ${s.message}` : `Fill the message box: ${s.message}`}
            >
              {s.label}
              {!s.send && <span style={{ opacity: 0.6 }}> …</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// A compact, B&W plan proposal card (RELUX_MASTER_PLAN §10 planning layer, §11.1).
// It renders STRICTLY what Prime's proposal carried — the goal as a heading, a
// summary line, and (for a genuine multi-step plan) the proposed steps with their
// role and the agent each would land on. It mints nothing and runs nothing: the
// only commit path is Prime's explicit "Create these tasks" suggestion rendered
// below the card. The card invents no step or assignee (§17.1).
function ProposalCard({ proposal }: { proposal: ReluxPrimeProposal }) {
  return (
    <div
      style={{
        marginTop: 10,
        border: "1px solid var(--border)",
        borderRadius: 6,
        padding: "10px 12px",
      }}
    >
      <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
        <span className="badge todo" style={{ fontSize: 9 }} title="A reviewable plan — nothing is created yet">
          plan preview
        </span>
        {proposal.polish && (
          <span
            className="badge backlog"
            style={{ fontSize: 9 }}
            title={`Wording refined by ${polishProvenance(proposal)}. The steps, order, and assignees are unchanged.`}
          >
            AI-refined wording · {polishProvenance(proposal)}
          </span>
        )}
        <span className="mono" style={{ fontSize: 13, fontWeight: 600 }}>
          {proposal.goal}
        </span>
      </div>
      <div className="muted" style={{ fontSize: 11, marginBottom: hasSteps(proposal) ? 8 : 0 }}>
        {proposalDisplaySummary(proposal)}
      </div>
      {hasSteps(proposal) && (
        <ol style={{ margin: 0, paddingLeft: 0, listStyle: "none" }}>
          {proposal.steps.map((s) => (
            <li
              key={s.index}
              className="row wrap"
              style={{
                gap: 8,
                alignItems: "baseline",
                padding: "4px 0",
                borderTop: "1px solid var(--border)",
                fontSize: 12,
              }}
            >
              <span className="mono muted" style={{ fontSize: 11, minWidth: 16 }}>
                {s.index}.
              </span>
              <span style={{ flex: 1, minWidth: 160 }}>{stepDisplayTitle(proposal, s)}</span>
              <span className="badge backlog" style={{ fontSize: 9 }} title="Specialist role this step needs">
                {s.role}
              </span>
              <span className="mono muted" style={{ fontSize: 10 }} title="Agent this step would be assigned to">
                → {s.agent}
              </span>
            </li>
          ))}
        </ol>
      )}
      {/* Advisory, presentation-only notes the AI brain may attach (§17.1). These
          are wording aids for the operator — answering a question or noting a risk
          commits nothing and changes no step. */}
      {proposal.polish?.questions && proposal.polish.questions.length > 0 && (
        <PolishNotes label="Worth clarifying first" items={proposal.polish.questions} />
      )}
      {proposal.polish?.risks && proposal.polish.risks.length > 0 && (
        <PolishNotes label="Risks to keep in mind" items={proposal.polish.risks} />
      )}
      {/* The honest contract: a preview commits nothing. The "Create these tasks"
          (or one-task) button below is the only path that materializes work. */}
      <div className="muted" style={{ fontSize: 10, marginTop: 8, fontStyle: "italic" }}>
        Nothing is created yet — use the button below to commit this plan.
      </div>
    </div>
  );
}

// A compact list of advisory polish notes (clarifying questions / risks). Purely
// presentational: it renders the AI brain's wording and commits nothing (§17.1).
function PolishNotes({ label, items }: { label: string; items: string[] }) {
  return (
    <div style={{ marginTop: 8, borderTop: "1px solid var(--border)", paddingTop: 8 }}>
      <div className="muted" style={{ fontSize: 10, textTransform: "uppercase", letterSpacing: 0.4, marginBottom: 4 }}>
        {label}
      </div>
      <ul style={{ margin: 0, paddingLeft: 16, fontSize: 12 }}>
        {items.map((it, i) => (
          <li key={i} style={{ marginBottom: 2 }}>
            {it}
          </li>
        ))}
      </ul>
    </div>
  );
}
