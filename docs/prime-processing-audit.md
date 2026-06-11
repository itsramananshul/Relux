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

## Applied change (conversation guard — questions & musing never mint work)

The classifier was still too action-happy: the broad task-creation catch matched a
work verb anywhere as a **substring**, so an informational question or a musing
that merely mentioned work was silently turned into a task. "how does the build
work?", "what's the best way to fix the flaky tests?", "we should refactor auth",
and even "show me a preview" (the substring "review") or "the prefix is wrong"
(the substring "fix") all minted a task. That is exactly the behavior §17.1 forbids
("Prime must understand conversational intent", "must not blindly turn every
message into a plan") and §10.5 rules out ("ask clarifying questions when needed",
do not create work from casual chat). This is the Hermes/Paperclip pattern (1)
"classify before you act" and (4) "conversation is action-free" applied one rung
deeper — the *question vs. command* boundary, not just the *ideation lead-in*
boundary the earlier slice drew.

`relux-kernel/src/prime.rs` — three tightenings in `classify_intent`, all gated so
an explicit command always wins:

- **Conversation guard.** A new `is_question(&m)` predicate (a wh-/auxiliary opener
  — how/what/why/should/is/do/… — or a trailing "?") routes a question with **no
  explicit command** to `Brainstorming`, *before* the task-creation catch. So a
  deliberative question answers conversationally and offers a one-click "turn this
  into a task" instead of minting one. `is_explicit_command` still overrides ("can
  you create a task to fix X?" acts), and status/explanation/tool questions are
  classified above the guard so it never swallows them. Polite directives
  ("can/could/would you …") are deliberately **not** question openers, so a clear
  request still acts.
- **Whole-word work verbs.** The task-creation catch now matches the `CREATION_VERBS`
  only as **whole words** (`has_word`), never substrings — "the prefix is wrong",
  "show me a preview", "the building plan", and "it fixes the crash" stop being read
  as new work, while "please fix the login bug" still creates it. `StatusQuestion`
  moved above the catch too, so "give me a status of the build" reports state instead
  of being read as work off "build".
- **Declarative soft-intent is musing.** `is_ideation` gained the soft-intent openers
  ("i want to", "i'd like to", "we should", "we could", "i think we", "maybe we",
  "let's", "what about/how about") so a stated wish stays a conversation unless an
  explicit command rides along ("let's create a task to X" still acts;
  "we should refactor auth" becomes a conversation). `brainstorm_task_candidate`
  learned the matching question/soft-intent strips so the "turn this into a task"
  pre-fill names the work cleanly ("what's the best way to fix the tests?" →
  `fix the tests`).

Every routed turn stays a `PrimePlan::Reply` (Brainstorming) — nothing is created or
run, and the kernel still attaches the "Turn this into a task" / "Plan this out"
suggestions, so the conversation flows into work in one explicit click. The
action-free wall and the deterministic-classifier-owns-the-action seam are intact;
the LLM brain (when live) still only shapes the *reply* for these conversational
turns. Pinned by `questions_about_work_stay_a_conversation_not_a_task`,
`soft_intent_musing_stays_a_conversation_not_a_task`,
`work_verbs_match_whole_words_not_substrings`,
`explicit_command_inside_a_question_still_acts`, and
`brainstorm_candidate_strips_question_and_soft_intent_lead_ins`.

## Applied change (brain-assisted, VALIDATED task-slot extraction)

Intent classification was already brain-mediated, but the *slots* of a created
task were still keyword string-slicing: `prime::task_title` strips a fixed lead-in
list and takes the remainder verbatim — no normalization, no details, no assignee,
no priority. So a polite, run-on request became a task titled after the whole
clause. Per master plan §10.1 (Intent Layer), §10.2 (Action Layer), and §17.1
("Prime must be smart and grounded"), and following the reference read recorded in
`reference-driven-development.md` (Hermes `coerce_tool_args` / argument sanitization;
openclaw `readPlanSteps` allowlist, `UNSUPPORTED_*_PARAM_KEYS`, existing-target
validation), a configured brain may now *propose* the slots, validated hard before
any task is created.

- **New module `relux-kernel/src/prime_slots.rs`.** `build_task_slots_prompt`
  demands JSON-only `{title, details?, assignee?, priority?, confidence}`.
  `parse_task_slots` lifts the JSON via the shared balanced-brace scanner
  (`prime_intent::extract_json_object`), **rejects any field outside the strict
  allowlist** (a smuggled `run_tool`/`tags`/`action` fails the whole proposal
  closed), requires a non-empty `title`, **sanitizes** every string (control chars
  stripped, title forced single-line) and **clamps** lengths, and **coerces**
  priority (number or numeric string → `[1,9]`, else dropped).
  `reconcile_task_slots` then validates against the live state: a low-confidence
  proposal falls back; an `assignee` is honored ONLY when it names an EXISTING
  agent (`summary.all_agent_ids`), else dropped; a proposal that merely echoes the
  deterministic title with nothing else is a no-op.
- **New wire field `PrimeTurn.slots: Option<PrimeTaskSlots>`** (`relux-core`),
  `skip_serializing_if = "Option::is_none"` so the wire is byte-for-byte unchanged
  on every turn the brain did not sharpen. Provenance/presentation only — the kernel
  validated every field before the task existed.
- **Kernel chokepoint.** `KernelState::prime_turn_with_intent_and_slots` reconciles
  the slot proposal for a create `Act` and threads `ResolvedTaskSlots` into
  `prime_execute`, which applies the title, folds details into the task input,
  assigns the validated agent (CreateTask only), and applies the clamped priority.
  `prime_turn_with_intent` is now a thin wrapper passing `None`, so the deterministic
  path is unchanged.
- **Safety invariants (binding).** Slots are computed only for a create intent the
  fail-closed gate already accepted, and only when the deterministic path already
  produced a real create — so **casual chat/ideation can never mint a task via
  slots** (the intent gate keeps it `Brainstorming`). The auto-run path
  (`CreateAndRunTask`) takes a brain title/details/priority but **never** the
  brain's assignee: the run stays on Prime, the only agent wired for the required
  grant. Any failure (no brain, low confidence, invalid JSON, unsupported field,
  unknown assignee) leaves the deterministic slots in place.
- **Both brains feed one validator.** OpenRouter goes through
  `ai::extract_task_slots_via_openrouter`; the Claude/Codex CLI brains are spawned
  in the same bounded, non-bypass mode and their stdout is lifted by
  `parse_adapter_result` FIRST (`server.rs` `extract_task_slots_via_cli` /
  `parse_cli_task_slots`) so the raw envelope never reaches the parser or the UI.
  Both land on the SAME `parse_task_slots` → `reconcile_task_slots`. The server gates
  the (slow, off-lock) slot call on the RESOLVED intent being a create, and stamps
  the provenance label (model id / `Claude CLI` / `Codex CLI`).
- **Dashboard.** A compact B&W slot card (`apps/dashboard/src/pages/Prime.tsx` +
  the pure `slotProvenance` helper in `src/prime.ts`) shows the normalized title,
  optional details, the honored assignee/priority, and a small `🧠 <source>`
  provenance chip — present ONLY when the kernel attached brain-assisted slots.

Pinned by `prime_task_slots_round_trip_and_omit_empty_optionals` (core wire guard,
plus the extended omission assertion in `prime_suggestion_round_trips_…`); the
`prime_slots` unit tests (clean parse, noisy-reply extraction, invalid JSON,
unsupported-field fail-closed, empty-title reject, overlong-title clamp + control
strip, priority coercion/clamp, reconcile low-confidence/known-and-unknown-assignee/
pure-echo/details+priority); the kernel integration tests
(`brain_slots_sharpen_a_created_task_…`, `brain_slot_assignee_is_honored_only_when_
the_agent_exists`, `no_slot_proposal_is_byte_for_byte_the_deterministic_create`,
`ideation_still_cannot_mint_a_task_even_with_a_slot_proposal`,
`create_and_run_sharpens_the_title_but_never_reassigns_the_run`); the server seam
tests (`cli_slots_*`: valid envelope, plain JSON, error envelope, prose-without-JSON,
unsupported-field fail-closed); and the dashboard `slotProvenance` test. No test
calls a real provider.

## Applied change (brain-assisted agent + admin slots)

The validated-slot layer now reaches past task creation to the next brittle Prime
paths flagged in the roadmap: **agent creation** (`derive_agent_name`), **plugin
identity** (`derive_plugin_id`), and **permission-subject extraction** (the
`message.contains("agent")` slice). Per master plan §10.1/§10.2/§10.3 and the
reference read recorded in `reference-driven-development.md` (Hermes
`coerce_tool_args` / sanitization; openclaw `sessions-spawn-tool` unsupported-key
rejection + required-string + clamp, `common.ts` `readStringParam`/`ToolInputError`,
`approval-classifier.ts` subject resolution + `normalizeToolName` + kind allowlists),
a configured brain may now *propose* these slots, validated hard before anything.

- **New module `relux-kernel/src/prime_agent_slots.rs`** (executable path). For an
  `AgentCreation` turn the brain proposes `{name, role?, adapter?, notes?, confidence}`.
  `parse_agent_slots` rejects any field outside the allowlist (fail closed), requires a
  non-empty name, and sanitizes/clamps every string. `reconcile_agent_slots` normalizes
  the name into an id (`agent_id_form`), **rejects a duplicate** (an id colliding with an
  existing agent fails the whole proposal — a create can never be reshaped into a clash),
  and honors an `adapter` ONLY when it names a live adapter plugin (else the deterministic
  default stands). The validated name/description/adapter flow into the kernel's existing
  `create_agent`; `notes` are advisory/UI only.
- **New module `relux-kernel/src/prime_admin_slots.rs`** (advisory; the action stays
  approval-gated). For a `PluginInstallation` turn the brain proposes `{plugin_id,
  confidence}` → normalized; for a `PermissionChange` turn it proposes `{subject_kind,
  subject_id, permission?, confidence}` — `subject_kind` is checked against the
  `["agent"]` allowlist (an off-allowlist kind fails closed), `subject_id` is honored
  ONLY when it names an EXISTING agent (`summary.all_agent_ids`), and the permission
  label is sanitized to the `[a-z0-9:_-]` grammar. `sharpen_admin_action` (state.rs)
  reshapes the proposed `InstallPlugin`/`GrantPermission` subject — but the turn stays a
  `PrimePlan::Propose` behind a human approval, so **a brain slot can never execute a
  protected install or grant by itself** (the kernel logs an approval and changes
  nothing; pinned by tests asserting the permission set / plugin count are unchanged).
- **New wire fields `PrimeTurn.agent_slots` / `PrimeTurn.admin_slots`** (`relux-core`,
  both `skip_serializing_if = "Option::is_none"`, so the wire is byte-for-byte unchanged
  on every un-sharpened turn). Provenance/presentation only — the kernel validated every
  field before acting/proposing.
- **One kernel chokepoint.** `KernelState::prime_turn_with_brain` takes a
  `BrainSlotProposals` bundle (task/agent/plugin/permission); `prime_turn_with_intent_
  and_slots` is now a thin wrapper passing only task slots, so the deterministic path is
  unchanged. The Act arm reconciles task/agent slots; the Propose arm sharpens the admin
  subject. The server dispatches the slot brain on the RESOLVED intent (the same
  reconciliation the kernel redoes), so a chat/status/plan turn never invokes it, and
  stamps the provenance label (model id / `Claude CLI` / `Codex CLI`) onto each card.
- **Both brains feed one validator per slot type.** OpenRouter goes through
  `ai::extract_{agent_slots,plugin_ref,permission_slots}_via_openrouter` (via a shared
  `complete_json_only` helper); the Claude/Codex CLI brains spawn in the same bounded,
  non-bypass mode (shared `cli_brain_json`) and their stdout is lifted by
  `parse_adapter_result` FIRST (`server.rs` `parse_cli_{agent_slots,plugin_ref,
  permission_slots}`) so the raw envelope never reaches the parser or the UI.
- **Dashboard.** Compact B&W chips on the Prime chat (`apps/dashboard/src/pages/Prime.tsx`
  + the shared `brainSourceLabel` helper in `src/prime.ts`): an "brain-extracted agent"
  card (normalized name/id, role, adapter) and an advisory admin card (the sharpened
  plugin id, or the grant subject + permission) that says plainly *"Advisory — requires
  your approval before anything changes."* The **Crew** and **Plugins** pages were
  verified to render via the safe `useAsync` hook (no `useLoaderData` blank-route bug).

Pinned by `prime_agent_slots_round_trip_and_omit_empty_optionals` /
`prime_admin_slots_round_trip_and_omit_empty_optionals` (core wire guards, plus the
extended omission assertions in `prime_suggestion_round_trips_…`); the `prime_agent_slots`
and `prime_admin_slots` unit tests (clean parse, noisy-reply extraction, invalid JSON,
unsupported-field / unsupported-subject-kind fail-closed, empty-name reject, control-char
clamp, duplicate-id reject, existing-only adapter/subject, low-confidence/echo no-op,
permission-label sanitize); the kernel integration tests (`brain_agent_slots_sharpen_a_
created_agent_…`, `brain_agent_slot_rejects_a_duplicate_id_…`, `no_agent_slot_proposal_…`,
`brain_permission_subject_sharpens_…_but_stays_approval_gated` + the unchanged-permission-
set safety assertion, `brain_permission_subject_is_dropped_when_the_agent_does_not_exist`,
`brain_plugin_ref_sharpens_…_but_stays_approval_gated` + the unchanged-plugin-count safety
assertion); the server seam tests (`cli_agent_slots_*`, `cli_plugin_ref_*`,
`cli_permission_slots_*`); and the dashboard `brainSourceLabel` test. No test calls a real
provider.

## Applied change (brain-assisted clarification wording + agent persona seed)

The remaining keyword surfaces flagged by the previous "Next recommended slice" are now
behind the brain: the **reflect-and-clarify wording** and the created-agent **starter
persona**. Per master plan §10.5 ("ask clarifying questions when needed"), §10.1/§10.2,
and §17.1, and following the reference read recorded in
`reference-driven-development.md` (Hermes `<missing_context>`/`<act_dont_ask>` ask-one-
question steering + `message_sanitization` clamp; openclaw `sessions-spawn-tool`/`common.ts`
allowlist + required string, `cli-output`/`balanced-json` envelope lift), a configured brain
may now *re-word* an ambiguous/musing turn and *propose* a bounded persona — validated hard.

- **New module `relux-kernel/src/prime_clarify.rs`.** `clarify_polish_kind(turn)` decides which
  turns are eligible: a `NeedsClarification` turn (every `Clarify` arm, incl. `TaskUpdate`,
  orchestration single-step, run/assign ambiguity) → a `Clarify` wording polish; a
  `Brainstorming` reply or a **single-step** `PlanRequest` steer → a `Brainstorm` polish; **every
  actionful turn → `None`**, so the brain is never near an action. `build_clarify_prompt` demands
  JSON-only `{text, confidence}`. `parse_clarify` lifts the JSON via the shared balanced-brace
  scanner, **rejects any field outside `text`/`confidence`/`rationale`**, sanitizes + clamps the
  text (clarify forced single-line, 240 chars; brainstorm 600), **structurally enforces a single
  `?` for a clarify** (a multi-question lecture or a statement is rejected), and **rejects any
  reply that claims a completed action** (a keyword safety rail). `reconcile_clarify` drops a
  low-confidence or pure-echo proposal. The brain only ever swaps `turn.reply` text on a
  non-actionful turn — the action-free wall is intact.
- **Both brains feed one validator.** OpenRouter goes through
  `ai::polish_clarify_via_openrouter`; the Claude/Codex CLI brains spawn in the same bounded,
  non-bypass mode and their stdout is lifted by `parse_adapter_result` FIRST (`server.rs`
  `polish_clarify_via_cli` / `parse_cli_clarify`) so the raw envelope never reaches the parser or
  the chat bubble. Both land on the SAME `parse_clarify` → `reconcile_clarify`. Wired in
  `run_prime` OUTSIDE the lock, gated on `clarify_polish_kind`; the free-form shaper is skipped
  for these turns so the brain returns ONE validated question/summary, never free-form prose. Any
  failure leaves the grounded deterministic template wording in place, with no provenance.
- **Agent persona seed.** The data model already supports it (`Agent.persona`,
  `KernelState::create_agent(persona)` — previously always handed `None`). `prime_agent_slots`
  now accepts an optional `persona` (added to the allowlist), sanitized/clamped, with an
  **overlong persona failing the whole proposal closed** (never silently truncated). A
  persona-alone proposal counts as a real contribution. The validated persona flows through the
  existing `AgentCreation` → `create_agent` seam and is surfaced on the new agent card (Prime
  chat) and on **Crew**. The deterministic path still creates a personaless agent.
- **Provenance (UI).** A small `🧠 brain-worded question/reply · <source>` chip on the Prime
  chat turn (server stamps `reply_polish` on the response) when the brain re-worded the turn; the
  agent-slot card shows the seeded persona; Crew renders an agent's persona when set. All
  presentation/provenance only — the wording was schema-validated and the turn is action-free.

Pinned by the `prime_clarify` unit tests (clean clarify/brainstorm parse, noisy-reply extraction,
invalid JSON / unsupported field fail-closed, exactly-one-question enforcement, action-claim
rejection, control-char strip + clamp, reconcile low-confidence/echo); the kernel integration
tests (`clarify_polish_targets_only_nonactionful_clarify_and_brainstorm`,
`brain_agent_slots_seed_a_starter_persona_on_the_created_agent`,
`deterministic_agent_create_has_no_persona`); the `prime_agent_slots` persona unit tests
(`parses_and_bounds_a_starter_persona`, `rejects_an_overlong_persona_fail_closed`,
`reconcile_honors_a_persona_only_contribution`); the server seam tests
(`cli_clarify_lifted_from_a_result_envelope`,
`cli_clarify_error_envelope_and_non_question_yield_nothing`,
`cli_clarify_brainstorm_rejects_an_action_claim`); and the dashboard `replyPolishLabel` test +
the extended `PrimeAgentSlots` persona round-trip. No test calls a real provider.

## Applied change (multi-turn clarification memory)

The biggest remaining intelligence gap: a `Clarify` turn asked one good question, but the
NEXT user message did not *carry* the prior question's context. "assign this to the
researcher" → "which task?" → "task_0001" reclassified the bare id from scratch as a
`DirectAnswer`, so the original request was lost and Prime read as keyword-shaped, not like
Hermes/Codex. Per master plan §10.1 (Intent Layer), §10.5 (Conversation Rules), §17.1
("Prime must understand conversational intent"), and following the reference read recorded
in `reference-driven-development.md` (openclaw's `exec-approval-followup-state` pending-record
+ TTL + consume-and-clear, and its `exec-approval-followup` continue-by-running-a-fresh-turn;
Hermes's `run_conversation` follow-up-interpreted-against-prior-context), Prime now remembers a
small, bounded pending clarification and resolves it safely on the next turn.

- **New module `relux-kernel/src/prime_clarify_memory.rs`.** `resolve_pending(pending,
  new_message, now_secs)` is the pure, deterministic decision: `Expired` past TTL,
  `Cancelled` on an explicit "never mind", `FreshRequest` when the follow-up stands on its
  own (`prime::is_standalone_request` — a complete command/question supersedes the pending
  context), else `Continue { combined }` where `combine` concatenates the stored original
  message with the bare answer (length-bounded). `is_resolvable_clarify_intent` limits the
  memory to the intents whose clarify a follow-up can actually turn into an action
  (`AssignTask` / `TaskCreation` / `CreateAndRunTask`) — a run-start / task-update clarify is
  NOT recorded (no by-id action is wired; we never set up an unresolvable loop or fake a
  capability).
- **New wire type `relux_core::PendingClarification`** — bounded, non-secret user text + a
  deterministic intent label + `needs` (e.g. `"task id"`) + `created_at_secs` /
  `expires_at_secs` + provenance. Persisted in `KernelState.pending_clarifications`
  (keyed `namespace::actor`) through the `meta` JSON snapshot seam, exactly like
  `orchestrations`; bounded by `MAX_PENDING_CLARIFICATIONS` (oldest evicted).
- **One kernel chokepoint.** `prime_turn_with_brain` resolves any pending record BEFORE
  classifying: a `Continue` swaps in the combined message (and drops the raw-answer brain
  proposals so the deterministic combined classification stands), a `FreshRequest`/`Expired`
  clears the record and handles the message fresh, a `Cancelled` returns a natural
  action-free reply. After the turn, `update_pending_clarification` records a NEW pending
  clarification only for a resolvable, unresolved `Clarify`, or clears it otherwise — so a
  follow-up can keep accumulating context, and a resolved request leaves nothing behind.
- **Safety invariants (binding).** The memory only decides *how to read* the follow-up; it
  executes nothing and grants no authority. The combined message flows through the unchanged
  `decide` → `prime_execute` (safe `Act`) / human-approval (`Propose`) path, so a continuation
  can never run a protected install or grant by itself, and an unknown task/agent still fails
  closed. An expired, cancelled, or superseded record is dropped, so a pending question can
  never silently steer a much later, unrelated message.
- **UI.** The server surfaces the still-pending record on `PrimeResponse.pending_clarification`
  (read back under the lock after the turn). The Prime chat shows a compact `⏳ waiting for:
  <needs>` chip (the pure `pendingClarificationLabel` helper) with a **Cancel** button that
  just sends "never mind" — a normal user message, never a privileged path. Chat stays the
  primary surface; no panel.

Pinned by the `prime_clarify_memory` unit tests (bare-answer continue, expiry ignored,
explicit cancel, fresh-command supersede, cancel-plus-command still acts, bounded combine,
only-resolvable-intents); the kernel integration tests
(`clarification_is_recorded_and_resolved_by_a_follow_up_answer`,
`an_explicit_cancellation_clears_the_pending_clarification`,
`an_unrelated_follow_up_supersedes_and_does_not_action_the_pending_request`,
`an_expired_clarification_is_ignored_and_does_not_continue`,
`a_risky_follow_up_still_requires_approval_through_the_memory_path`,
`pending_clarification_survives_a_snapshot_round_trip`); and the dashboard
`pendingClarificationLabel` test. No test calls a real provider.

## Applied change (roster-aware fuzzy assignee resolution)

Multi-turn memory carried "assign this to the researcher" → "which task?" → "task_0001"
into one combined message, but the assignee extractor then failed it: the deterministic
`extract_agent_id_from_assignment` takes only the FIRST word after "to", so "the
researcher" became the agent id `the` — which exists on no roster, so the canonical
continuation dialogue still dead-ended on "Agent with ID 'the' does not exist". Per master
plan §10.1/§10.2 and §17.1, and following the reference read recorded in
`reference-driven-development.md` (Hermes `repair_tool_call` normalize/strip-then-match;
openclaw `resolveSubagentTargetFromRuns` exact→prefix→ambiguous-is-an-error,
`resolveControlledSubagentTarget` resolve-only-to-an-existing-target), the `AssignTask`
decide arm now resolves a fuzzy assignee against the live roster.

- **New helpers in `relux-kernel/src/prime.rs`.** `extract_assignee_phrase` keeps the FULL
  trailing phrase ("the researcher"), task-id token stripped (vs. the first-word
  `extract_agent_id_from_assignment`, kept only as the "did the user name an agent?"
  presence signal the clarify branches use). `resolve_assignee(phrase, roster) ->
  AssigneeResolution` drops stopwords + sub-2-char noise, then matches the roster in
  fail-closed priority order — exact (case-insensitive) → unique prefix → unique substring;
  exactly one distinct match `Resolved`, more than one `Ambiguous`, none `Unresolved`. A
  `Resolved` id is taken verbatim from `summary.all_agent_ids`, so the resolver can never
  invent an assignee.
- **The `AssignTask` arm** keys its clarify branches on phrase *presence* (unchanged
  wording), but a present task id + named agent now runs through `resolve_assignee`:
  `Resolved` → the existing `AssignTask` `Act`; `Ambiguous` → a `Clarify` that lists the
  candidates and asks which (still a resolvable clarify, so the memory can continue it);
  `Unresolved` → the existing "Agent with ID '…' does not exist" `Reply`.
- **Safety (binding).** Deterministic, no brain in the loop — this is the fallback the
  later brain-assisted assignment slot reconciles against. Durable state still flows only
  through `decide` → `prime_execute`; only the assignee *resolution* got smarter, and a
  fuzzy phrase can only ever name an agent that already exists.

Pinned by the `prime` unit tests (`resolve_assignee_matches_exact_prefix_and_substring_…`,
`resolve_assignee_reports_ambiguity_and_never_invents`,
`assign_decide_resolves_a_fuzzy_assignee_against_the_roster`,
`assign_decide_clarifies_an_ambiguous_assignee`, `assign_decide_still_rejects_an_unknown_agent`)
and the kernel integration test `a_fuzzy_assignee_continuation_resolves_against_the_roster`
(the motivating dialogue end-to-end). No test calls a real provider.

## Applied change (by-id run start + a resolvable run-start clarification)

The multi-turn memory deliberately did NOT remember a run-start clarify because no by-id
`StartRun` action was wired — "start it" → "which one?" → "task_0001" could not resolve. The
`StartRun` action already existed (the decide arm just never read an explicit id), so this
slice wires it: the `RunStart` arm now honors a named, ready task id, and the run-start
clarify becomes resolvable. Per master plan §10.2/§10.5 and the same reference read
(openclaw `resolveControlledSubagentTarget` — act only on a target that EXISTS *and* is
runnable; the exec-approval-followup consume-and-continue shape for the memory).

- **`relux-kernel/src/prime.rs` `RunStart` arm.** When `extract_task_id` finds an id, it is
  honored only when it is in `summary.queued` (exists AND ready) → the existing `StartRun`
  `Act`; an existing-but-not-ready id gets an honest "not ready to start" `Reply`; an unknown
  id fails closed with "does not exist". With no id named, the prior ready-queue heuristic
  (single ready → start, several → ask, none → clarify) is unchanged.
- **`prime_clarify_memory::is_resolvable_clarify_intent`** now includes `RunStart`, and
  `clarify_needs_label(RunStart)` is `"task id"`, so a multi-ready "which should I start?"
  clarify is remembered and a bare task id continues it into a `StartRun`. A `TaskUpdate`
  clarify is still NOT recorded (no `UpdateTask` action is wired — no faked capability).
- **Safety (binding).** Deterministic; the continuation flows through the unchanged
  `decide` → `prime_execute` path and starts a run only for a task that exists and is ready,
  so a stale or fuzzy follow-up can never start the wrong task or an unrunnable one.

Pinned by `run_start_honors_an_explicit_ready_task_id`,
`run_start_reports_an_unready_or_unknown_explicit_id` (prime unit), the updated
`only_resolvable_intents_are_recorded` (`prime_clarify_memory`), and
`a_run_start_clarification_is_resolved_by_a_task_id_follow_up` (kernel integration).

## Applied change (brain-assisted continuation resolution)

The deterministic slices above fixed the common assignment/run-start continuations. This
slice adds the brain as a strictly-additive fallback for what the extractors still miss — an
assignment referenced without a `task_` token, or a continuation where the original request and
the answer only TOGETHER name both task and agent. Per master plan §10.1/§10.2 and §17.1, and
following the reference read in `reference-driven-development.md` (openclaw
`exec-approval-followup` continue-by-fresh-validated-turn + `resolveSubagentTargetFromRuns`
existing-target resolution; Hermes `coerce_tool_args` + follow-up-in-context), a brain may now
*propose* the missing `{task_id, agent_id}`, validated against the live state.

- **New module `relux-kernel/src/prime_assign_slots.rs`.** `build_assign_slots_prompt` grounds
  the brain in the live board; `parse_assign_slots` lifts the JSON, rejects any field outside the
  allowlist (fail closed), and sanitizes/clamps; `reconcile_assign_slots` honors `task_id` ONLY
  when it exists (`summary.all_task_ids`), resolves `agent_id` via the shared
  `prime::resolve_assignee` (always an existing agent), and requires BOTH — a half-resolved
  assignment is never invented. New wire type `relux_core::PrimeAssignSlots` (provenance only,
  `skip_serializing_if`).
- **Kernel chokepoint.** `BrainSlotProposals` gains `assign` + a `continuation` flag. On an
  `AssignTask` intent where the deterministic plan did NOT produce the assignment, a validated
  proposal PROMOTES it to the same safe `AssignTask` action (assignment is safe and in-scope, and
  both ids are validated — the brain authors no risky action). The bundle is kept ONLY when
  `continued == slots.continuation`, so a proposal computed for the wrong message can never shape
  an action. `KernelState::continuation_preview` is the read-only seam the server consults to
  learn the combined message + recorded intent before dispatching the brain.
- **Both brains feed one validator.** OpenRouter via `ai::extract_assign_slots_via_openrouter`;
  the Claude/Codex CLI brains via `server.rs` `extract_assign_slots_via_cli` → the no-leak
  `parse_cli_assign_slots` (`parse_adapter_result` FIRST). The server dispatches the slot brain on
  the COMBINED message of a continuation (else the raw message + resolved intent), and stamps the
  provenance label.
- **Safety (binding).** Strictly additive — any failure (no brain, low confidence, unknown id,
  mismatched continuation flag) leaves the deterministic clarify in place. Durable state still
  flows only through `decide` → `prime_execute`; a risky intent still becomes an approval-gated
  `Propose`. The brain can promote ONLY a safe assignment, and only to ids that already exist.
- **Dashboard.** A compact B&W "🧠 brain-resolved assignment" card on the Prime chat
  (`apps/dashboard/src/pages/Prime.tsx`, reusing the shared `brainSourceLabel`), present only when
  the kernel attached `assign_slots`.

Pinned by the `prime_assign_slots` unit tests (clean/noisy parse, unsupported-field /
objectless fail-closed, reconcile validates-both / fails-closed-on-unknown-or-low-confidence /
falls-back-to-deterministic, prompt grounding); the kernel integration tests
(`brain_assign_slots_resolve_an_under_specified_assignment`,
`brain_assign_slots_fail_closed_on_an_unknown_id`,
`continuation_slots_are_dropped_on_a_fresh_turn_and_vice_versa`); the server seam tests
(`cli_assign_slots_lifted_from_a_result_envelope`,
`cli_assign_slots_error_envelope_and_prose_yield_nothing`,
`cli_assign_slots_unsupported_field_fails_closed`); and the core wire guard (the extended
omission assertion). No test calls a real provider.

## Applied change (safe by-id task update)

`TaskUpdate` was the one resolvable-looking clarify with no action wired: `decide` could
only ask, the multi-turn memory deliberately refused to record it, and
`task_update_clarify` reflected the parsed field but never applied it. Per master plan
§10.1/§10.2 and §17.1, and following the reference read recorded in
`reference-driven-development.md` (openclaw `update-plan-tool` schema + status allowlist,
`tool-mutation` mutating-action classifier, `sessions-spawn-tool`/`common.ts`
unsupported-key rejection + required string + clamp, `subagents-utils`
resolve-to-an-existing-target; Hermes `coerce_tool_args` + sanitization), `PrimeAction::
UpdateTask { task_id, patch }` is now a REAL, safe mutating action.

- **New module `relux-kernel/src/prime_update_slots.rs`.** `TaskUpdatePatch` is the validated
  change set (title / details / priority / status / assignee), serialized into the action's
  `patch` string and parsed back at apply time. `deterministic_update` is the rail: it parses a
  SIMPLE command ("rename task_0001 to Fix the login blank page", "set task_0001 priority to 8",
  "cancel task_0001", "reassign task_0001 to the researcher"), validates the task against
  `summary.all_task_ids`, resolves a fuzzy assignee against the roster, clamps priority to
  `[1,9]`, and honors ONLY the operator-settable status allowlist (`blocked`/`cancelled`). A
  named-but-unknown task / agent fails closed with an honest reply; a non-settable status
  ("mark it done") is honestly refused (Prime never fakes a completion — that flows through the
  run lifecycle); a missing task/field asks one concrete, *resolvable* clarifying question.
  `build_update_slots_prompt` / `parse_update_slots` / `reconcile_update_slots` are the
  brain-assisted fallback (allowlist fields, sanitize/clamp, drop a non-settable status,
  fail closed on an unsupported field), validated against the live state.
- **Supported fields:** `title`, `details` (folded into the task input), `priority` (1-9
  clamp), `status` (operator-settable `blocked`/`cancelled` only), `assignee` (resolved to an
  existing agent). `deadline`/due-date and labels are deliberately NOT exposed: a label field
  does not exist on the `Task` model, and a free-text deadline has no date-validation
  infrastructure yet — surfaced as a future slice rather than faked.
- **Kernel chokepoint.** `decide`'s `TaskUpdate` arm runs the deterministic rail; the
  `prime_turn_with_brain` chokepoint promotes a validated brain proposal ONLY when the
  deterministic path CLARIFIED (an explicit-but-wrong id / refused status is never silently
  corrected) and `continuation` matches the turn. `prime_execute`'s `UpdateTask` arm re-checks
  existence, enforces a **terminal-state guard** (a completed/failed/cancelled/expired task is
  never edited), applies only allowlisted fields, audits `task:update`, and attaches a
  `PrimeTaskUpdate` change card.
- **Classify.** The classifier recognizes a task-anchored field command ("set task_0001
  priority to 8", "cancel task_0001", "rename task_0001 to …") as a by-id update BEFORE the
  broad task-creation catch (so an embedded "fix"/"build" verb does not mint a new task), while
  a *question* about a task ("should I cancel task_0001?") stays a `Brainstorming` conversation.
- **Memory.** `is_resolvable_clarify_intent` now includes `TaskUpdate`, so "change task
  priority" → "task_0001 to 8" continues the original request; `clarify_needs_label(TaskUpdate)`
  names what is missing ("task id" / "the field to change").
- **UI.** Every successful `TaskUpdate` turn carries a `PrimeTurn.update` card (the changed
  fields, the task linked to Work) with a small `🧠 <source>` provenance chip ONLY when a brain
  resolved the change (`apps/dashboard/src/pages/Prime.tsx` + the pure `updateChangeSummary` /
  `updateProvenance` helpers in `src/prime.ts`).

Safety invariants (binding): a by-id update is a SAFE, in-scope action (it edits the operator's
own task; it is never risk-gated), validated against the live state, with a terminal-state guard
the brain can never bypass and a status allowlist that keeps Prime from decreeing a fake
completion. Any failure (no brain, low confidence, unknown task/agent, unsupported field,
non-settable status) leaves the deterministic outcome — a clarify or an honest reply — in place.

Pinned by the `prime_update_slots` unit tests (deterministic rename/priority/cancel/block/
reassign, ambiguous/unknown assignee, unknown task, under-specified clarify, brain parse/
reconcile, status allowlist, patch round-trip); the `prime` decide + classify tests
(`task_update_decide_applies_a_simple_command`, `task_update_decide_fails_closed_and_refuses_
completion`, `task_update_decide_clarifies_when_underspecified`,
`task_update_is_classified_for_by_id_field_commands`); the kernel integration tests
(`task_update_applies_each_supported_field`, `task_update_fails_closed_on_unknown_task_and_agent`,
`task_update_refuses_completion_and_terminal_tasks`, `brain_update_slots_resolve_an_under_
specified_update`, `brain_update_slots_fail_closed_on_an_unknown_task`,
`task_update_clarification_is_resolved_by_a_follow_up`, `casual_chat_never_triggers_a_task_update`);
the server seam tests (`cli_update_slots_*`); the core wire guard
(`prime_task_update_round_trips_and_omits_source_when_absent` + the extended omission assertion);
and the dashboard `updateChangeSummary` / `updateProvenance` test. No test calls a real provider.

## Applied change (unified brain decision envelope)

The brain stack had grown one specialized call at a time, so a single Prime turn could fire
the brain TWO or THREE times in series — intent, then slots for the resolved intent, then
clarify wording. That is slow, costly, and less coherent than how Hermes/Codex/Claude work
(ONE model response carries the answer and the structured actions in one turn). Per master
plan §10.1/§10.2/§17.1 and following the reference read recorded in
`reference-driven-development.md` (Hermes `run_conversation` one-response-carries-everything +
allowlist validation; openclaw `extractBalancedJsonPrefix`/`parseCliOutput` parse-only-the-
object, `readPlanSteps` compositional field-by-field validation, `sessions-spawn-tool`
unsupported-key rejection), a configured brain may now answer the WHOLE turn in ONE call.

- **New module `relux-kernel/src/prime_decision.rs`.** `PrimeBrainDecision` carries optional,
  strictly-allowlisted sections: `classification` (intent), `task`/`agent`/`plugin`/
  `permission`/`assign`/`update` slots, `wording`, plus `confidence`/`provenance`.
  `build_decision_prompt(message, summary)` is the one grounded prompt; `parse_decision` lifts
  the envelope via the shared balanced-brace scanner, **rejects any UNKNOWN top-level key
  (fail the WHOLE envelope closed)**, and validates each KNOWN section by REUSING its existing
  validator (`parse_intent_proposal`, `parse_task_slots`, `parse_agent_slots`,
  `parse_plugin_ref`, `parse_permission_slots`, `parse_assign_slots`, `parse_update_slots`) —
  no weaker duplicate logic. An **invalid nested section is dropped** (its specialized/
  deterministic fallback applies) while the rest of the envelope stands; an envelope with zero
  usable sections is a failure. The carried wording is validated LATER against the turn's
  actual `ClarifyKind` through the SAME `parse_clarify`/`reconcile_clarify` chokepoint
  (`validated_wording`), so a clarify is still forced to one question and an action-claim is
  still rejected.
- **Both brains feed one parser.** OpenRouter via `ai::decide_prime_via_openrouter`; the
  Claude/Codex CLI brains via `server.rs` `decide_prime_via_cli` → the no-leak
  `parse_cli_decision` (`parse_adapter_result` FIRST, so the raw `--output-format json`
  envelope never reaches the parser or the chat).
- **Wired in `run_prime` (server.rs), unified-first.** The continuation pre-flight + board
  snapshot run first; then ONE unified decision call on the COMBINED message (continuation) or
  the raw message, grounded with the live board. Its `classification` becomes the intent
  proposal and its slots become the `BrainSlotProposals` bundle fed to the unchanged
  `prime_turn_with_brain` chokepoint; a carried clarify wording is reused by `run_clarify_polish`
  with NO second call. **Fallback:** when the unified call returns nothing usable (no brain,
  disabled, malformed/empty envelope, unknown top-level key), the prior specialized stack runs —
  a dedicated intent call, then a dedicated slot call for the resolved intent, then the dedicated
  clarify polish — so behavior is byte-for-byte the old path whenever the unified shape is
  unavailable (`Local` always takes this path, exactly as before).
- **Flows now on the unified call:** intent + task/agent/plugin/permission/assign/update slots +
  clarify/brainstorm wording, all in one round-trip. **Still specialized (documented):** the
  free-form conversational reply for non-clarify chat (`shape_reply`/`run_cli_brain`) and the
  advisory multi-step plan-card polish (`polish_proposal`) — neither is part of the
  intent+slots+wording decision; folding them in is a future slice.
- **Safety invariants (binding).** The envelope changes only HOW the brain is asked and parsed,
  NOT authority. The fail-closed intent gate (`reconcile_intent`) still runs at the kernel
  chokepoint (guarded chat can never become work); every slot is still reconciled against the
  live state and the kernel uses ONLY the sections matching the turn it produces; risky
  plugin/permission slots stay advisory behind a human approval; the wording stays action-free
  and schema-validated. The brain authors a *proposal*; `decide` → `prime_execute` (or a human
  approval) remains the SOLE path that changes durable state.
- **UI.** A single concise `🧠 one brain decision · <source>` chip (the pure
  `decisionSourceLabel` helper + `PrimeResponse.decision_source`) is shown ONLY when the one
  unified call carried more than one proposal; the existing per-section chips (intent, slots,
  wording) still attribute each piece. No new panel.

Pinned by the `prime_decision` unit tests (full valid decision, noisy-reply extraction, unknown
top-level fail-closed, invalid/objectless nested section dropped, off-allowlist intent label
dropped, no-usable-section error, `validated_wording` reuse, assign/update reuse); the kernel
integration tests (`unified_decision_creates_a_task_with_title_and_details_in_one_envelope`,
`unified_decision_updates_a_task_by_id_in_one_envelope`,
`unified_decision_supplies_validated_clarify_wording_in_one_envelope`,
`unified_decision_ideation_still_creates_nothing`); the server seam tests (`cli_decision_*`:
valid envelope, plain JSON, error envelope/prose, unknown-top-level fail-closed); and the
dashboard `decisionSourceLabel` test. No test calls a real provider.

## Applied change (folding the conversational reply + plan-polish into the unified envelope)

The unified envelope (above) answered intent + slots + clarify wording in one call, but TWO brain
calls still ran separately AFTER it: the free-form conversational reply (`shape_reply` /
`run_cli_brain`) for a non-clarify chat turn, and the advisory multi-step plan-card polish
(`polish_proposal`). So a greeting still cost a decision call plus a reply call, and a multi-step
plan turn a decision call plus a reply call plus a polish call — slower and less coherent than how
Hermes/Codex answer (ONE response carries the natural text AND the structured actions). Per master
plan §10.1/§10.2/§11.1/§17.1 and following the reference read recorded in
`reference-driven-development.md` (Hermes `run_conversation` one-message-carries-content-and-
tool_calls + the exhaustion fallback; openclaw `update-plan-tool` schema validation, `cli-output`/
`balanced-json` parse-only-the-object, `sessions-spawn-tool` unsupported-key rejection), this slice
folds both — where safe — into the one decision envelope, with the deterministic/policy authority
unchanged.

- **Two new optional sections on `PrimeBrainDecision`** (`relux-kernel/src/prime_decision.rs`):
  `reply` (the free-form conversational answer; `assistant_message` accepted as an alias) and
  `plan_polish` (the advisory plan-card overlay). Both are added to the top-level allowlist; any
  OTHER unknown top-level key still fails the WHOLE envelope closed. `build_decision_prompt` now
  describes both sections and their safety rules (reply = a short natural answer for a
  conversational turn, never an action claim; plan_polish = wording only, never the step
  count/order/owners).
- **Carried raw, validated LATER** (the same shape `wording` already uses, because eligibility /
  grounding is known only after the kernel produces the turn):
  - `validated_reply(deterministic_text)` reuses the EXACT brainstorm chokepoint a clarify reply
    uses (`prime_clarify::parse_clarify` with `ClarifyKind::Brainstorm` → `reconcile_clarify`):
    control chars stripped, length clamped (600), an action-claim (`ACTION_CLAIM_MARKERS`) rejected
    wholesale, low-confidence / pure-echo dropped. A bare-string `reply` is normalized to
    `{text, confidence}` (stamped just above the honor floor so a deliberately-simple committed
    reply is honored).
  - `validated_polish(proposal, label)` reuses the EXACT `validate_polish` chokepoint (via
    `ai::polish_from_cli_text`): a step title is honored ONLY on an exact authoritative-index
    match (any merge/split/reorder/add/rename drops the titles entirely), summary/questions/risks
    are trimmed and bounded, and `label` stamps provenance.
- **Wired in `run_prime` (server.rs), unified-first with the dedicated calls as the fallback.**
  A non-actionful, non-clarify conversational turn PREFERS `decision.validated_reply(&turn.reply)`
  (no extra call); on a miss it falls back to the dedicated `run_cli_brain`/`shape_reply`. A
  non-actionful plan turn PREFERS `decision.validated_polish(&proposal, …)`; on a miss it falls
  back to the dedicated `polish_proposal`/`polish_proposal_via_cli`. So behavior is byte-for-byte
  the prior path whenever the fold is unavailable (`Local` always takes the fallback, exactly as
  before). The new `unified_reply_outcome` helper stamps the mode/model provenance exactly as the
  clarify-polish path does.
- **Flows now on the unified call:** intent + task/agent/plugin/permission/assign/update slots +
  clarify/brainstorm wording + **the free-form conversational reply** + **the advisory plan-card
  polish**, all in one round-trip. Nothing in the intent/slots/wording decision is left for a
  second brain call on the common chat / plan-preview turns.
- **Safety invariants (binding).** The fold changes only HOW the brain is asked (one call) and HOW
  its reply is parsed (one allowlisted object), NOT authority. The action-free wall is intact: the
  reply is applied ONLY on a NON-actionful turn, so the brain still never narrates a real state
  change — an actionful turn keeps the grounded deterministic reply. We deliberately do NOT
  implement the permitted "after-action explanation" variant: the brain composes its reply before
  the kernel executes, so it cannot honestly narrate the actual result, and folding it would breach
  the wall — it stays a deferred future slice, not a faked capability. The plan-polish runs through
  the identical `validate_polish` index-match invariant, so it can never change what "Create these
  tasks" creates. Any failure leaves the dedicated specialized call, then the deterministic outcome.

Pinned by the `prime_decision` unit tests (`carries_a_free_form_reply_and_validates_it_via_the_
brainstorm_chokepoint`, `a_reply_that_claims_a_completed_action_is_rejected`,
`carries_plan_polish_and_validates_it_against_the_authoritative_proposal`,
`reply_and_plan_polish_count_toward_the_section_total`) and the server seam test
(`cli_decision_carries_reply_and_plan_polish_through_the_no_leak_seam`). No test calls a real
provider.

## Applied change (the first safe Prime tool loop — READ-ONLY context tools)

Every brain stage above is *propose-only* and answers from ONE static `StateSummary` snapshot baked
into the prompt: the brain could not drill into a specific task, inspect a run, or enumerate the
crew before answering. That is the gap §10.1/§17.1 names — Prime "does not inspect live
control-plane state through a governed tool interface before answering the way Hermes/Codex/
Paperclip-like agents do". Per master plan §10.1 (Intent Layer), §10.2 (Action Layer), §17.1, and
following the reference read recorded in `reference-driven-development.md` (Hermes `run_conversation`
bounded loop + tool-NAME allowlist validation + `"Tool '…' does not exist. Available: …"`
self-correction + result injection; openclaw `isMutatingToolCall` fail-closed read-only/mutating
gate, `common.ts` required-string/`ToolInputError`, `cli-output`/`balanced-json` parse-only-the-
object), Prime now has the FIRST safe rung: a bounded, governed loop over **read-only context
tools**.

- **New module `relux-kernel/src/prime_tools.rs`** (pure, provider-free). `READ_ONLY_TOOLS` is the
  explicit allowlist (`board_summary`, `list_tasks`, `get_task`, `list_agents`, `get_agent`,
  `list_runs`); `classify_tool` is the FAIL-CLOSED gate (anything not on the allowlist is
  `Refused`, never executed — the first slice ships read-only tools only, so the allowlist IS the
  read-only set). `interpret_reply` lifts the brain's `{"tool":..,"args":..}` via the shared
  balanced-brace scanner and validates the name; an off-list name becomes `UnknownTool` (fed back
  via `unknown_tool_feedback`, Hermes's self-correction), a `{"done":true}`/no-tool reply ends the
  loop. `execute_context_tool(snapshot, call)` is a PURE read of a `ContextSnapshot` — a missing id
  is an honest `ok:false` ("does not exist"), never a fabricated record; lists are bounded and every
  result is length-clamped. `ContextLoop` is the bounded driver (`MAX_TOOL_ROUNDS`, stop-on-repeat);
  the async drivers and the synchronous `run_context_loop` test twin share the SAME stepper.
- **New `KernelState::context_snapshot(&ctx)`** (`state.rs`) takes an owned, bounded read-only
  projection of the live board (tasks/agents/runs, mirroring `inspect_state`'s view) ONCE under the
  kernel lock, so the loop's brain rounds run OUTSIDE the lock and the executors stay pure.
- **New wire type `relux_core::PrimeContextRead`** + `PrimeTurn.context_reads: Vec<…>`
  (`skip_serializing_if = "Vec::is_empty"`, so the wire is byte-for-byte unchanged on every turn
  that consulted no tool). Provenance only: tool name + an honest `ok` flag + a short summary; the
  full result bodies stay server-side grounding and never ship.
- **All three brains drive the loop** through the SAME stepper. OpenRouter via
  `ai::complete_tool_round`; the Claude/Codex CLI brains via `server.rs` `cli_brain_tool_round` →
  the no-leak `lift_cli_tool_text` (`parse_adapter_result` FIRST, error envelope dropped). `Local`
  (no brain) gathers nothing.
- **Wired in `run_prime`** (`server.rs`): the read-only snapshot is taken alongside the board
  summary under the existing pre-turn lock; for a NON-actionful inspection turn
  (`turn_wants_context`: `StatusQuestion`/`ExplanationRequest`/`DirectAnswer`/`Brainstorming`) the
  loop runs OUTSIDE the lock, and the gathered observations are folded into the reply's
  `grounded_facts` (`grounded_facts_with_observations`) for the existing reply-shaping brain
  (`shape_reply`/`run_cli_brain`). The gathered reads are surfaced on `PrimeTurn.context_reads`.

Safety invariants (binding): the loop is **read-only and gather-only**. Every tool is a pure read
of the snapshot; there is no path from `prime_tools` to `prime_execute`, an approval, or any durable
change. The tool NAME is fail-closed validated against the read-only allowlist before execution (an
off-list name is refused and self-corrected, never run); the loop is bounded by `MAX_TOOL_ROUNDS`
and stop-on-repeat; results are length/list-bounded; a missing id is an honest miss, never
fabricated. The gathered reads only GROUND the existing action-free reply (the brain still authors
no intent, slot, or action), and the loop runs ONLY on a non-actionful inspection turn under a
configured brain. `Local` is byte-for-byte the prior reply path. The write-capable tool surface is
deliberately deferred until this read-only loop is proven.

Pinned by the `prime_tools` unit tests (`classify_is_fail_closed_on_unknown_names`,
`interpret_detects_calls_unknown_tools_and_done`, `execute_reads_real_state_and_is_honest_about_
misses`, `list_tasks_honors_an_optional_status_filter`, `get_agent_reads_the_roster`,
`loop_gathers_validates_and_self_corrects_with_a_scripted_brain`, `loop_is_bounded_by_the_round_cap`,
`loop_stops_on_a_repeated_call_with_no_progress`, `render_observations_and_wire_projection_are_
bounded_and_provenance_only`, `no_brain_gathers_nothing`); the kernel integration test
`context_snapshot_feeds_the_read_only_tools_end_to_end`; the server seam test
`cli_tool_round_lifts_text_and_drops_error_envelopes` (the no-leak boundary, feeding the SAME
`interpret_reply`); and the core wire guard `prime_context_read_round_trips_as_bounded_provenance`
plus the extended omission assertion. No test calls a real provider.

## Applied change (dashboard provenance for the read-only context loop)

The read-only tool loop (above) shipped the `PrimeTurn.context_reads` wire field but left it
invisible: the operator could not see what live state Prime inspected before answering, so a brain
that drilled into a task / the crew / the runs read as a hidden action rather than visible
provenance. Per master plan §10.1/§17.1 and §11.1 (Prime Chat surfaces what the brain did), and
following the reference read recorded in `reference-driven-development.md` (open-webui
`ToolCallDisplay.svelte` — collapsed-by-default summary + per-tool status icon + tool-named label +
a BOUNDED, expand-to-see result), the inspection is now visible as compact, bounded provenance.

- **Wire type on the dashboard.** `apps/dashboard/src/api.ts` gains `ReluxPrimeContextRead`
  (`tool` / `ok` / `summary`) and `ReluxPrimeTurn.context_reads?: ReluxPrimeContextRead[]`, mirroring
  the `relux_core::PrimeContextRead` the kernel already serializes — provenance only, omitted on
  every turn that consulted no tool.
- **Pure helpers** (`apps/dashboard/src/prime.ts`): `contextReadsUsedLabel` names the DISTINCT tools
  in look order, itself bounded (`used: a, b, c, d, +N more`); `contextReadsHadMiss` flags an honest
  miss (an `ok:false` read — a missing id, never fabricated) for a subtle ok/partial indicator;
  `contextReadDetail` clamps each read's summary to 160 chars (never an unbounded blob / raw JSON);
  `boundedContextReads` caps the detail list (`MAX_CONTEXT_READS_SHOWN = 8`) with an honest `+N more`.
- **UI.** The Prime turn card renders a `<details>` whose `<summary>` is the always-on chip
  `🔎 used: get_task, list_agents`; expanding it shows a bounded per-read list with a `✓`/`!`
  ok/miss icon, the tool name, and the clamped one-line summary. Collapsed by default so the chat
  stays primary and the input composer is untouched; no new panel.

Safety/invariants (binding): this is **presentation/provenance only**. The chip renders ONLY what
the kernel returned, attributes no authority, and appears only on a turn that genuinely ran the
(already governed, fail-closed, read-only) loop. Unlike open-webui — which renders the tool's full
raw arguments/result JSON — Relux ships **no raw JSON / provider envelope to the UI**, only the
short, server-clamped `summary` (the full result body stayed server-side grounding): the same
no-leak posture as the two CLI-output shaping seams. A turn that consulted no tool omits the field
and renders nothing, so existing turns are unaffected.

Pinned by the `apps/dashboard/test/prime.test.ts` cases (`contextReadsUsedLabel` distinct/ordered/
bounded/null, `contextReadsHadMiss` honest-miss, `contextReadDetail` clamp + honest-fallback,
`boundedContextReads` cap + hidden-count). No backend change (the wire was already produced by the
read-only loop slice); the dashboard bundle was rebuilt.

## Applied change (more read-only context tools — runs / plugins / approvals)

The first read-only loop shipped six tools (`board_summary`/`list_tasks`/`get_task`/`list_agents`/
`get_agent`/`list_runs`). The brain could enumerate runs but not drill into a single run, could not
enumerate the installed plugins/adapters, and could not inspect the approval queue — the "more
read-only tools" rung this audit's "Next recommended slice" named. Per master plan §10.1 (Intent
Layer), §10.2 (Action Layer), §17.1, and following the reference read recorded in
`reference-driven-development.md` (Hermes `run_conversation` bounded loop + name-allowlist
validation, unchanged; openclaw `tool-mutation` `READ_ONLY_ACTIONS` `get`/`list`/`inspect` verb set,
`common.ts` `readStringParam` required, `sessions-spawn-tool` unsupported-key rejection, the no-leak
CLI-output seam), three more **read-only** tools now ride the SAME governed loop.

- **`get_run`** — full detail of one run by id: `status`, `task`, `agent`, `adapter`, the logical
  start/end stamps, the real `duration_ms` (CLI runs only), and a **redacted, bounded** `summary`/
  `error`. A missing/empty/unknown `run_id` is an HONEST `ok:false` miss ("Run '…' does not exist."),
  never a fabricated run. The raw provider `usage`/`cost` envelope is **deliberately NOT projected**.
- **`list_plugins`** — the installed plugins/adapters: `id`, `version`, `kind`, `enabled`,
  `protected` (a bundled fixture), `source_kind`, and the manifest tool count. The raw `source_label`
  (a local path / URL) is **deliberately NOT projected** — only the source-kind label.
- **`list_approvals`** — the approval queue, pending-first then most-recent (the same ordering the
  `/v1/relux/approvals` endpoint uses): `id`, `status`, `risk`, `requested_by`, and a **redacted,
  bounded** `action`/`reason`. An optional `{"status":"pending|approved|rejected"}` filter is honored
  only when recognized; an unrecognized filter is ignored (all listed), never an error.
- **Snapshot, not faked.** `KernelState::context_snapshot` (`state.rs`) gains `plugins` (from the live
  `installed_plugins()` + each manifest's tool count) and `approvals` (sorted like the HTTP endpoint),
  and the existing run projection is enriched with the adapter/timing/redacted-summary/error — all
  taken ONCE under the kernel lock, bounded by `MAX_SNAPSHOT_ITEMS`, with free-text fields run through
  the new `redact_line` (control-strip, whitespace-collapse, clamp). If real data does not exist
  (e.g. no runs/approvals yet), the tool returns an HONEST empty, never a placeholder.
- **No write surface, no new authority.** All three are pure reads of the `ContextSnapshot`;
  `classify_tool` stays fail-closed (anything off the read-only allowlist is `Refused`), the loop is
  still bounded by `MAX_TOOL_ROUNDS`/stop-on-repeat, results are still length/list-bounded, and there
  is no path from `prime_tools` to `prime_execute` or an approval. The async drivers (OpenRouter /
  CLI) and the synchronous test twin are unchanged — a new tool is just an allowlist member + a pure
  executor, so the loop control flow is pinned ONCE and the new tools inherit it.
- **UI.** No dashboard change: the `PrimeTurn.context_reads` wire type is generic (tool + ok +
  summary), so the provenance chip from the prior slice surfaces the new tool names automatically
  (`🔎 used: get_run, list_plugins, …`) with the same no-leak, bounded rendering. The dashboard
  bundle is unchanged.

Safety invariants (binding): the new tools are **read-only and gather-only**, exactly like the first
six. Every tool is a pure read of the snapshot; a missing id is an honest miss; the raw provider
`usage`/`cost` envelope and the raw plugin `source_label` are never projected; the tool NAME is still
fail-closed validated against the read-only allowlist; the loop is bounded; the gathered reads only
GROUND the existing action-free reply. The brain authors no intent, slot, or action, and the
write-capable tool surface stays deferred.

Pinned by the new `prime_tools` unit tests (`get_run_reads_real_runs_and_is_honest_about_misses`,
`list_plugins_reports_enabled_protected_and_tool_counts`,
`list_approvals_honors_an_optional_status_filter`, plus the extended
`classify_is_fail_closed_on_unknown_names`); the extended kernel integration test
`context_snapshot_feeds_the_read_only_tools_end_to_end` (list_plugins shows the bundled prime adapter
as protected, list_approvals reads the empty queue honestly, get_run on an unknown id is an honest
miss). No test calls a real provider; no wire/dashboard change was needed.

## Applied change (read context on the unified decision)

The read-only context loop shipped as a SELF-CONTAINED sidecar: the unified `PrimeBrainDecision`
answered intent + slots + wording from one static board snapshot, and THEN a separate multi-round
`ContextLoop` ran to gather live reads on a non-actionful inspection turn. So an inspection turn
cost the unified call PLUS up to `MAX_TOOL_ROUNDS` loop calls PLUS the reply call — two disjoint
brain interactions, less coherent than how Hermes/Codex answer (ONE response carries the answer AND
the structured tool requests). Per master plan §10.1 (Intent Layer), §10.2 (Action Layer), §17.1,
and following the reference read recorded in `reference-driven-development.md` (Hermes
`run_conversation` one-response-carries-content-and-tool_calls + name-allowlist validation + bounded
inject-and-ground; openclaw `tool-mutation` fail-closed read-only gate, `update-plan-tool` per-entry
validation, `sessions-spawn-tool` unsupported-key rejection, `cli-output`/`balanced-json`
parse-only-the-object), the ONE unified decision envelope may now ALSO carry the brain's read-only
tool requests, executed deterministically and grounded into the reply — the sidecar loop kept as the
fallback, with NO mutation path.

- **New section `tool_requests` on `PrimeBrainDecision`** (`relux-kernel/src/prime_decision.rs`):
  an array of `{tool, args}` the brain wants run BEFORE it answers, carried in the SAME envelope as
  intent/slots/wording. `parse_decision` validates EACH entry through the SAME read-only allowlist
  the loop uses (`prime_tools::validate_tool_request` → `classify_tool`): a mutating / unknown /
  made-up name (`delete_task`, `run_shell`) is DROPPED at parse time and can never execute; the list
  is capped at `MAX_TOOL_ROUNDS`; `context_reads` is accepted as an alias. The section counts toward
  the usable-section total, and `tool_requests`/`context_reads` join the top-level allowlist (any
  OTHER unknown top-level key still fails the whole envelope closed). `build_decision_prompt`
  describes the section and lists the read-only tool names.
- **Deterministic, bounded execution** (`relux-kernel/src/prime_tools.rs`):
  `execute_requested_reads(snapshot, calls)` runs the validated list against the pre-taken
  `ContextSnapshot` — bounded by `MAX_TOOL_ROUNDS`, repeated identical reads skipped — with NO brain
  round. It is the unified counterpart of the multi-round `ContextLoop`; both are read-only and
  gather-only, and both feed `render_observations` → the existing reply shaper.
- **Wired in `run_prime` (server.rs), unified-first.** On a non-actionful inspection turn
  (`turn_wants_context` ∧ `!is_actionful` ∧ a configured brain), if the unified decision carried
  `context_requests`, the server executes THOSE deterministically (no second multi-round loop, no
  DUPLICATE execution) and shapes the reply grounded in the observations — the one bounded follow-up
  response. When the envelope requested none (or there was no usable decision), the sidecar
  `gather_read_only_context` loop runs exactly as before. `Local` always takes the sidecar path
  (gathering nothing), byte-for-byte the prior behavior.
- **Both brains feed one parser.** OpenRouter via `ai::decide_prime_via_openrouter`; the
  Claude/Codex CLI brains via `server.rs` `decide_prime_via_cli` → the no-leak `parse_cli_decision`
  (`parse_adapter_result` FIRST, so the raw `--output-format json` envelope never reaches the request
  validation or the chat).
- **Safety invariants (binding).** The fold changes only WHEN the read-only tools are requested (in
  the one decision call) and removes a duplicate brain interaction; it changes NOTHING about
  authority or the read-only-and-gather-only contract. Every requested tool is a pure read of the
  owned snapshot validated against the read-only allowlist; a mutating request is rejected at parse
  time; the execution is deterministic and bounded; there is no path from this path to
  `prime_execute`, an approval, or any mutation. The reads only GROUND the action-free reply, and on
  any failure / a no-request turn / `Local` the sidecar loop is the byte-for-byte fallback.
- **UI.** No dashboard change: the requested reads land on the SAME `PrimeTurn.context_reads` wire
  type (tool + ok + summary), so the existing provenance chip (`🔎 used: get_task, list_agents`)
  surfaces them automatically with the same no-leak, bounded rendering. The dashboard bundle is
  unchanged.

Pinned by the `prime_decision` unit tests (`carries_read_only_tool_requests_validated_against_the_
allowlist`, `a_mutating_tool_request_is_rejected_never_executed`,
`context_reads_is_accepted_as_an_alias_for_tool_requests`, `tool_requests_are_bounded_by_the_round_
cap`); the `prime_tools` unit tests (`validate_tool_request_is_fail_closed_on_mutating_and_unknown_
names`, `execute_requested_reads_is_bounded_deduped_and_read_only`); the kernel integration test
`unified_decision_tool_requests_execute_against_the_live_snapshot` (parse + validate + execute
against the live board, mutating request dropped, no mutation); and the server seam test
`cli_decision_carries_read_only_tool_requests_through_the_no_leak_seam`. No test calls a real
provider; no wire/dashboard change was needed.

## Applied change (the first safe WRITE-capable Prime tool surface)

Every brain stage and the read-only tool loop above are *propose-only* or *read-only*: the brain
could classify, sharpen slots, and inspect live state, but it could not ask Prime to *do* anything
through a governed tool contract. That is the audit's named next rung ("A WRITE-capable tool
surface"). Per master plan §10.1 (Intent Layer), §10.2 (Action Layer), §17.1, and following the
reference read recorded in `reference-driven-development.md` (Hermes `run_conversation`
one-response-carries-the-action + name-allowlist validation; openclaw `tool-mutation`
fail-closed-unsafe-default, `tool-policy` gated-capability, `update-plan-tool`/`common.ts`/
`sessions-spawn-tool` validate-the-payload-hard, the no-leak CLI-output seam), a configured brain may
now *request* a known mutating tool that Relux converts into an EXISTING action and routes through
every current validation/approval gate. The brain writes nothing directly.

- **New module `relux-kernel/src/prime_write_tools.rs`.** `WRITE_TOOLS` is the explicit, tiny
  allowlist: `task.create` → CreateTask, `task.update` → UpdateTask, `task.assign` → AssignTask,
  `task.start` → StartRun, `agent.create` → CreateAgent (all safe `Act`s), plus `plugin.install` →
  InstallPlugin and `permission.grant` → GrantPermission (both APPROVAL-GATED `Propose`s).
  `classify_write_tool` is the FAIL-CLOSED name gate (anything off the list — `task.delete`,
  `shell.run` — is refused). `parse_write_tool_request` maps the tool to its existing intent and
  validates the `args` by REUSING the existing per-action validator (`parse_task_slots`,
  `parse_update_slots`, `parse_assign_slots`, `parse_agent_slots`, `parse_plugin_ref`/
  `parse_permission_slots`; `task.start` reads a required `task_id`) — no weaker duplicate parsing;
  an unsupported field / missing required field fails the whole request closed. A committed
  confidence is stamped so the slot validators honor an explicit tool request. `reconcile_run_start`
  validates `task.start` against the live ready queue (EXISTS and is READY).
- **Carried on the unified decision.** `PrimeBrainDecision` gains `action_request: Option<…>` (parsed
  from a single `action_request` / `tool_call` object); a mutating/unknown name is dropped at parse
  time, a batched multi-tool request is refused (at most ONE per turn). The server desugars it into a
  synthesized `BrainIntentProposal` + the matching slot in the `BrainSlotProposals` bundle, fed to
  the UNCHANGED `prime_turn_with_brain` chokepoint; a new `run` bundle field + a `RunStart` promotion
  arm (mirroring the existing assign/update promotions) lets `task.start` start a named ready task.
- **Safety invariants (binding).** The brain runs NOTHING — a write tool is *desugared* into the
  existing intent+slot mechanism. The fail-closed `reconcile_intent` gate still decides: a write
  tool's intent is sensitive, so **casual chat/ideation can never trigger it** (guarded chat keeps
  the deterministic non-work intent). Every id is validated against the live state (an unknown
  task/agent fails closed); the terminal-state guard (update) and readiness guard (start) hold;
  `plugin.install` / `permission.grant` stay behind a human approval (the install/grant is never
  executed — pinned by unchanged plugin-count / permission-set assertions). Every durable change
  still flows through `decide` → `prime_execute` (safe `Act`) or approval (`Propose`).
- **UI.** A small `🛠 requested tool: <name>` provenance chip (`PrimeResponse.requested_tool` + the
  pure `requestedToolLabel` helper), present ONLY when a write tool genuinely drove an actionful
  turn (the turn is actionful AND its intent matches the tool) — a vetoed request attributes nothing,
  keeping the chip honest. No panel.

Pinned by the `prime_write_tools` unit tests (fail-closed classify, each tool → intent/slot,
unknown-name refused, unsupported-args fail-closed, batched-request refused, run-start readiness,
confident intent proposal); the `prime_decision` unit tests (`carries_a_write_tool_request_…`,
`a_mutating_or_unknown_write_tool_is_dropped_at_parse_time`,
`tool_call_is_accepted_as_an_alias_and_gated_tools_are_marked`); the kernel integration tests
(`write_tool_task_{create,update,assign,start}_maps_to_the_existing_*_path`,
`write_tool_task_start_rejects_an_unknown_or_unready_task`,
`write_tool_agent_create_maps_to_the_existing_agent_path`,
`write_tool_{plugin_install,permission_grant}_stays_approval_gated` + the unchanged-count/permission
safety assertions, `casual_chat_never_triggers_a_write_tool`,
`write_tool_assign_fails_closed_on_an_unknown_task`); the server seam test
(`cli_decision_carries_a_write_tool_request_through_the_no_leak_seam`); and the dashboard
`requestedToolLabel` test. No test calls a real provider; the dashboard bundle was rebuilt.

## Applied change (safe post-execution after-action reply shaping)

Every brain stage above composes its reply BEFORE the kernel executes, so the action-free wall
kept an ACTIONFUL turn's reply strictly deterministic (`is_actionful` → `shape_reply` keeps it
`DeterministicForAction`). The brain could classify, sharpen slots, request a governed tool, and
re-word a *conversational* turn — but never the confirmation a user reads AFTER a create / update /
assign / start / agent.create executes, or after a plugin.install / permission.grant is proposed.
That was the explicitly-deferred "after-action narration" rung. Per master plan §10.2 (Action
Layer) and §17.1, and following the reference read recorded in `reference-driven-development.md`
(Hermes `tool_executor` inject-the-real-bounded-result-then-answer + `conversation_loop`
deterministic fallback; openclaw `exec-approval-followup` succeeded/failed/did-not-run grounding +
`sanitizeUserFacingText` + the no-leak CLI-output seam), a configured brain may now re-word an
actionful turn's confirmation AFTER the kernel executed, grounded ONLY in a sanitized result
envelope and validated against it.

- **New module `relux-kernel/src/prime_after_action.rs`.** `after_action_kind(turn)` gates which
  turns are eligible: `Executed` for a safe disposition `Executed` turn, `Proposed` for an
  `AwaitingApproval` turn — and `None` for a NON-actionful turn (the clarify/brainstorm/free-form
  paths shape those), a TOOL turn (`invoked_tool`/`tool_output`/`tool_error`/`ToolDiscovery`,
  preserving the long-standing "never narrate a tool result" wall), and a high-risk action that is
  not a proposal (defensive). `build_action_envelope(turn, kind)` derives a sanitized, bounded
  `ActionEnvelope` from the ALREADY-executed turn: the result kind, a short action label, the
  concrete ids it produced/targeted, the durable `ActionFacts`, and the redacted grounded reply.
  `build_after_action_prompt` hands ONLY that to the brain with the three openclaw-style steers
  (executed = confirm / proposed = NOT done, awaiting approval / failed = do not claim success).
- **The validator is the INVERSE of `prime_clarify`.** Where the pre-execution clarify path rejects
  EVERY action claim, `parse_after_action` honors a completion claim ONLY when the envelope's
  matching fact is confirmed: a create that started no run rejects a "started the run" claim; a
  still-pending `Proposed` install rejects an "installed"/"is now installed" claim (Prime never
  EXECUTES an install/grant — those facts are NEVER set); a `Failed` envelope rejects a success
  claim; and a structured id-shaped token (`task_`/`run_`/`appr_`/`approval_`) not in the
  envelope's ids fails the reply closed (an invented id). The allowlist (`text`/`confidence`/
  `rationale`), control-char strip, length clamp, and secret/path redaction (`redact_secrets`:
  secret-prefixed tokens, high-entropy blobs, absolute unix/windows paths) mirror the slot/clarify
  discipline. `reconcile_after_action` drops a low-confidence or pure-echo proposal.
- **Both brains feed one validator.** OpenRouter via `ai::polish_after_action_via_openrouter`; the
  Claude/Codex CLI brains via `server.rs` `polish_after_action_via_cli` → the no-leak
  `parse_cli_after_action` (`parse_adapter_result` FIRST, error envelope dropped). Wired in
  `run_prime` OUTSIDE the lock, in the ACTIONFUL branch (which previously only ran `shape_reply`):
  a non-Local brain attempts the after-action call; on ANY failure it falls back to the grounded
  deterministic reply via `shape_reply`, byte-for-byte the prior behavior. `Local` always falls
  back.
- **Safety invariants (binding).** The action ALREADY ran (or was proposed) through the unchanged
  `decide` → `prime_execute` / approval path; this stage ONLY re-words the confirmation and changes
  nothing — there is no path from `prime_after_action` to a mutation. The brain can never claim
  unexecuted work, invent an id, narrate a failure as a success, or say installed/granted on a
  still-pending proposal; any such reply is rejected wholesale and the deterministic reply stands.
- **UI.** A small `🧠 after-action wording · <source>` chip (`PrimeResponse.after_action_source` +
  the pure `afterActionLabel` helper), present ONLY when a brain genuinely shaped the actionful
  turn's confirmation. The existing action/update/provenance cards are untouched.

Pinned by the `prime_after_action` unit tests (gating executed/proposed/tool/high-risk; envelope
ids/facts/redaction; valid executed confirmation; claim-of-unexecuted-work rejection; proposed
must-not-say-installed; failed must-not-claim-success; invented-id rejection; control-strip +
secret/path redaction; unsupported-field/empty-text rejection; low-confidence/echo reconcile;
prompt steer); the kernel integration tests (`after_action_shapes_a_real_create_but_changes_no_
state`, `after_action_falls_back_when_the_brain_claims_unexecuted_work`,
`after_action_proposal_must_not_say_installed_and_installs_nothing`); the server seam tests
(`cli_after_action_lifted_from_a_result_envelope`,
`cli_after_action_drops_error_envelope_contradiction_and_invented_id`,
`cli_after_action_proposal_rejects_installed_claim`); and the dashboard `afterActionLabel` test. No
test calls a real provider; the dashboard bundle was rebuilt.

## Current Prime brain stack

The end-to-end shape of one Prime turn, with the brain strictly additive and the
deterministic kernel always the authority. **A configured brain now answers the structured
turn in ONE unified decision call** (`prime_decision`: intent + applicable slots + optional
wording + read-only tool requests + an optional single governed write tool), with the
per-section specialized calls below as the fallback when that envelope is unavailable:

0. **Clarification context** — BEFORE classifying, `prime_turn_with_brain` checks for a
   bounded pending clarification from the prior turn (`prime_clarify_memory::resolve_pending`,
   keyed `namespace::actor`, TTL-bounded). A bare answer is *combined* with the stored original
   message and the combined text drives the rest of the turn; a standalone command/question
   supersedes the pending context; "never mind" cancels it; an expired record is ignored. On a
   continuation the server dispatches the slot brain on the COMBINED message (learned via the
   read-only `continuation_preview`), and the kernel keeps those slots ONLY when the bundle's
   `continuation` flag matches the turn it produced — so a fuzzy assignee or an
   extractor-missed `{task_id, agent_id}` can be brain-resolved in context, with the deterministic
   combine as the fallback. The follow-up therefore continues the original request through the
   same pipeline instead of being classified blind.
1. **Intent** — the brain *proposes* a label (`prime_intent::build_intent_prompt` →
   `parse_intent_proposal`); `reconcile_intent` is the fail-closed gate (guarded
   chat can never become work; low confidence keeps the deterministic intent;
   `create_and_run` without run language downgrades to `create`). No brain → the
   deterministic `classify_intent` decides.
2. **Validated slots** — dispatched on the resolved intent at the single chokepoint
   `prime_turn_with_brain` (a `BrainSlotProposals` bundle):
   - a **create** intent → task slots (`prime_slots`): allowlist fields, clamp
     lengths/priority, existing-agent assignee only;
   - an **`AgentCreation`** → agent slots (`prime_agent_slots`): normalized
     non-colliding id (duplicate rejected), existing-only adapter, sanitized
     role/notes — applied to the executable `create_agent`;
   - a **`PluginInstallation` / `PermissionChange`** → advisory admin subject
     (`prime_admin_slots`): a normalized plugin id, or a permission subject validated
     against the live agent roster + a `["agent"]` kind allowlist + a sanitized label;
   - an **`AssignTask`** the deterministic path could not complete → assignment slots
     (`prime_assign_slots`): `task_id` honored only when it exists, `agent_id` resolved via
     `resolve_assignee` to an existing agent, BOTH required — then it PROMOTES the turn to the
     same safe `AssignTask` action (safe + fully validated, so no approval needed).
   - a **`TaskUpdate`** the deterministic rail could only clarify → update slots
     (`prime_update_slots`): `task_id` existence-checked, fields sanitized/clamped, status held
     to the operator-settable allowlist, assignee resolved to an existing agent — then it
     PROMOTES the clarify to the same safe `UpdateTask` action (the terminal-state guard is
     enforced at apply time; Prime never decrees a fake completion).
   Any failure (no brain, low confidence, invalid JSON, unsupported field/kind,
   duplicate id, unknown adapter/subject/task) → the deterministic slot stands.
3. **Deterministic / policy execution** — `decide` → `prime_execute` is the SOLE
   path that changes durable state. Risky intents still become `Propose` behind a
   human approval; the admin slots only sharpen the *subject the human reviews* —
   `sharpen_admin_action` never changes the action's kind, so the brain runs no
   protected install or grant. Plan previews (`PlanRequest`) remain action-free, and
   orchestration steps stay owned by the deterministic `plan_orchestration` (the brain
   only *polishes* their wording).
3b. **Read-only context loop** (a NON-actionful inspection/explanation/question turn only) — before
   the reply is shaped, a configured brain may inspect live state through the GOVERNED READ-ONLY
   tools (`prime_tools`): an allowlisted read-only tool (`board_summary`/`list_tasks`/`get_task`/
   `list_agents`/`get_agent`/`list_runs`/`get_run`/`list_plugins`/`list_approvals`), validated by
   name (off-list ⇒ refused, never run) and executed deterministically against a pre-taken state
   snapshot. **UNIFIED-FIRST:** when the ONE decision envelope (step 0c) already carried
   `tool_requests`, the kernel executes those validated read-only requests deterministically
   (`execute_requested_reads`, bounded by `MAX_TOOL_ROUNDS`, repeated reads skipped) with NO second
   brain loop. **FALLBACK:** when the envelope requested none (or there was no usable decision), the
   sidecar multi-round `ContextLoop` runs (request → validate → inject → re-prompt, bounded by
   `MAX_TOOL_ROUNDS`, stop-on-repeat) — so there is never duplicate execution. Either way the reads
   change nothing and only GROUND the reply (folded into `grounded_facts`, surfaced as
   `context_reads` provenance). `Local` / an actionful turn gathers nothing.
3c. **Governed WRITE tool** (an explicitly-commanded turn only) — the ONE unified decision may carry
   a single `action_request` naming an allowlisted WRITE tool (`prime_write_tools`): `task.create` /
   `task.update` / `task.assign` / `task.start` / `agent.create` (safe `Act`s) or `plugin.install` /
   `permission.grant` (approval-gated `Propose`s). The name is fail-closed validated
   (`classify_write_tool`); the args are validated by REUSING the existing per-action slot validator;
   the tool is *desugared* into a synthesized intent proposal + the matching slot fed to the
   UNCHANGED `prime_turn_with_brain` chokepoint (step 2/3). So the fail-closed intent gate still
   vetoes a mutating tool on guarded chat, every id is validated against the live state, the
   terminal-state/readiness guards hold, and a risky tool stays behind a human approval — the brain
   requests; `decide` → `prime_execute` / approval is still the SOLE path that changes durable state.
   At most ONE write tool per turn; the chip `🛠 requested tool: <name>` appears only when it
   genuinely drove an actionful turn. `Local` / a vetoed request changes nothing.
4. **UI response** — the reply text may be brain-shaped on a conversational turn. A
   **clarify / brainstorm / single-step plan** turn goes through the VALIDATED wording
   path (`prime_clarify`: one schema-checked question / short summary, action claims
   rejected); other non-actionful conversational turns PREFER the `reply` the SAME unified
   decision already carried (`validated_reply`, the brainstorm chokepoint), falling back to
   the free-form `shape_reply` / CLI brain only when the envelope omitted it. A plan card may
   carry an advisory polish overlay — likewise PREFERRED from the unified decision's
   `plan_polish` (`validated_polish`, the same `validate_polish` index-match chokepoint) with
   the dedicated `polish_proposal` call as the fallback. A sharpened create carries a
   `slots` card, a sharpened agent an `agent_slots` card (incl. a seeded **persona**),
   and a sharpened risky `Propose` an advisory `admin_slots` card. Every brain
   contribution is labeled (intent → `brain-classified`; re-worded reply → `🧠
   brain-worded question/reply`; polish/slots → the model id or CLI label via the shared
   `brainSourceLabel`) and is presentation/provenance only — never a fresh authority. When
   the turn leaves a clarification pending, the response carries `pending_clarification` and
   the chat shows a `⏳ waiting for: <needs>` chip with a Cancel action, so the user knows the
   next message will be read as the answer.
4b. **Post-execution (after-action) wording** (an ACTIONFUL non-tool turn only) — the reply on an
   actionful turn was always deterministic, because the brain composed its decision BEFORE the
   kernel executed. After `decide` → `prime_execute` / approval has ALREADY run, a configured brain
   may re-word the FINAL confirmation through the VALIDATED after-action path (`prime_after_action`):
   the kernel builds a sanitized, bounded `ActionEnvelope` (kind executed/proposed/failed, the
   action label, the concrete ids, the durable facts, the redacted grounded reply) and the brain
   answers grounded ONLY in it. The validator is the INVERSE of the clarify path: a completion claim
   is honored ONLY when the envelope's fact is confirmed; a success claim on a failure, an
   installed/granted claim on a still-pending proposal, or an invented id all fail the reply closed,
   and secret-shaped tokens / absolute paths are redacted. Any failure (no brain, low confidence,
   contradiction, invented id, echo) falls back to the grounded deterministic reply. The action
   already ran; this changes only the wording. The chip `🧠 after-action wording · <source>` appears
   only when the brain genuinely shaped an actionful confirmation. A TOOL turn (real kernel output)
   keeps the deterministic reply, and `Local` always falls back.

## Next recommended slice

The **read-only context tools** now cover the local control plane: on an inspection/explanation/
question turn the brain inspects live state (a task, the crew, the runs, **a single run, the
installed plugins, the approval queue**) through a governed, fail-closed, bounded read-only loop
before answering. The obvious next rungs build on it:

- **`get_plugin` / `get_approval` detail reads** — the list tools (`list_plugins`/`list_approvals`)
  now exist; a by-id detail read for one plugin (its tools + executable status via `discover_tools`)
  or one approval is the natural completion, each a pure projection + an allowlist entry. The
  raw provider `usage`/`cost` on a run stays deliberately unexposed (redaction), so any richer run
  read must keep that boundary.
- **~~A WRITE-capable tool surface~~ (DONE)** — the unified decision now carries a single governed
  `action_request` naming an allowlisted WRITE tool (`prime_write_tools`: `task.create`/`task.update`/
  `task.assign`/`task.start`/`agent.create` safe; `plugin.install`/`permission.grant` approval-only),
  desugared into the EXISTING intent + slot mechanism so it flows through the unchanged fail-closed
  `decide` → `prime_execute` (safe `Act`) / human-approval (`Propose`) path. `classify_write_tool` is
  the fail-closed gate (unknown ⇒ refused), and the SAME `reconcile_intent` gate keeps a mutating
  tool off guarded chat. The next rungs build on it: a **multi-round write loop** (request → observe
  → act INSIDE the one envelope flow, which needs the decision call itself to loop) and **richer
  write tools** (e.g. `run.retry`, an `orchestration` tool) as each proves out.
- **~~After-action narration~~ (DONE)** — a brain reply on an ACTIONFUL turn, the post-execution
  re-shaping pass that preserves the action-free wall (`prime_after_action`). After the kernel has
  ALREADY executed (or proposed) the action, the brain re-words the confirmation grounded ONLY in a
  sanitized `ActionEnvelope` and validated against it (a completion claim is honored only when its
  fact is confirmed; a success-on-failure, installed/granted-on-a-proposal, or invented id is
  rejected; secrets/paths redacted), falling back to the deterministic reply on any failure. The
  remaining deferral here is a **multi-round write loop** (act → observe the result → act again
  INSIDE the one envelope flow), which needs the decision call itself to loop.
- **~~Read context on the unified decision~~ (DONE)** — the unified `PrimeBrainDecision` now carries
  the brain's read-only `tool_requests` alongside intent+slots+wording; Relux executes the validated
  reads deterministically (`execute_requested_reads`, no second brain loop, no duplicate execution)
  and grounds the reply in them, with the sidecar `ContextLoop` as the fallback. The next coherence
  win is a **multi-round** read on the unified call (request → observe → request again INSIDE the one
  envelope flow before emitting the final intent/slots/reply), which needs the decision call itself
  to loop; the single-pass up-front request shipped first.

Multi-turn clarification continuation is now smart on four fronts: a **fuzzy assignee** resolves
against the live roster (deterministic), a **by-id run start** is wired and its clarify is
remembered, a **brain-assisted continuation** can resolve the `{task_id, agent_id}` the
extractors miss, and a **by-id task update** is now a real, validated mutating action whose
clarify the memory continues ("change task priority" → "task_0001 to 8"). The remaining surfaces
are narrower:

- **More update fields** — the by-id update supports title/details/priority/status/assignee.
  A **due date** (`Task.deadline`) and **labels** are the obvious next fields, but a label field
  does not exist on the `Task` model yet and a free-text deadline needs date-validation
  infrastructure; both should be added to the data model first rather than faked into the patch.
- **The remaining slot field-extractors** (`update_change_phrase`, `orchestration_goal`,
  `plan_goal`) are still deterministic string-slicing. They are the *grounding* the brain
  reconciles against and the fallback when no brain is live, so they should stay.
- **A persona for the manual Crew create form** — today persona is brain-seeded only; a
  small optional persona input on the Crew form (validated/clamped the same way) would let an
  operator set one without the brain.
- **The free-form reply + plan-polish are now folded into the unified decision** (one call on the
  common chat / plan-preview turns), so the only remaining separate brain call is the dedicated
  fallback when the envelope omits a section. The deliberately-deferred piece is a brain reply on
  an **actionful** turn (a short after-action explanation): the brain composes its reply before the
  kernel executes, so an honest after-action narration needs a post-execution re-shaping pass — a
  future slice that must preserve the action-free wall (no claim of work that did not happen).
