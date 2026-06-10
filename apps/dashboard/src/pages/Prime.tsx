import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { reluxAi, reluxPrime, type ReluxAiStatus, type ReluxPrimeTurn } from "../api";
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

  return (
    <div className="chat" style={{ height: "calc(100vh - 96px)" }}>
      <AiStatusBanner status={aiStatus} />
      <div style={{ padding: "0 10px 10px" }}>
        <PrimeAutonomyPanel />
      </div>
      <div style={{ padding: "0 10px 10px" }}>
        <OrchestrationPanel />
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
          return <PrimeTurnCard key={i} turn={m.turn} />;
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
      </div>
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
function PrimeTurnCard({ turn }: { turn: ReluxPrimeTurn }) {
  const tone = DISPOSITION_TONE[turn.disposition] ?? "todo";
  return (
    <div className="msg assistant" style={{ maxWidth: 720 }}>
      <div className="row wrap" style={{ gap: 6, marginBottom: 6, alignItems: "center" }}>
        <span className="badge todo" style={{ fontSize: 9 }} title="What Prime understood">
          {turn.intent.replace(/_/g, " ")}
        </span>
        <span className={"badge " + tone} style={{ fontSize: 9 }} title="How the turn resolved">
          {turn.disposition.replace(/_/g, " ")}
        </span>
        <span className="muted" style={{ fontSize: 9, marginLeft: "auto" }} title="Which provider produced this reply">
          {providerLabel(turn.ai_mode)}
        </span>
      </div>
      <div style={{ whiteSpace: "pre-wrap" }}>{turn.reply}</div>

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

      {(turn.created_task || turn.started_run || turn.created_agent || turn.approval) && (
        <div className="row wrap" style={{ gap: 10, marginTop: 10, fontSize: 11 }}>
          {turn.created_task && (
            <span className="muted">
              task <Link to="/work" className="mono" title="View on work board">{turn.created_task}</Link>
            </span>
          )}
          {turn.started_run && (
            <span className="muted">
              run <Link to="/work" className="mono" title="View execution history">{turn.started_run}</Link>
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
    </div>
  );
}
