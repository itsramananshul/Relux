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

## Current Prime brain stack

The end-to-end shape of one Prime turn, with the brain strictly additive and the
deterministic kernel always the authority:

0. **Clarification context** — BEFORE classifying, `prime_turn_with_brain` checks for a
   bounded pending clarification from the prior turn (`prime_clarify_memory::resolve_pending`,
   keyed `namespace::actor`, TTL-bounded). A bare answer is *combined* with the stored original
   message and the combined text drives the rest of the turn (deterministic only — the
   raw-answer brain proposals are dropped); a standalone command/question supersedes the pending
   context; "never mind" cancels it; an expired record is ignored. The follow-up therefore
   continues the original request through the same pipeline instead of being classified blind.
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
     against the live agent roster + a `["agent"]` kind allowlist + a sanitized label.
   Any failure (no brain, low confidence, invalid JSON, unsupported field/kind,
   duplicate id, unknown adapter/subject) → the deterministic slot stands.
3. **Deterministic / policy execution** — `decide` → `prime_execute` is the SOLE
   path that changes durable state. Risky intents still become `Propose` behind a
   human approval; the admin slots only sharpen the *subject the human reviews* —
   `sharpen_admin_action` never changes the action's kind, so the brain runs no
   protected install or grant. Plan previews (`PlanRequest`) remain action-free, and
   orchestration steps stay owned by the deterministic `plan_orchestration` (the brain
   only *polishes* their wording).
4. **UI response** — the reply text may be brain-shaped on a conversational turn. A
   **clarify / brainstorm / single-step plan** turn goes through the VALIDATED wording
   path (`prime_clarify`: one schema-checked question / short summary, action claims
   rejected); other conversational turns go through the free-form `shape_reply` / CLI
   brain. A plan card may carry an advisory polish overlay, a sharpened create carries a
   `slots` card, a sharpened agent an `agent_slots` card (incl. a seeded **persona**),
   and a sharpened risky `Propose` an advisory `admin_slots` card. Every brain
   contribution is labeled (intent → `brain-classified`; re-worded reply → `🧠
   brain-worded question/reply`; polish/slots → the model id or CLI label via the shared
   `brainSourceLabel`) and is presentation/provenance only — never a fresh authority. When
   the turn leaves a clarification pending, the response carries `pending_clarification` and
   the chat shows a `⏳ waiting for: <needs>` chip with a Cancel action, so the user knows the
   next message will be read as the answer.

## Next recommended slice

Multi-turn clarification memory now carries the prior question's context into the next turn
(see the applied change above), so a follow-up answer resolves the original request without
re-stating it. The remaining keyword surfaces are narrower:

- **The slot field-extractors themselves** (`extract_task_id`,
  `extract_agent_id_from_assignment`, `update_change_phrase`, `orchestration_goal`,
  `plan_goal`) are still deterministic string-slicing. They are the *grounding* the brain
  reconciles against and the fallback when no brain is live, so they should stay — but the
  `AssignTask` arm (today pure `extract_task_id` + `extract_agent_id_from_assignment`) is a
  candidate for the same validated-slot treatment (a brain proposes `{task_id, agent_id}`,
  reconciled against `summary.all_task_ids` / `all_agent_ids`). This would also let the
  *continuation* path resolve a fuzzy assignee ("the researcher") the deterministic extractor
  misses today.
- **Brain-assisted resolution of a bare follow-up answer** — the continuation path is
  deterministic-only on a `Continue` (the bare answer is combined and re-classified); a
  validated brain extractor proposing `{task_id, agent_id}` from the combined message
  (reconciled against the live roster) would let a brain sharpen an answer the extractors miss,
  while the deterministic combine stays the fallback.
- **By-id run-start / task-update actions** — a run-start / task-update clarify is deliberately
  NOT remembered today because no `StartRun { task_id }` / `UpdateTask` action is wired for
  Prime to resolve it into. Wiring those actions would let the same memory resolve "start it"
  → "task_0001" and "raise the priority" → "task_0001 to 8".
- **A persona for the manual Crew create form** — today persona is brain-seeded only; a
  small optional persona input on the Crew form (validated/clamped the same way) would let an
  operator set one without the brain.
