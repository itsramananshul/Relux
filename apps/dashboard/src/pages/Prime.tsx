import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  reluxAi,
  reluxApprovals,
  reluxPrime,
  reluxWork,
  type ReluxAiStatus,
  type ReluxCapabilityCandidate,
  type ReluxPrimeConfigureCandidateResult,
  type ReluxPrimeInstallPluginResult,
  type ReluxPrimeProposal,
  type ReluxPrimeSuggestion,
  type ReluxPrimeToolApprovalRequest,
  type ReluxPrimeToolPlanProposal,
  type ReluxPrimeToolView,
  type ReluxPrimeTurn,
  type ReluxToolDescriptor,
  type ReluxToolInvocationResult,
} from "../api";
import { afterActionLabel, boundedContextReads, brainSourceLabel, configurePluginCandidateAction, contextReadDetail, contextReadsHadMiss, contextReadsUsedLabel, decisionSourceLabel, formatToolOutput, githubPluginInstallAction, hasSteps, intentProvenance, pendingClarificationLabel, polishProvenance, PRIME_GREETING, PRIME_HINT, PRIME_PLACEHOLDER, PRIME_SUGGESTIONS, proposalDisplaySummary, replyPolishLabel, requestedToolLabel, slotProvenance, stepDisplayTitle, updateProvenance, type ConfigurePluginCandidateAction } from "../prime";
import { workTaskHref, workRunHref } from "../routing";
import { consumeInvestigationSeed } from "../investigateseed";
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

// The example chips and Prime's intro/hint/placeholder copy now live in the pure
// `../prime` module (`PRIME_SUGGESTIONS` / `PRIME_GREETING` / `PRIME_HINT` /
// `PRIME_PLACEHOLDER`) so the Hermes-first, general-agent framing is unit-testable
// (`docs/prime-processing-audit.md` "Hermes-first general agent").
const SUGGESTIONS = PRIME_SUGGESTIONS;

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

const GREETING = PRIME_GREETING;

export function Prime() {
  const [log, setLog] = useState<Entry[]>([]);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [aiStatus, setAiStatus] = useState<ReluxAiStatus | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  // Guards the one-shot investigation-seed pickup so a re-render / StrictMode
  // double-invoke never re-sends it (the consume also removes it from storage).
  const seedConsumedRef = useRef(false);

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

  // "Investigate with Prime" handoff (recovery decision card §3.3b / §6.10): when the
  // operator opened Prime from a recovery card, a safe, bounded, redacted investigation
  // seed was stashed in sessionStorage. Pick it up ONCE on mount and send it as the
  // first user message, so Prime answers it like a debugging partner. The seed itself
  // instructs Prime not to create or run anything; it is a normal "answered" turn, so
  // nothing is materialized. No seed → normal chat, untouched (investigateseed.ts).
  useEffect(() => {
    if (seedConsumedRef.current) return;
    seedConsumedRef.current = true;
    const seed = consumeInvestigationSeed(window.sessionStorage);
    if (seed) void send(seed);
    // eslint-disable-next-line react-hooks/exhaustive-deps
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

  // Resume a paused agent loop ("Keep working"): call the continuation route with the stored token
  // (NOT a re-sent message), and append the resumed loop's turn. This continues from the
  // already-gathered observations, so it does not repeat completed tool calls.
  async function continueLoop(id: string, extended: boolean, label?: string) {
    if (busy) return;
    const line = label ?? (extended ? "Keep working (extended)" : "Keep working");
    setLog((l) => [...l, { role: "user", text: line }]);
    setBusy(true);
    try {
      const turn = await reluxPrime.continue(id, extended);
      setLog((l) => [...l, { role: "prime", turn }]);
      void refreshAi();
    } catch (e) {
      setLog((l) => [
        ...l,
        { role: "error", text: e instanceof Error ? e.message : "Could not continue the agent loop" },
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
      <div className="prime-hint muted">{PRIME_HINT}</div>
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
            <PrimeTurnCard
              key={i}
              turn={m.turn}
              busy={busy}
              onSuggestion={handleSuggestion}
              onContinue={continueLoop}
            />
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
          placeholder={PRIME_PLACEHOLDER}
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

      {/* The inventory of tools Prime can actually RUN from chat (installed plugins +
          governed command tools + live MCP), so "I installed a plugin — can Prime use
          it?" has a visible, honest answer. Collapsed + lazy-loaded so it never blocks
          the chat or pays the MCP discovery cost until opened (docs/prime-tool-use.md). */}
      <PrimeToolInventoryPanel />

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

// The inventory of tools Prime can actually run from chat — installed plugin /
// governed-command / built-in tools (ready or needs-approval) PLUS the live tools of
// every enabled MCP server, as returned by GET /v1/relux/prime/tools. This is the
// EXACT runnable catalog the agent loop offers Prime's brain, so a tool listed here is
// one a user can ask Prime to use in chat ("use the readme summarizer on this repo").
// Honest by construction: a tool Prime cannot run is never listed; a `gated` tool needs
// an approval (or a standing allow-always grant) before it runs. Lazy-loaded on first
// open so the chat is never blocked and the live MCP discovery cost is only paid on
// demand (docs/prime-tool-use.md; RELUX_MASTER_PLAN §10.1/§10.5/§17.1).
function PrimeToolInventoryPanel() {
  const [tools, setTools] = useState<ReluxPrimeToolView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const loadedRef = useRef(false);

  async function load() {
    if (loading) return;
    setLoading(true);
    setError(null);
    try {
      setTools(await reluxPrime.tools());
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load tools");
    } finally {
      setLoading(false);
    }
  }

  return (
    <details
      className="prime-advanced"
      onToggle={(e) => {
        if (e.currentTarget.open && !loadedRef.current) {
          loadedRef.current = true;
          void load();
        }
      }}
    >
      <summary>🧰 Tools Prime can use</summary>
      <div className="prime-advanced-body">
        <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
          <div className="muted" style={{ fontSize: 12 }}>
            Tools Prime can run from chat — installed plugins, governed command tools, and live MCP
            tools. Ask Prime to use one (e.g. <span className="mono">use {tools?.[0]?.label ?? "the tool"}</span>).
            A <span className="badge in_review" style={{ fontSize: 9 }}>needs approval</span> tool
            pauses for your OK before it runs.
          </div>
          <button className="btn ghost" disabled={loading} onClick={() => void load()}>
            {loading ? "..." : "Refresh"}
          </button>
        </div>
        {error && (
          <div className="banner err" style={{ marginTop: 8 }}>
            {error}. Make sure <span className="mono">relux-kernel serve</span> is running.
          </div>
        )}
        {tools && tools.length === 0 && !error && (
          <div className="muted" style={{ marginTop: 8, fontSize: 12 }}>
            No runnable tools yet. Install a plugin and configure its command tool or register an MCP
            server from the <Link to="/plugins">Plugins</Link> page, then it will appear here
            and Prime can use it.
          </div>
        )}
        {tools && tools.length > 0 && (
          <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 6 }}>
            {tools.map((t) => (
              <div
                key={t.label}
                className="row wrap"
                style={{ gap: 6, alignItems: "baseline", fontSize: 12 }}
              >
                <span className="badge" style={{ fontSize: 9 }}>
                  {t.source}
                </span>
                <span className={`badge ${t.gated ? "in_review" : "done"}`} style={{ fontSize: 9 }}>
                  {t.gated ? "needs approval" : "ready"}
                </span>
                <span className="mono">{t.label}</span>
                <span className="muted">risk={t.risk}</span>
                {t.description && <span className="muted">— {t.description}</span>}
              </div>
            ))}
          </div>
        )}
      </div>
    </details>
  );
}

// The tool a Prime turn actually ran (with its real JSON output) or the honest
// reason a requested tool did NOT run. Rendered straight from the turn — the UI
// never fabricates a tool result. Nothing renders for a turn that touched no tool.
function ToolResult({ turn }: { turn: ReluxPrimeTurn }) {
  if (turn.invoked_tool) {
    // Chat-natural, bounded rendering of the shaped result — never the raw envelope.
    const output = formatToolOutput(turn.tool_output);
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

// The compact trace of the tools Prime called inside the bounded AGENT LOOP this turn — one chip
// per real, gated, audited execution, in order. Each chip carries the tool label, a source badge
// (mcp / plugin), and the one-line summary on hover; an errored call is marked. Rendered straight
// from the turn's `tool_trace` — the UI fabricates nothing. Nothing renders when the loop ran a
// single tool (that is already shown by ToolResult) or no tools at all.
function ToolTrace({ turn }: { turn: ReluxPrimeTurn }) {
  const trace = turn.tool_trace;
  // A single execution is already surfaced by ToolResult; the trace strip is for a real CHAIN.
  if (!trace || trace.length < 2) return null;
  return (
    <div style={{ marginTop: 8 }}>
      <div className="row wrap" style={{ gap: 6, alignItems: "center", fontSize: 11 }}>
        <span className="badge done" style={{ fontSize: 9 }} title="Tools Prime called in sequence this turn">
          🛠 {trace.length} tool steps
        </span>
        {trace.map((t, i) => (
          <span
            key={i}
            className={"badge " + (t.ok ? "done" : "blocked")}
            style={{ fontSize: 9 }}
            title={`${t.source} · ${t.ok ? "ok" : "error"} — ${t.summary}`}
          >
            {t.source === "mcp" ? "mcp" : "tool"} · <span className="mono">{t.label}</span>
          </span>
        ))}
      </div>
    </div>
  );
}

function AiStatusBanner({ status }: { status: ReluxAiStatus | null }) {
  if (!status) return null;
  const brain = status.brain ?? "local";
  let icon = "🤖";
  let label = "Prime: Local (deterministic)";
  const auto = status.auto_detected ? " · auto-detected" : "";
  if (brain === "openrouter") {
    icon = "✨";
    label = status.configured ? `Prime: OpenRouter (${status.model})` : "Prime: OpenRouter (no key)";
  } else if (brain === "claude_cli") {
    icon = "✦";
    label = `Prime: Claude CLI${auto}`;
  } else if (brain === "codex_cli") {
    icon = "✦";
    label = `Prime: Codex CLI${auto}`;
  }
  // Prime is on the deterministic Local fallback: say so plainly and make the
  // one-click path to a real brain obvious (the chief first-run pain — Prime
  // listed adapters but it was unclear how to actually power it).
  const onFallback = brain === "local";
  return (
    <div className="row wrap muted" style={{ gap: 8, fontSize: 10, padding: "4px 8px", borderBottom: "1px solid var(--border)", marginBottom: 8, alignItems: "center" }} title={status.reason}>
      <span>{icon} {label}</span>
      {onFallback && <span className="badge backlog" style={{ fontSize: 8 }}>fallback / test</span>}
      {status.disabled && status.configured && <span className="badge todo" style={{fontSize: 8}}>LLM disabled</span>}
      <div className="spacer" style={{ flex: 1 }} />
      {onFallback ? (
        <Link to="/health" className="link" style={{ fontSize: 10, fontWeight: 600 }} title="Set up a real brain (Claude CLI / Codex CLI / OpenRouter) and test it">
          Set up a real brain →
        </Link>
      ) : (
        <Link to="/health" className="link" style={{ fontSize: 10 }} title="Choose Prime's brain (Local / OpenRouter / Claude CLI / Codex CLI)">
          Prime Brain settings →
        </Link>
      )}
    </div>
  );
}

// One Prime turn rendered as a compact card: the reply text, an intent +
// disposition chip, and any durable artifact (task created, run started, or an
// approval that is now pending). All of it is read straight from the turn — the
// UI never fabricates an outcome Prime did not report. Exported so the
// approval-continuation render test can seed a paused-on-approval turn directly
// (a first-paint Prime render cannot stage one — useEffect never fires under
// renderToStaticMarkup).
export function PrimeTurnCard({
  turn,
  busy,
  onSuggestion,
  onContinue,
}: {
  turn: ReluxPrimeTurn;
  busy: boolean;
  onSuggestion: (s: ReluxPrimeSuggestion) => void;
  onContinue: (id: string, extended: boolean, label?: string) => void;
}) {
  const tone = DISPOSITION_TONE[turn.disposition] ?? "todo";
  const suggestions = turn.suggested_actions ?? [];
  const continuation = turn.prime_continuation;
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

      <ToolTrace turn={turn} />

      {/* Resumable agent-loop continuation: the bounded loop paused with work still to do (a
          configured autonomy ceiling was reached, or a gated tool is waiting on approval). The
          "Keep working" button RESUMES that exact loop from the already-gathered observations via
          the continuation route — it does NOT re-send the original text, and it never repeats a
          completed tool call. When the loop is waiting on a tool approval, approve it on the card
          first; continuing then folds the approved result back in. (docs/mcp.md "Prime Agent
          Loop"; §10.5, §17.1) */}
      {continuation && (
        <div
          className="row wrap"
          style={{ gap: 8, marginTop: 10, alignItems: "center", fontSize: 11 }}
        >
          <span
            className="badge todo"
            style={{ fontSize: 9 }}
            title={`Paused: ${continuation.reason}. ${continuation.observation_count} tool result(s) gathered so far.`}
          >
            ⏸ paused · {continuation.reason} · {continuation.observation_count} gathered
          </span>
          {continuation.awaiting_approval ? (
            <span className="muted" style={{ fontSize: 10 }}>
              Approve the tool above — I'll continue automatically with its result.
            </span>
          ) : (
            <button
              className="btn"
              style={{ fontSize: 12, padding: "4px 12px" }}
              disabled={busy}
              onClick={() => onContinue(continuation.id, true)}
              title="Resume this loop from where it paused, with the extended (long-work) limits — continues from the gathered results, not a re-run"
            >
              Keep working (extended)
            </button>
          )}
        </div>
      )}

      {/* A pending per-call tool approval Prime staged because an explicit chat tool
          invocation named a gated (needs_approval) tool with no standing grant. The
          card drives the EXISTING approval routes (approve once → execute, allow
          always, deny) — Prime ran nothing by showing it, and nothing is auto-approved
          (docs/mcp.md "Invocation"; §7.4). */}
      {turn.pending_tool_approval && (
        <ApprovalCard
          request={turn.pending_tool_approval}
          busy={busy}
          continuationId={continuation?.awaiting_approval ? continuation.id : undefined}
          onContinue={onContinue}
        />
      )}

      {/* A GitHub plugin-import Prime proposed this turn ("install owner/repo as a
          plugin"). The card shows the canonical source, the destination, and the
          no-code-run guarantee, then confirms before doing anything. Confirm posts to the
          single backend-governed action route (POST /v1/relux/prime/actions/install-plugin),
          which re-validates server-side, runs the existing install + read-only candidate
          scan, and closes the logged approval — no new authority — then renders the
          installed plugin + detected candidates with Configure / Open Plugins links. Deny
          just rejects the logged approval. Nothing installs by showing it
          (RELUX_MASTER_PLAN §8/§10.2/§10.3; docs/plugins.md). */}
      {githubPluginInstallAction(turn.action) && turn.disposition === "awaiting_approval" && (
        <PluginInstallCard
          install={githubPluginInstallAction(turn.action)!}
          approvalId={turn.approval}
          busy={busy}
        />
      )}

      {/* A capability ACTIVATION Prime proposed this turn ("configure the first
          candidate", "enable the MCP server from <plugin>", "turn that script into a
          tool"). Confirm posts to the single backend-governed action route
          (POST /v1/relux/prime/actions/configure-candidate), which re-reads the plugin's
          candidates server-side, re-resolves the selection, and activates through the
          EXISTING governed path (register the MCP server, or configure a command tool) —
          metadata/recipe only, no source code runs, and the resulting tool stays gated
          until invoked. Cancel just rejects the logged approval (RELUX_MASTER_PLAN
          §8/§8.2/§10.2/§10.3; docs/prime-tool-use.md). */}
      {configurePluginCandidateAction(turn.action) && turn.disposition === "awaiting_approval" && (
        <ConfigureCandidateCard
          action={configurePluginCandidateAction(turn.action)!}
          approvalId={turn.approval}
          busy={busy}
        />
      )}

      {/* The reviewable plan proposal (RELUX_MASTER_PLAN §10 planning layer, §11.1):
          a compact card showing the proposed shape — goal, steps, roles, and the
          agents work would land on. It is informational only; nothing runs from
          showing it. The explicit commit is the "Create these tasks" button below
          (from suggested_actions), so the card never acts on its own (§10.5, §17.1). */}
      {turn.proposal && <ProposalCard proposal={turn.proposal} />}

      {/* The reviewable MULTI-TOOL plan proposal (docs/mcp.md "Run-driven multi-tool
          plan"): a compact card showing the grounded tool steps, each step's
          readiness/risk, and a compact args preview. It is INERT — showing it creates
          and runs nothing. The explicit "Create tool-run task" button inside the card
          is the ONLY commit path; it POSTs the validated steps to the existing
          tool_plan task-create route, where the unchanged permission/approval/grant
          gates still apply at run time (§10.5, §17.1). */}
      {turn.tool_plan_proposal && <ToolPlanCard proposal={turn.tool_plan_proposal} busy={busy} />}

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
// A compact, chat-first pending-approval card for a gated tool call Prime staged
// (docs/mcp.md "Invocation"; RELUX_MASTER_PLAN §7.4 per-call approval). It shows the
// tool + source, the risk/reason, and a bounded, secret-redacted args preview, then
// offers exactly the decisions the existing approval machinery supports — "Approve &
// run" (decide → execute), "Allow always" (allow-always: persist a standing grant +
// execute), and "Deny" (decide:rejected, which drops the bound invocation). It calls
// ONLY the existing /v1/relux/approvals/:id/{decide,execute,allow-always} routes — it
// invents no parallel security path and never auto-approves. Mirrors openclaw's
// allow-once / allow-always / deny permission options (src/acp/permission-relay.ts).
function ApprovalCard({
  request,
  busy,
  continuationId,
  onContinue,
}: {
  request: ReluxPrimeToolApprovalRequest;
  busy: boolean;
  // When this approval paused an agent loop, the continuation token to resume once the tool ran.
  continuationId?: string;
  onContinue?: (id: string, extended: boolean, label?: string) => void;
}) {
  const [working, setWorking] = useState<null | "approve" | "always" | "deny">(null);
  const [outcome, setOutcome] = useState<
    null | { kind: "ran"; result: ReluxToolInvocationResult } | { kind: "denied" }
  >(null);
  const [err, setErr] = useState<string | null>(null);
  const continuedRef = useRef(false);
  const id = request.approval_id;
  const locked = busy || working !== null || outcome !== null;

  // After the operator approves and the gated tool RUNS, the kernel has already folded its result
  // into the paused continuation (execute_approved_tool_invocation → fold_approved_into_continuation
  // clears the pending-approval marker). So if this approval paused an agent loop, resume it ONCE —
  // automatically — so Prime continues with the real result and answers WITHOUT the operator typing
  // another prompt (the agentic approve → run → continue flow; docs/prime-tool-use.md). Idempotent
  // (continuedRef) and safe: the resume runs behind the same gates and never re-runs the completed
  // call (the loop skips it by signature). When there is no continuation (e.g. a non-loop approval),
  // the inline result below is the answer — never a dead-end.
  function resumeAfterRun() {
    if (continuedRef.current) return;
    if (!continuationId || !onContinue) return;
    continuedRef.current = true;
    onContinue(continuationId, false, "Continue with the approved tool result");
  }

  async function approveAndRun() {
    if (locked) return;
    setErr(null);
    setWorking("approve");
    try {
      // The exact two-step the Approvals page uses: decide(approved) then execute once.
      await reluxApprovals.decide(id, "approved");
      const result = await reluxApprovals.execute(id);
      setOutcome({ kind: "ran", result });
      resumeAfterRun();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not approve and run the tool");
    } finally {
      setWorking(null);
    }
  }

  async function allowAlways() {
    if (locked) return;
    setErr(null);
    setWorking("always");
    try {
      // allow-always approves AND persists a standing grant; then run the bound call once.
      await reluxApprovals.allowAlways(id);
      const result = await reluxApprovals.execute(id);
      setOutcome({ kind: "ran", result });
      resumeAfterRun();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not allow-always and run the tool");
    } finally {
      setWorking(null);
    }
  }

  async function deny() {
    if (locked) return;
    setErr(null);
    setWorking("deny");
    try {
      await reluxApprovals.decide(id, "rejected");
      setOutcome({ kind: "denied" });
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not deny the tool call");
    } finally {
      setWorking(null);
    }
  }

  // The same chat-natural, bounded shaping the ran-tool result uses — surface the
  // shaped result text directly, never the raw transport envelope.
  const ranOutput = outcome?.kind === "ran" ? formatToolOutput(outcome.result.output) : "";

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
        <span className="badge in_review" style={{ fontSize: 9 }} title="A gated tool call awaiting your decision — nothing ran yet">
          approval needed
        </span>
        {request.source === "mcp" && request.server ? (
          <span className="badge todo" style={{ fontSize: 8 }} title={`Live tool from MCP server "${request.server}"`}>
            MCP · {request.server}
          </span>
        ) : (
          <span className="badge backlog" style={{ fontSize: 8 }} title="An installed plugin tool">
            plugin
          </span>
        )}
        <span className="mono" style={{ fontSize: 13, fontWeight: 600 }}>{request.label}</span>
        <span className="badge backlog" style={{ fontSize: 9 }} title="Declared/derived risk of this tool">
          {request.risk}
        </span>
      </div>
      <div className="muted" style={{ fontSize: 11, marginBottom: 6 }}>{request.reason}</div>
      <div className="row wrap" style={{ gap: 6, fontSize: 10, marginBottom: 6 }}>
        <span className="mono muted" title="The permission this call requires">{request.permission}</span>
      </div>
      {request.args_preview && (
        <pre
          className="mono"
          style={{
            margin: "0 0 8px",
            padding: "6px 8px",
            fontSize: 11,
            maxHeight: 140,
            overflow: "auto",
            border: "1px solid var(--border)",
            borderRadius: 4,
            whiteSpace: "pre-wrap",
          }}
          title="A bounded, secret-redacted preview of the arguments — never the raw values"
        >
          {request.args_preview}
        </pre>
      )}

      {outcome === null ? (
        <div className="row wrap" style={{ gap: 8 }}>
          <button
            className="btn"
            style={{ fontSize: 12, padding: "4px 12px" }}
            disabled={locked}
            onClick={() => void approveAndRun()}
            title="Approve this single call and run it once through the existing per-call execute path"
          >
            {working === "approve" ? "Running…" : "Approve & run"}
          </button>
          {request.allow_always_supported && (
            <button
              className="btn"
              style={{ fontSize: 12, padding: "4px 12px" }}
              disabled={locked}
              onClick={() => void allowAlways()}
              title="Approve and persist a standing allow-always grant, then run it once — future matching calls skip the prompt"
            >
              {working === "always" ? "Running…" : "Allow always"}
            </button>
          )}
          <button
            className="btn ghost"
            style={{ fontSize: 12, padding: "4px 12px" }}
            disabled={locked}
            onClick={() => void deny()}
            title="Deny this call — it is dropped and can never run without a fresh approval"
          >
            {working === "deny" ? "Denying…" : "Deny"}
          </button>
        </div>
      ) : outcome.kind === "denied" ? (
        <div className="banner" style={{ fontSize: 11, margin: 0 }}>
          Denied — the call was dropped and will not run.
        </div>
      ) : (
        <div>
          <div className="banner" style={{ fontSize: 11, margin: 0 }}>
            Ran <span className="mono">{request.label}</span> once through the approved path.
            {continuationId && " Prime is continuing with the result…"}
          </div>
          {ranOutput && (
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
              {ranOutput}
            </pre>
          )}
        </div>
      )}
      {err && (
        <div className="banner err" style={{ fontSize: 11, marginTop: 8 }}>{err}</div>
      )}
      <div className="muted" style={{ fontSize: 10, marginTop: 8, fontStyle: "italic" }}>
        Nothing ran yet — your decision runs through the same permission/approval/grant/audit gates.
      </div>
    </div>
  );
}

// A GitHub plugin-import confirmation card (RELUX_MASTER_PLAN §8 Plugin Model,
// §10.2 Action Layer, §10.3 Approval Rules; docs/plugins.md). Prime PROPOSED the
// import behind a human approval; this card surfaces the canonical source, the
// destination, and the explicit no-code-run guarantee so the operator confirms with
// full context (mirroring Hermes's clone-then-confirm and openclaw's confirmation
// gate). Confirm posts to the SINGLE backend-governed action route
// (`POST /v1/relux/prime/actions/install-plugin`): the kernel re-validates the repo
// URL + proposed id server-side, runs the existing manifestless install + read-only
// candidate scan internally, and closes the logged governance approval — one auditable
// chokepoint instead of chaining install-github + hints + decide client-side. The card
// shows the installed plugin id/status, the detected candidate count, the honest next
// actions the kernel returned, and Configure / Open Plugins links. It grants no new
// authority and runs no plugin code.
function PluginInstallCard({
  install,
  approvalId,
  busy,
}: {
  install: { repoUrl: string; pluginId: string };
  approvalId: string | null;
  busy: boolean;
}) {
  const [working, setWorking] = useState<null | "confirm" | "deny">(null);
  const [outcome, setOutcome] = useState<
    null | { kind: "installed"; result: ReluxPrimeInstallPluginResult } | { kind: "denied" }
  >(null);
  const [err, setErr] = useState<string | null>(null);
  const locked = busy || working !== null || outcome !== null;

  async function confirm() {
    if (locked) return;
    setErr(null);
    setWorking("confirm");
    try {
      // ONE backend-governed chokepoint: the kernel re-validates the repo URL + proposed
      // id server-side, runs the existing manifestless install + read-only candidate scan
      // internally, and closes the logged governance approval — no client-side chaining of
      // install-github + hints + decide. Metadata only; no repository code runs.
      const result = await reluxPrime.installPluginFromGithub(
        install.repoUrl,
        install.pluginId,
        approvalId,
      );
      setOutcome({ kind: "installed", result });
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not import the plugin from GitHub");
    } finally {
      setWorking(null);
    }
  }

  async function deny() {
    if (locked) return;
    setErr(null);
    setWorking("deny");
    try {
      if (approvalId) await reluxApprovals.decide(approvalId, "rejected");
      setOutcome({ kind: "denied" });
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not cancel the import");
    } finally {
      setWorking(null);
    }
  }

  const candidateCount = outcome?.kind === "installed" ? outcome.result.candidate_count : 0;

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
        <span
          className="badge in_review"
          style={{ fontSize: 9 }}
          title="A GitHub plugin import awaiting your confirmation — nothing has been cloned yet"
        >
          import needed
        </span>
        <span className="badge backlog" style={{ fontSize: 8 }} title="Imported from a GitHub repository">
          GitHub
        </span>
        <span className="mono" style={{ fontSize: 13, fontWeight: 600 }}>{install.repoUrl}</span>
      </div>

      {/* Source / destination / guarantee — the full context the operator confirms. */}
      <div className="row wrap" style={{ gap: 6, fontSize: 10, marginBottom: 6 }}>
        <span className="mono muted" title="Proposed local plugin id (finalized by the installer)">
          → {install.pluginId}
        </span>
      </div>
      <ul className="muted" style={{ fontSize: 11, margin: "0 0 8px 16px", padding: 0 }}>
        <li>Clones the repository's metadata into your local managed plugins.</li>
        <li>
          <strong>No code from the repository runs on import.</strong> Its tools stay
          disabled until you configure them.
        </li>
        <li>Next: review the detected capability candidates, then configure a tool / runtime.</li>
      </ul>

      {!outcome && (
        <div className="row wrap" style={{ gap: 8 }}>
          <button
            className="btn"
            style={{ fontSize: 12, padding: "4px 12px" }}
            disabled={locked}
            onClick={confirm}
            title="Import this repository as a plugin (metadata only, no code run)"
          >
            {working === "confirm" ? "Importing…" : "Confirm import"}
          </button>
          <button
            className="chip"
            style={{ fontSize: 11, padding: "3px 10px" }}
            disabled={locked}
            onClick={deny}
            title="Cancel — reject the logged approval and import nothing"
          >
            Cancel
          </button>
        </div>
      )}

      {outcome?.kind === "installed" && (
        <div className="banner" style={{ fontSize: 11, marginTop: 4 }}>
          <div style={{ marginBottom: 4 }}>
            Imported <span className="mono" style={{ fontWeight: 600 }}>{outcome.result.plugin.id}</span>{" "}
            <span className="badge backlog" style={{ fontSize: 8 }}>
              {outcome.result.plugin.enabled ? "enabled" : "metadata only"}
            </span>
            {outcome.result.generated && (
              <span className="badge todo" style={{ fontSize: 8, marginLeft: 4 }} title="Relux scaffolded a metadata-only manifest because the repo had no relux-plugin.json">
                scaffolded
              </span>
            )}
            {outcome.result.no_code_executed && (
              <span className="badge backlog" style={{ fontSize: 8, marginLeft: 4 }} title="The import cloned metadata only — no repository code ran">
                no code run
              </span>
            )}
          </div>
          <div className="muted" style={{ marginBottom: 6 }}>
            {candidateCount > 0
              ? `${candidateCount} capability candidate${candidateCount === 1 ? "" : "s"} detected — configure one to make it runnable.`
              : "No runnable capability detected yet — open Plugins to add a tool definition or runtime."}
          </div>
          {outcome.result.next_actions.length > 0 && (
            <ul className="muted" style={{ fontSize: 11, margin: "0 0 6px 16px", padding: 0 }}>
              {outcome.result.next_actions.map((step, i) => (
                <li key={i}>{step}</li>
              ))}
            </ul>
          )}
          {/* Detected candidates, each with a one-click "Configure with Prime" button
              that posts to the backend-governed activation route (register the MCP
              server / configure the command tool). A manual candidate has no one-click
              path, so it points at the Plugins page instead — never a fake "ready". */}
          {outcome.result.candidates.length > 0 && (
            <div style={{ marginBottom: 6 }}>
              {outcome.result.candidates.map((c) => (
                <CandidateRow key={c.id} pluginId={outcome.result.plugin.id} candidate={c} />
              ))}
            </div>
          )}
          <div className="row wrap" style={{ gap: 8 }}>
            <Link className="btn" style={{ fontSize: 12, padding: "4px 12px" }} to="/plugins">
              Configure on Plugins
            </Link>
            <Link className="chip" style={{ fontSize: 11, padding: "3px 10px" }} to="/plugins">
              Open Plugins
            </Link>
          </div>
        </div>
      )}

      {outcome?.kind === "denied" && (
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          Import cancelled — nothing was cloned.
        </div>
      )}

      {err && <div className="banner err" style={{ fontSize: 11, marginTop: 8 }}>{err}</div>}

      {!outcome && (
        <div className="muted" style={{ fontSize: 10, marginTop: 8, fontStyle: "italic" }}>
          Nothing has been cloned yet — confirming runs the same gated import the Plugins page uses.
        </div>
      )}
    </div>
  );
}

// A human label + button copy for a detected candidate's activation. A one-click MCP
// register and a governed command tool each get a "Configure with Prime" button; an
// honest `manual` candidate has no one-click path (it points at the Plugins page).
function candidateActivationLabel(activation: string): { kind: string; button: string } | null {
  if (activation === "mcp_register") return { kind: "MCP server", button: "Configure with Prime (register MCP server)" };
  if (activation === "command_tool") return { kind: "command tool", button: "Configure with Prime (command tool)" };
  return null;
}

// One detected candidate in the install result: its kind/confidence + a one-click
// "Configure with Prime" button when it has a governed activation path, or honest
// "open Plugins" guidance for a manual pending candidate. The button calls the same
// backend-governed activation route the chat proposal uses.
function CandidateRow({ pluginId, candidate }: { pluginId: string; candidate: ReluxCapabilityCandidate }) {
  const act = candidateActivationLabel(candidate.activation);
  return (
    <div style={{ border: "1px solid var(--border)", borderRadius: 6, padding: "6px 8px", marginTop: 6 }}>
      <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 2 }}>
        <span className="mono" style={{ fontSize: 12, fontWeight: 600 }}>{candidate.title}</span>
        <span className="badge backlog" style={{ fontSize: 8 }} title="Detection confidence">{candidate.confidence}</span>
        {!act && (
          <span className="badge todo" style={{ fontSize: 8 }} title="No one-click activation — follow the next steps on the Plugins page">manual</span>
        )}
      </div>
      <div className="muted" style={{ fontSize: 10, marginBottom: act ? 6 : 2 }}>{candidate.rationale}</div>
      {act ? (
        <CandidateActivation
          pluginId={pluginId}
          candidateId={candidate.id}
          label={act.button}
          title={`Activate this ${act.kind} through the existing governed path — no source code runs`}
        />
      ) : (
        <Link className="chip" style={{ fontSize: 11, padding: "3px 10px" }} to="/plugins">Configure on Plugins</Link>
      )}
    </div>
  );
}

// One discovered MCP tool row: its name + a gated/runnable chip. Discovery LISTS tools
// only — nothing here invokes one. An unclassified tool reads as gated until classified.
function DiscoveredMcpToolRow({ tool }: { tool: ReluxToolDescriptor }) {
  const gated = tool.executable === "needs_approval";
  return (
    <li className="row wrap" style={{ gap: 6, alignItems: "baseline", marginBottom: 2 }}>
      <span className="mono" style={{ fontWeight: 600 }}>{tool.tool_name}</span>
      <span
        className={`badge ${gated ? "blocked" : "done"}`}
        style={{ fontSize: 8 }}
        title={gated ? "Gated — needs approval before it runs (classify it to change this)" : "Classified runnable"}
      >
        {gated ? "gated" : "runnable"}
      </span>
      {tool.description && (
        <span className="muted" style={{ fontSize: 10 }}>{tool.description.slice(0, 80)}</span>
      )}
    </li>
  );
}

// The guided post-activation discovery panel for an mcp_register result: what the
// freshly-registered server advertises (each tool still gated), or an honest "couldn't
// reach it / what's missing" message with the registered-server status. Turns "registered"
// into "here's what Prime can use" without the user driving a separate Discover.
function McpDiscoveryResult({ result }: { result: ReluxPrimeConfigureCandidateResult }) {
  const d = result.mcp_discovery;
  if (!d) return null;
  return (
    <div style={{ marginTop: 6 }}>
      <div className="row wrap" style={{ gap: 6, alignItems: "baseline", marginBottom: 2 }}>
        <span
          className={`badge ${d.reachable ? "done" : "blocked"}`}
          style={{ fontSize: 8 }}
          title={d.reachable ? "A tools/list probe reached the server" : "A tools/list probe could not reach the server yet"}
        >
          {d.reachable ? `${d.tool_count} tool${d.tool_count === 1 ? "" : "s"} found` : "not reachable yet"}
        </span>
        {d.reachable && d.gated_count > 0 && (
          <span className="badge blocked" style={{ fontSize: 8 }} title="Gated tools need approval before they run">
            {d.gated_count} gated
          </span>
        )}
      </div>
      {d.tools.length > 0 && (
        <ul style={{ listStyle: "none", padding: 0, margin: "2px 0 4px" }}>
          {d.tools.map((t) => (
            <DiscoveredMcpToolRow key={`${t.plugin_id}/${t.tool_name}`} tool={t} />
          ))}
        </ul>
      )}
      {!d.reachable && d.error && (
        <div className="muted mono" style={{ fontSize: 10, marginBottom: 4 }} title="The sanitized probe failure reason">
          {d.error}
        </div>
      )}
    </div>
  );
}

// The success view after a confirmed activation: the new tool / MCP server, the guided MCP
// discovery (for an mcp_register result), the honest "ask me to use it" next step, and a
// link. Exported so a render test can mount it directly with a fabricated result (the live
// component sets `result` from the POST, which a static render does not run). Nothing here
// invokes a tool — every discovered/configured tool stays gated until asked for.
export function CandidateActivationResult({ result }: { result: ReluxPrimeConfigureCandidateResult }) {
  return (
    <div className="banner" style={{ fontSize: 11, marginTop: 4 }}>
      <div style={{ marginBottom: 4 }}>
        Configured <span className="mono" style={{ fontWeight: 600 }}>{result.tool_name}</span>{" "}
        <span className="badge backlog" style={{ fontSize: 8 }}>
          {result.activation === "mcp_register" ? "MCP server" : "command tool"}
        </span>
        {result.no_code_executed && (
          <span className="badge backlog" style={{ fontSize: 8, marginLeft: 4 }} title="Activation registered metadata/recipe only — no source code ran">
            no code run
          </span>
        )}
      </div>
      <div className="muted" style={{ marginBottom: 6 }}>{result.next_step}</div>
      {result.activation === "mcp_register" && <McpDiscoveryResult result={result} />}
      <div className="row wrap" style={{ gap: 8 }}>
        <Link className="chip" style={{ fontSize: 11, padding: "3px 10px" }} to="/plugins">Open Plugins</Link>
      </div>
    </div>
  );
}

// Activate ONE detected candidate through the SINGLE backend-governed route
// (POST /v1/relux/prime/actions/configure-candidate), then show the result: the new
// tool / MCP server, the honest "ask me to use it" next step, and links. Shared by the
// install-result candidate rows and the chat proposal card. Optionally renders a Cancel
// that rejects the logged approval (the chat proposal case). Nothing runs by showing it;
// confirming registers metadata/recipe only and the resulting tool stays gated.
function CandidateActivation({
  pluginId,
  candidateId,
  label,
  title,
  approvalId,
  showCancel,
  busy,
}: {
  pluginId: string;
  candidateId: string;
  label: string;
  title: string;
  approvalId?: string | null;
  showCancel?: boolean;
  busy?: boolean;
}) {
  const [working, setWorking] = useState<null | "confirm" | "deny">(null);
  const [result, setResult] = useState<ReluxPrimeConfigureCandidateResult | null>(null);
  const [denied, setDenied] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const locked = !!busy || working !== null || result !== null || denied;

  async function go() {
    if (locked) return;
    setErr(null);
    setWorking("confirm");
    try {
      const r = await reluxPrime.configureCandidate(pluginId, candidateId, approvalId ?? null);
      setResult(r);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not configure the capability");
    } finally {
      setWorking(null);
    }
  }

  async function deny() {
    if (locked) return;
    setErr(null);
    setWorking("deny");
    try {
      if (approvalId) await reluxApprovals.decide(approvalId, "rejected");
      setDenied(true);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not cancel the activation");
    } finally {
      setWorking(null);
    }
  }

  if (result) {
    return <CandidateActivationResult result={result} />;
  }

  if (denied) {
    return <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>Activation cancelled — nothing was configured.</div>;
  }

  return (
    <div>
      <div className="row wrap" style={{ gap: 8 }}>
        <button className="btn" style={{ fontSize: 12, padding: "4px 12px" }} disabled={locked} onClick={go} title={title}>
          {working === "confirm" ? "Configuring…" : label}
        </button>
        {showCancel && (
          <button className="chip" style={{ fontSize: 11, padding: "3px 10px" }} disabled={locked} onClick={deny} title="Cancel — reject the logged approval and configure nothing">
            Cancel
          </button>
        )}
      </div>
      {err && <div className="banner err" style={{ fontSize: 11, marginTop: 6 }}>{err}</div>}
    </div>
  );
}

// A capability ACTIVATION Prime proposed from chat ("configure the first candidate",
// "enable the MCP server from <plugin>", "turn that script into a tool"). The card
// states what will be activated, where, and the no-code-run guarantee, then confirms
// before doing anything. Confirm posts to the single backend-governed action route,
// which re-reads + re-resolves the candidate server-side and activates through the
// existing governed path. Cancel rejects the logged approval. Nothing activates by
// showing it (RELUX_MASTER_PLAN §8/§8.2/§10.2/§10.3; docs/prime-tool-use.md).
function ConfigureCandidateCard({
  action,
  approvalId,
  busy,
}: {
  action: ConfigurePluginCandidateAction;
  approvalId: string | null;
  busy: boolean;
}) {
  const what =
    action.candidateId === "mcp"
      ? "register the detected MCP server"
      : action.candidateId === "command"
        ? "configure the detected command tool"
        : `configure the detected capability "${action.candidateId}"`;
  const where = action.pluginId ? `plugin ${action.pluginId}` : "the imported plugin";
  return (
    <div style={{ marginTop: 10, border: "1px solid var(--border)", borderRadius: 6, padding: "10px 12px" }}>
      <div className="row wrap" style={{ gap: 6, alignItems: "center", marginBottom: 4 }}>
        <span className="badge in_review" style={{ fontSize: 9 }} title="A capability activation awaiting your confirmation — nothing is configured yet">
          confirm needed
        </span>
        <span className="badge backlog" style={{ fontSize: 8 }} title="Activates a detected capability through the existing governed path">
          configure
        </span>
        <span style={{ fontSize: 13, fontWeight: 600 }}>I can {what} from {where}.</span>
      </div>
      <ul className="muted" style={{ fontSize: 11, margin: "0 0 8px 16px", padding: 0 }}>
        <li><strong>No code from the source runs.</strong> Activation registers metadata/recipe only.</li>
        <li>The resulting tool stays gated (needs approval) until you ask me to use it.</li>
      </ul>
      <CandidateActivation
        pluginId={action.pluginId}
        candidateId={action.candidateId}
        label="Configure with Prime"
        title="Activate this capability through the existing governed path — no source code runs"
        approvalId={approvalId}
        showCancel
        busy={busy}
      />
    </div>
  );
}

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

// Tone for a tool-plan step's readiness badge — grounded in the kernel's honest
// executability label (never optimistic).
const READINESS_TONE: Record<string, string> = {
  ready: "done",
  needs_approval: "in_review",
  missing_permission: "backlog",
  not_runnable: "backlog",
  unknown: "err",
  // A referenced MCP server/tool that was not reachable on the live `tools/list`.
  unavailable: "err",
};

// A short label for a tool-plan step's readiness badge.
const READINESS_LABEL: Record<string, string> = {
  ready: "ready",
  needs_approval: "needs approval",
  missing_permission: "needs permission",
  not_runnable: "not runnable",
  unknown: "unknown tool",
  unavailable: "unavailable",
};

// Render a step's compact args preview without leaking a giant blob into the chat.
function compactArgs(args: unknown): string {
  if (args == null) return "";
  let s: string;
  try {
    s = JSON.stringify(args);
  } catch {
    return "";
  }
  if (s === "{}" || s === "null") return "";
  return s.length > 80 ? `${s.slice(0, 79)}…` : s;
}

// A compact, B&W MULTI-TOOL plan proposal card (docs/mcp.md "Run-driven multi-tool
// plan"). It renders STRICTLY what Prime's grounded preview carried — a summary, the
// ordered tool steps with their resolved plugin/tool, readiness/risk, and a compact
// args preview — plus any issues that block creation. The card is INERT until the
// operator clicks "Create tool-run task", which POSTs the validated steps to the
// EXISTING tool_plan task-create route (reluxWork.createTask); the usual
// permission/approval/grant gates still apply at run time. The card invents no step
// (§10.5, §17.1).
function ToolPlanCard({ proposal, busy }: { proposal: ReluxPrimeToolPlanProposal; busy: boolean }) {
  const [creating, setCreating] = useState(false);
  const [createdId, setCreatedId] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function createToolRunTask() {
    if (creating || createdId) return;
    setErr(null);
    setCreating(true);
    try {
      const tool_plan = proposal.steps.map((s) => ({
        plugin: s.plugin,
        tool: s.tool,
        args: s.args ?? {},
      }));
      const title = `Tool plan: ${proposal.goal}`.slice(0, 120);
      const task = await reluxWork.createTask(title, { tool_plan });
      setCreatedId(task.id);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not create the tool-run task");
    } finally {
      setCreating(false);
    }
  }

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
        <span
          className="badge todo"
          style={{ fontSize: 9 }}
          title="A reviewable multi-tool plan — nothing is created or run yet"
        >
          tool plan preview
        </span>
        <span className="mono" style={{ fontSize: 13, fontWeight: 600 }}>
          {proposal.goal}
        </span>
      </div>
      <div className="muted" style={{ fontSize: 11, marginBottom: proposal.steps.length > 0 ? 8 : 0 }}>
        {proposal.summary}
      </div>
      {proposal.steps.length > 0 && (
        <ol style={{ margin: 0, paddingLeft: 0, listStyle: "none" }}>
          {proposal.steps.map((s) => {
            const args = compactArgs(s.args);
            // An MCP-backed step is namespaced under a `mcp:<server>` synthetic plugin
            // id; surface the source server explicitly so the operator sees it came from
            // a live MCP server, not an installed plugin (docs/mcp.md "Run-driven
            // multi-tool plan").
            const mcpServer = s.plugin.startsWith("mcp:") ? s.plugin.slice("mcp:".length) : null;
            return (
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
                <span className="mono" style={{ flex: 1, minWidth: 160 }}>
                  {mcpServer && (
                    <span
                      className="badge todo"
                      style={{ fontSize: 8, marginRight: 6 }}
                      title={`Live tool from MCP server "${mcpServer}"`}
                    >
                      MCP · {mcpServer}
                    </span>
                  )}
                  {s.tool ? `${s.plugin}/${s.tool}` : s.plugin}
                  {args && (
                    <span className="muted" style={{ marginLeft: 6, fontWeight: 400 }}>
                      {args}
                    </span>
                  )}
                  {s.note && (
                    <span className="muted" style={{ display: "block", fontSize: 10, fontWeight: 400 }}>
                      {s.note}
                    </span>
                  )}
                </span>
                {s.risk && (
                  <span className="badge backlog" style={{ fontSize: 9 }} title="Declared risk level of this tool">
                    {s.risk}
                  </span>
                )}
                <span
                  className={`badge ${READINESS_TONE[s.readiness] ?? "backlog"}`}
                  style={{ fontSize: 9 }}
                  title="Grounded against the live tool registry — what the run would actually do"
                >
                  {READINESS_LABEL[s.readiness] ?? s.readiness}
                </span>
              </li>
            );
          })}
        </ol>
      )}
      {/* Anything that blocks creation, surfaced honestly (an unknown tool, a
          not-runnable step, too many steps). An unknown tool is never silently
          accepted — the commit stays disabled until the plan is clean (§17.1). */}
      {proposal.issues && proposal.issues.length > 0 && (
        <PolishNotes label="Before this can be created" items={proposal.issues} />
      )}
      <div style={{ marginTop: 10 }}>
        {createdId ? (
          <div className="banner" style={{ fontSize: 11, margin: 0 }}>
            Created tool-run task <span className="mono">{createdId}</span> —{" "}
            <Link to={workTaskHref(createdId)}>open it in Work</Link> to start it. It runs each step
            through the usual gates.
          </div>
        ) : (
          <button
            className="btn"
            style={{ fontSize: 12, padding: "4px 12px" }}
            disabled={busy || creating || !proposal.ready_to_create}
            onClick={() => void createToolRunTask()}
            title={
              proposal.ready_to_create
                ? "Create a tool-run task from these steps (nothing runs until you start it)"
                : "Resolve the issues above before this plan can be created"
            }
          >
            {creating ? "Creating…" : "Create tool-run task"}
          </button>
        )}
        {err && (
          <div className="banner err" style={{ fontSize: 11, marginTop: 8 }}>
            {err}
          </div>
        )}
      </div>
      {/* The honest contract: showing this commits nothing. Only the explicit button
          above materializes a task, and even then nothing RUNS until it is started —
          the unchanged permission/approval/grant gates still apply at run time. */}
      <div className="muted" style={{ fontSize: 10, marginTop: 8, fontStyle: "italic" }}>
        Nothing is created or run yet — the button above creates a task you start when ready.
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
