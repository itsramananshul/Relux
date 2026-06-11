# Prime processing audit — lessons from Hermes & Paperclip

Status: implementation note (audit + applied change). Grounds Prime's request
processing in the two vendored references rather than inventing a model from
scratch. Pairs with `relix-hermes-integration.md`, `relix-agent-adapters.md`, and
`hermes-vs-paperclip-vs-relix.md` (strategic disposition) — this note is the
*request-processing* slice specifically.

## Files inspected

Hermes (Python agent runtime — `references/hermes-agent/`):
- `agent/conversation_loop.py` — `run_conversation(agent, user_message, …)`
  (def at line 351). The per-turn agentic loop: build request → call provider →
  parse tool calls → execute tools → re-call until the model stops, then persist
  the transcript. Returns a structured `Dict` (final response + message history),
  never raw text thrown over the wall.
- `agent/error_classifier.py` (`classify_api_error` → `FailoverReason`, used at
  `conversation_loop.py:2267`) — every provider error is *classified*
  (transient / permanent / policy / billing / unsupported) and routed to a
  bounded recovery, not surfaced as an opaque failure.

Paperclip (TypeScript control plane — `references/paperclip/`):
- `server/src/adapters/process/execute.ts` — `execute(ctx)`: spawn a local
  process, then return a typed `AdapterExecutionResult` carrying
  `exitCode` / `timedOut` / `errorMessage` / `resultJson{stdout,stderr}`. A
  non-zero exit or a timeout becomes an *honest structured result*, not a throw.
- `server/src/adapters/registry.ts` + `http/execute.ts` — one uniform
  "execute(Run) → stream events → report a structured result" contract behind a
  backend registry (`process` / `http` / CLI backends are siblings).

## Reusable patterns (what to keep)

1. **Classify before you act.** Hermes turns a turn into an explicit decision
   (tool call vs. final answer) before doing anything; errors are classified into
   a small enum before recovery. Relux already mirrors this: `classify_intent`
   (`relux-kernel/src/prime.rs`) is a single ordered, first-match pass that yields
   one `PrimeIntent`, and `parse_adapter_result` (`relux-core`) classifies adapter
   stdout into a structured summary. This is the seam the eventual LLM sits behind.

2. **Structured result envelope, never raw passthrough.** Paperclip's adapter and
   Hermes's loop both return typed results that *name* success, error, and
   metadata. Relux's `parse_adapter_result` + the two CLI-output seams (assigned
   run and Prime brain) keep raw JSON/stdout from leaking into the chat. Honest
   failure beats a blank or a wall of JSON — the same principle that fixed the
   Crew blank page (render a real loading/error/empty view, never a white screen).

3. **Ask one concrete clarifying question instead of guessing.** Hermes prefers a
   targeted follow-up over a low-confidence action. Relux already does this for
   ambiguous run control / assignment (the `Clarify` arm), and now does it for
   **brainstorming** too (see the applied change below).

4. **Conversation is action-free by design.** Hermes separates "answer" turns from
   "tool" turns. Relux enforces a hard wall: a chat/brainstorm turn is a
   `PrimePlan::Reply` and the brain path (`run_cli_brain`) only ever shapes a
   reply — a proposed_changes envelope from a chat turn is surfaced as advisory,
   never silently captured. This is the chat / plan / action mode separation the
   prompt calls for; it is preserved, not changed.

## Where Relux deliberately differs

- **Deterministic, not LLM-driven (today).** `decide()` is a pure function of
  `(message, StateSummary)` — no network, no wall clock. Hermes is LLM-first.
  Relux keeps the deterministic brain as the testable stand-in and the LLM as an
  optional reply-shaping path only (master plan §10, §15). The classifier is the
  documented seam where a model later slots in.
- **Governance is native, not embedded.** Risky intents (`PluginInstallation`,
  `PermissionChange`) become `Propose` behind a human approval — Paperclip's
  approval-gate model, transplanted into Rust rather than run as Paperclip.
- **Work is durable + grounded.** Every reply is grounded in the live
  `StateSummary`; Prime never invents completed work or pretends a plugin exists
  (§10.5). Hermes's per-turn memory is a learning loop Relux treats as a future
  transplant, not a request-processing dependency.

## Applied change (this slice)

`relux-kernel/src/prime.rs` — the `Brainstorming` arm returned a single fixed
prompt regardless of what the user said. Per §10.5 ("ask clarifying questions when
needed") and pattern (3) above, it now calls `brainstorm_reply(message)`, which:

- reflects the recovered topic (the same noun/verb phrase
  `brainstorm_task_candidate` extracts for the one-click suggestion — lead-ins
  stripped, quoted as a reflection, **not** a verbatim echo) and asks ONE concrete
  follow-up (desired outcome + a constraint to design around);
- falls back to the open-ended prompt when the message names no topic (pure
  connective musing);
- stays a `PrimePlan::Reply` — **nothing is created or run**, explicit task
  creation/orchestration are untouched, and the kernel still attaches the "turn
  this into a task" suggestion.

Pinned by `brainstorm_reply_reflects_the_topic_and_asks_a_clarifying_question`.

## Next recommended slice

Extend the same reflect-and-clarify shape to the `Orchestration` single-step
`Clarify` and `TaskUpdate` arms (echo the parsed goal/target back), and — when the
optional LLM brain is enabled — let it *propose* the clarifying question while the
deterministic classifier still owns the action decision (keep the action-free
wall intact).
