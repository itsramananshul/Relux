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

## Applied change (reflect-and-clarify, second slice)

`relux-kernel/src/prime.rs` — two more `Clarify` arms emitted a fixed prompt that
ignored what the user already said. Per §10.5 and pattern (3), they now reflect the
parsed target/goal back, the same shape as `brainstorm_reply`:

- **`Orchestration` single-step** now calls `orchestration_clarify(&goal)`. When a
  coordination request does not actually split, it quotes the already-stripped
  `orchestration_goal` back ("\"summarizing the README\" reads like a single piece
  of work …") and asks for the distinct steps, instead of a generic nudge. It falls
  back to the old generic prompt when the recovered goal is not a nameable phrase
  (a lone word, or the whole message when nothing stripped), so a bare "orchestrate
  this" is never quoted back as if it named work.
- **`TaskUpdate`** now calls `task_update_clarify(message)`, which reflects whatever
  the message already named — the target task id (`extract_task_id`) and/or the
  field being changed (`update_change_phrase`: priority / title / assignee /
  status) — and asks only for the missing piece. The no-info case still clarifies
  but enumerates the editable fields instead of the old bare two-part question.

Both stay a `PrimePlan::Clarify`: the deterministic classifier still owns the
action decision and the action-free wall is intact. `TaskUpdate` has no
`UpdateTask` action wired today, so the reflection never claims to apply an edit —
it only asks for the value. Pinned by `orchestration_clarify_reflects_the_parsed_goal`
and `task_update_clarify_reflects_target_and_field`.

## Applied change (idea → plan → tasks rung)

The brainstorm work left a two-rung ladder: an idea (`Brainstorming`, a
conversation) could only jump straight to a single task ("Turn this into a
task"), or the user had to know the magic phrase "orchestrate" to fan a goal
into briefs — and that orchestration phrase *immediately* mints work. Per master
plan §10 (Prime "needs an intent layer, a planning layer, and an action layer"),
§10.5, §11.1, and §17.1 ("Prime must not blindly turn every message into a
plan"), there is now an explicit **middle rung**: an idea becomes a *reviewable
plan* before any task exists.

- **New intent `PrimeIntent::PlanRequest`** (`relux-core`) — recognized for
  explicit plan phrasing ("plan this out", "make a plan to …", "draft a plan for
  …", "plan out …"). Classified **after** `Orchestration` (so "plan and assign"
  still commits) and **before** task creation (so "make a plan to build X"
  previews instead of minting one task); the same plan phrases are added to
  `is_explicit_command` so an ideation lead-in + an explicit plan ask escapes
  `Brainstorming` and reaches the rung.
- **`decide()` is action-free for `PlanRequest`** (`relux-kernel/src/prime.rs`):
  it runs the pure planner (`plan_orchestration`) and returns a `PrimePlan::Reply`
  — a plan **preview** that lists the proposed steps/agents and states *nothing is
  created yet*, or, for a goal that does not genuinely split, steers to the
  one-task path. No `Act`, no `Propose`: the preview mints and runs nothing.
- **Explicit one-click commit** (`attach_suggestions`, `state.rs`): a multi-step
  preview offers "Create these tasks" → `orchestrate <goal>` (routing the
  EXISTING, unchanged orchestration `Act`); a single-step goal offers "Turn this
  into a task". `plan_goal` is the shared strip so the previewed and committed
  plans decompose from identical input. Brainstorming additionally gains a "Plan
  this out" button (→ `plan out <idea>`), so musing flows into a plan without a
  magic phrase. Every suggestion is still just a pre-written user message
  (`send: false`) — a button can do nothing the user could not type, and nothing
  is created until they send it.

The orchestration `Act` path (the commit target) and the formal approval-id
`Propose` machinery are untouched. Pinned by `classifies_plan_requests`,
`plan_request_previews_a_multi_step_plan_without_creating`,
`plan_request_single_step_steers_to_one_task`, and
`plan_goal_round_trips_with_orchestration`.

## Applied change (structured plan proposal on the wire + dashboard card)

The plan-request rung above shipped the *preview as prose* in `PrimeTurn.reply`
plus a "Create these tasks" suggestion; the dashboard could only render the prose
and a generic button. Per master plan §11.1 (Prime Chat shows "plugin/action
results" and "suggested next actions") and §10 (planning layer), a plan now also
rides the wire as STRUCTURED, action-free data so the chat renders a real
proposal card instead of parsing text.

- **New `PrimeProposal` / `PrimeProposalStep`** (`relux-core`) — descriptive only:
  `goal`, `multi_step`, ordered `steps` (1-based `index`, `title`, role `label`,
  and the `agent` each would land on — `"prime"` when no specialist fits), and the
  distinct `agents`. There is **no `PrimeAction`** in a proposal: it is a preview,
  not a command.
- **`PrimeTurn.proposal: Option<PrimeProposal>`** with `skip_serializing_if =
  "Option::is_none"` — present **only** on a `PlanRequest` turn, omitted on every
  other turn, so existing clients see exactly the JSON they did before.
- **Built in `attach_suggestions`** (`relux-kernel/src/state.rs`) from the SAME
  `plan_orchestration(&goal, summary)` the "Create these tasks" suggestion is keyed
  on, so the card shows exactly what the commit would create. A single-step goal
  gets a `multi_step: false` proposal with no steps (the card names the goal and
  the one-task route honestly, without fanning out).
- **Dashboard `ProposalCard`** (`apps/dashboard/src/pages/Prime.tsx` + pure helpers
  in `src/prime.ts`) — a compact B&W card: goal heading, a `N steps across M
  agents` summary, and the proposed steps with their role + assignee. The card
  commits nothing; the explicit "Create these tasks" / "Turn this into a task"
  button (still from `suggested_actions`) is the lone commit path. No echo, no
  auto-run.

Pinned by `prime_proposal_round_trips_and_carries_only_descriptive_data` and the
extended `prime_suggestion_round_trips_and_is_omitted_when_empty` (core wire
guard: a non-plan turn omits `proposal`), `plan_request_attaches_a_structured_
action_free_proposal` and `plan_request_single_step_proposal_steers_to_one_task`
(kernel), and `apps/dashboard/test/prime.test.ts` (card helpers).

## Applied change (advisory LLM polish of the plan proposal — presentation only)

The structured card above is built entirely by the deterministic planner. When the
optional **OpenRouter** brain is enabled, it may now also refine the *wording* of
that card — a clearer summary, per-step titles, clarifying questions, advisory risk
notes — while the deterministic `plan_orchestration` stays the sole authority on
**step count, order, agent grounding, the `multi_step` flag, the `goal`, and the
commit action**. This mirrors the bridge/coordinator's "model drafts, server
validates" seam (`product-spine-implementation.md` → AI Prime Planner) but applied
to the relux-kernel `PrimeTurn.proposal` surface, and keeps the action-free wall
intact: the LLM gets ZERO action authority.

- **New `PrimeProposalPolish` / `PrimePolishedStep`** (`relux-core`) and an optional
  `PrimeProposal.polish` field (`skip_serializing_if = "Option::is_none"`, so the
  unpolished wire is byte-for-byte unchanged). The overlay carries only presentation
  strings: `summary`, `step_titles` (each keyed to a real step `index`),
  `questions`, `risks`, and the `model` for provenance. It is never read by any
  action/commit path.
- **`polish_proposal` + the pure `plan_polish` / `validate_polish` / `finalize_polish`**
  (`relux-kernel/src/ai.rs`). The brain is asked for a strict-JSON overlay, then the
  kernel **validates it against the authoritative proposal**: `step_titles` is
  accepted ONLY when the model's indexes match the real steps exactly (same count,
  same set, no duplicates, no extras) — any merge/split/reorder/add/rename drops the
  titles entirely and the deterministic titles stand; `questions`/`risks` are trimmed
  and count/length-bounded; a failed or unusable call attaches nothing. `polish_proposal`
  is the OpenRouter HTTP path.
- **CLI brains (Claude / Codex) now polish through the SAME chokepoint.**
  `compose_polish_prompt` folds the strict-JSON polish instruction + the authoritative
  steps into one stdin prompt (mirroring `compose_chat_prompt`); the kernel spawns the
  adapter in the same bounded, non-bypass mode as the conversational path
  (`polish_proposal_via_cli` in `server.rs`), lifts the reply out of the result
  envelope with `parse_adapter_result` (the same shape seam), and runs it through
  `polish_from_cli_text` → **the same `validate_polish`**. So the CLI can only ever
  change titles/questions/risks/provenance — never step count, order, or agent ids —
  and an error envelope / prose / timeout / missing adapter / invalid suggestion all
  leave the deterministic card in place with no user-facing failure. The shared
  `proposal_wants_polish` predicate skips single-step proposals for every brain.
- **Wired in `run_prime`** (`relux-kernel/src/server.rs`) OUTSIDE the lock, after the
  reply is shaped — so the slow model/process call never holds the kernel lock — and
  **gated on a non-actionful turn** (only a `PlanRequest` carries a proposal; the
  "Create these tasks" commit is a separate `Orchestration` turn with no proposal), so
  the commit path never invokes polish. A skip/error simply leaves the deterministic
  preview in place.
- **Dashboard** (`apps/dashboard`) — the `ProposalCard` shows the polished summary /
  step titles when present (falling back to the authoritative values via the pure
  `stepDisplayTitle` / `proposalDisplaySummary` helpers), an **"AI-refined wording ·
  <source>"** provenance badge, and advisory question/risk lists. The provenance is
  now **visible on the badge itself** (no longer hover-only) via the pure
  `polishProvenance` helper, which reads the SAME `polish.model` field stamped by the
  one `validate_polish` chokepoint: the **OpenRouter model id** (e.g.
  `anthropic/claude-3.5-haiku`) on the HTTP path, or the **CLI brain label**
  (`Claude CLI` / `Codex CLI`) on the adapter path — so a CLI polish now reads as
  cleanly as the OpenRouter one. An overlay from an older kernel that did not stamp
  `model` degrades to a generic "AI brain" label. The authoritative steps/agents/order
  are unchanged; the commit button is untouched.

Invariant (binding): the polish overlay is **advisory/presentation only**. It can
never change what "Create these tasks" creates (that re-runs `plan_orchestration` on
the unchanged `goal`), and it only ever exists on a non-actionful `PlanRequest` turn.

Pinned by `proposal_polish_is_advisory_and_omitted_when_absent` (core wire guard),
`plan_polish_skips_unless_openrouter_live_and_multi_step`,
`validate_polish_applies_titles_only_on_exact_index_match`,
`validate_polish_rejects_titles_that_change_count_order_or_agents`,
`validate_polish_bounds_questions_and_risks`,
`finalize_polish_attaches_model_on_success_and_none_on_error`,
`polish_proposal_skips_with_no_network_when_brain_is_not_live` (kernel), the CLI-brain
guards `compose_polish_prompt_carries_steps_and_no_structural_change_rule`,
`polish_from_cli_text_accepts_valid_json_and_stamps_label`,
`polish_from_cli_text_tolerates_prose_around_the_json`,
`polish_from_cli_text_ignores_malformed_or_objectless_text`,
`polish_from_cli_text_rejects_structural_drift_via_the_same_chokepoint` (ai.rs) and the
`cli_polish_*` seam tests (`server.rs`: valid envelope/plain JSON accepted, prose / error
envelope ignored, structural drift rejected, no-adapter → `None`), and the
`stepDisplayTitle` / `proposalDisplaySummary` / `polishProvenance` tests in
`apps/dashboard/test/prime.test.ts` (the last pins the OpenRouter model id, the
`Claude CLI`/`Codex CLI` labels, the no-overlay `null`, and the generic fallback when
`model` is unstamped). No test calls a real provider.

## Next recommended slice

When the optional LLM brain is enabled, let it *propose* the clarifying question
across the remaining reflect-and-clarify arms (brainstorm, orchestration, task
update) — the same "model suggests wording, deterministic classifier owns the
action" seam now used for the plan proposal — while keeping the action-free wall
intact. (Extending the advisory polish to the CLI brains (claude/codex) through the
same `validate_polish` chokepoint is now done — see above; surfacing the CLI brain's
provenance label on the card the way the OpenRouter model id already is, is now done
too — the `polishProvenance` helper renders either visibly on the badge.)
