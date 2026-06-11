# Changelog

All notable changes to Relix are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once a stable release is cut.

## [Unreleased]

### Added

- **Relux local release v0.1.15 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.14` to `0.1.15`, bundling the post-v0.1.14
  **cross-platform source launcher + read-only kernel Doctor** slice into a fresh
  Windows release. No master-plan safety property is weakened: both surfaces are
  read-only / launch-only and leak no paths or secrets. Headlines:
  - **Cross-platform `start-relux.sh` source launcher (macOS/Linux).** A Bash
    counterpart to `Start-Relux.ps1` for Unix-like source checkouts: it locates the
    repo root, checks `cargo` (printing the [rustup](https://rustup.rs) install step
    if missing), builds/reuses `target/debug/relux-kernel` (`--release` optional,
    `RELUX_CARGO_JOBS` cap), sets the same `RELUX_HTTP_ADDR` / `RELUX_DB` /
    `RELUX_DASHBOARD_DIST` env vars, preflights the loopback port with an actionable
    busy-port error (`nc`, falling back to bash `/dev/tcp`), and runs `serve` in the
    foreground (Ctrl+C to stop). Flags: `--port`, `--release`, `--dry-run`,
    `--doctor`, `--help`. The README now separates the three launch paths (prebuilt
    Windows zip; Windows source via `Start-Relux.ps1`; macOS/Linux source via
    `./start-relux.sh`) and is explicit that the packaged zip is Windows-x64 only;
    `.gitattributes` pins `*.sh` to LF so the shebang works on Unix regardless of
    `core.autocrlf`.
  - **Read-only kernel Doctor report + dashboard panel (`relix-dashboard-design.md`
    §15.1).** A new session-protected `GET /v1/relux/doctor` emits a structured,
    read-only diagnostics report. It reuses the same cheap reads as
    `/v1/relux/health` (store open/load, dashboard bundle, AI status, adapter + tool
    readiness, agent + approval counts) and returns `ok`/`info`/`warn`/`fail` rows
    each with a message, remediation, and an in-app action link. No heavy work, no
    mutation, and no paths/secrets — `DoctorInputs` carries no filesystem path
    (structural redaction), and the severity rules mirror `readiness.ts` so the two
    surfaces agree. The dashboard gains a compact Doctor panel on Health below the
    readiness guide, sorted worst-first, with Fix links and a Refresh and an honest
    error state (never a blank panel); pure helpers live in `doctor.ts`. Proven by
    `doctor.rs` unit tests (every severity rule + redaction), a server test pinning
    session-gating / the row set / no db-path leak, frontend `doctor.test.ts`, and
    `doctor-render.test.mjs` (ok/warn/fail/error/loading render states + the
    committed bundle).

  Built reference-first per `docs/reference-driven-development.md` (Hermes
  `doctor.py` check_*/_fail_and_issue; openclaw `health-state.ts` includeSensitive).
  The tracked `dashboard-dist` bundle was rebuilt and committed in sync. Build the
  bundle with `scripts\relux-package-local.ps1 -FullE2E`. This version line is the
  `relux-kernel` crate version (separate from the legacy Relix workspace versions in
  the dated sections below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.14 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.13` to `0.1.14`, bundling the post-v0.1.13
  **manual Crew configuration + permissions governance** slice (`relix-dashboard-design.md`
  §9 / §9.1) into a fresh Windows release. No master-plan safety property is weakened:
  every new surface is operator-driven, fails closed, and `create_agent` still grants only
  the minimal echo tool. Headlines:
  - **Manual create/edit of crew.** The Crew page gains a shared create/edit form (name,
    id, role, persona, adapter/runtime, status) backed by a validated kernel edit path, so
    the manual surface matches what the brain could already seed. New
    `agent_config.rs` does pure, unit-tested validation/sanitization (name required, strict
    id shape, id+name uniqueness, adapter must resolve to a known/installed adapter, status
    allowlist, persona bounded + secret-redacted); `KernelState::update_agent` is
    field-granular (absent = unchanged, `Some(None)` clears persona) and audited;
    `POST /v1/relux/agents` now accepts persona and runs the validator, and a new
    `PATCH|PUT /v1/relux/agents/:id` edits name/role/persona/adapter/status. Validation
    failures are honest `400`s; a missing agent on edit is `404`.
  - **Explicit-permission view + safe revoke.** Crew cards now list explicit permissions
    (elevated control-plane grants flagged) instead of just a count, and the edit card gains
    a Governance section to grant/revoke. `KernelState::revoke_permission_from_agent`
    removes an explicit permission, audits it, and fails closed
    (`KernelError::PermissionNotGranted` → `404`) when the agent does not hold it, exposed
    via `DELETE /v1/relux/agents/:id/permissions`. A pure, unit-tested `governance.ts`
    mirrors the relux-core `VALID_PREFIXES` for client-side validation and classifies
    control-plane prefixes as elevated → a deliberate confirm before granting; nothing
    dangerous is auto-granted, and Prime's own `GrantPermission` stays approval-gated.
  - **Model-backed skills/tags + skill-aware assignment.** A bounded specialty-tag list is
    added to `relux_core::Agent` (serde-default, snapshot backwards-compatible) and used in
    Prime fuzzy assignee resolution: a skill held by exactly one agent routes work to that
    specialist; a shared skill is ambiguous (Prime asks, never guesses); an exact id/name
    still wins. Skills are validated/sanitized/clamped (strict slug, dedup, bounded count)
    on the manual create/edit path and surfaced as chips in the Crew UI. Skills are
    specialty for routing only, never a capability gate.
  - **Safe role presets for Crew create.** Curated role-preset bundles (researcher, builder,
    reviewer, planner, operator) seed the create form's role/persona/skills via a read-only
    `GET /v1/relux/agent-presets` (single source of truth, pure + unit-tested in
    `agent_presets.rs`). `POST /v1/relux/agents` accepts an optional preset id that fills
    only the role/persona/skills the request omitted (request value wins) and flows through
    the SAME validators; an unknown preset is an honest `400`. The `AgentPreset` type carries
    no permission/adapter field, so a preset SUGGESTS configuration only and cannot widen an
    agent's power.

  Built reference-first per `docs/reference-driven-development.md` (openclaw
  sessions-spawn-tool / approval-classifier / tool-policy, Hermes system_prompt +
  message_sanitization) and conforms to `docs/relix-dashboard-design.md` §9 / §9.1. Proven by
  new `agent_config` / `agent_presets` / `governance` unit tests, extended
  `agent_create_and_edit_workflow_over_http` (grant/revoke/`404`/`400`) and
  `agent_presets_list_and_create_with_preset_over_http` kernel tests, dashboard
  `governance.test.ts` / `presets.test.ts`, and the `crew-render` harness; the tracked
  `dashboard-dist` bundle was rebuilt and committed in sync. Build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. This version line is the `relux-kernel` crate
  version (separate from the legacy Relix workspace versions in the dated sections below).
  See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.13 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.12` to `0.1.13`, bundling the post-v0.1.12
  **in-app first-run / operational readiness guide + dashboard build hygiene**
  slice into a fresh Windows release. No new product surface, no new endpoint, and
  no master-plan safety property is weakened: this line is entirely dashboard-side
  and makes the Home/Health first-run experience honest and the dashboard build
  warning-clean. Headlines: a derived, honest **readiness guide** on Home and
  Health. A new pure `apps/dashboard/src/.../readiness.ts` `buildReadiness()` turns
  the four control-plane reads Home already makes (state, `ai/status`, adapters,
  plugins+tools) into one report — **no new endpoint** — with items for Prime brain
  (reusing `onboarding::primeBrainStep`), real-work adapter, crew (with local-Prime
  fallback), plugins/tools (reusing `plugins::pluginCategory`/`toolReadiness`), and
  pending approvals; a *selected-but-broken* brain is the only blocker, a local
  brain works, and metadata wrappers / unconfigured runtimes are surfaced as
  attention — **never a faked green check**. `ReadinessGuide.tsx` renders one
  compact card (setup mode = checklist with per-item action links; operational mode
  = concise summary + single first action behind `<details>` so a configured
  instance is not nagged), shared on both Home (replacing the old static checklist)
  and Health (built from the same derivation over the reads Health already makes —
  no duplicated business logic), and Home's redundant "Run real work: Claude/Codex
  adapters" prose card was dropped. **Partial-read honesty:** `buildReadiness` now
  distinguishes a *failed* read from one still *in flight* via a `ReadinessFailed`
  flag set (state/ai/adapters/plugins/tools) — a failed read becomes an explicit,
  retryable "… unavailable" warn row and marks the report degraded (`ready` forced
  false) so a Health-OK-but-state-read-failed instance can never paint a faked
  "operational" badge from partial data, while a still-loading null read stays a
  neutral "Checking readiness…" row. **Build hygiene:** the dashboard `typecheck`
  script now type-checks each project directly (outside `tsc -b` build mode) so
  `npm run typecheck` passes instead of failing TS6310 on the composite
  `tsconfig.node.json`, and route-level `React.lazy` + a `manualChunks` vendor rule
  replace the old single ~653 kB bundle (largest chunk now the ~165 kB vendor
  chunk) so `vite build` no longer warns about chunks over 500 kB — same components
  at the same paths, now per-route chunks behind a Suspense fallback. Built
  reference-first per `docs/reference-driven-development.md` (in-app readiness
  guide) and conforms to `docs/relix-dashboard-design.md` §15. Proven by
  `readiness.test.ts` (the four states + blocker + first-action priority +
  failed-vs-loading per read) and the `readiness-render`/`health-render`/
  `readiness-guide-render` `.mjs` render harnesses; the tracked `dashboard-dist`
  bundle was rebuilt and committed in sync. Build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. This version line is the
  `relux-kernel` crate version (separate from the legacy Relix workspace versions
  in the dated sections below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.12 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.11` to `0.1.12`, bundling the post-v0.1.11
  **source-checkout launcher + bounded Prime conversation memory** slice into a fresh
  Windows release. No new product surface and no master-plan safety property is weakened;
  this line makes the documented one-command boot actually work from a cloned repo and
  gives Prime's brain a small, fenced sense of recent context. Headlines: a **root
  source-checkout `Start-Relux.ps1`** (separate from the prebuilt bundle launcher of the
  same name) so the documented
  `powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1` works from the
  repo root — it locates the workspace root via `$PSScriptRoot` with a guard, builds or
  reuses `target\{debug,release}\relux-kernel.exe` (cold builds capped via
  `scripts\cargo-jobs.ps1`), points the kernel at the committed `dashboard-dist` and the
  gitignored `dev-data\` store, runs the same loopback port preflight as the bundle
  launcher, prints the dashboard URL, and serves in the foreground (flags `-Port`,
  `-Release`, `-DryRun`, `-Doctor`, `-Help`). The product change is **bounded conversation
  memory**: a small, bounded, secret-redacted per-conversation turn history
  (`relux_core::ConversationTurn`; `relux-kernel/prime_history.rs` with
  `MAX_HISTORY_TURNS=12`, `MAX_HISTORY_CONVERSATIONS=32`, `MAX_CONTEXT_CHARS=2000`) lets
  Prime's brain interpret follow-ups ("what about the second one?", "do that again") in
  context instead of reasoning from the bare current message + a state snapshot. It is
  persisted via the meta-snapshot seam (like `pending_clarifications`), injected into
  `build_decision_prompt` as a labelled BACKGROUND block BEFORE the current message (empty
  history leaves the decision prompt byte-for-byte unchanged), and recorded AFTER the reply
  is shaped — so the stored reply is the FINAL user-visible reply (pinned by
  `recorded_reply_is_the_final_shaped_reply_not_the_grounded_one`), with each read-only
  context tool surfaced as a bounded "(consulted: …)" sub-line and **never** raw tool JSON
  or a provider envelope. Crucially the history is **advisory prompt context with zero
  authority** — it never reaches the deterministic `classify_intent`, the fail-closed
  `reconcile_intent` gate, or any existence/approval check (those run on the current message
  alone), so it can never promote casual chat into work or override an explicit current-turn
  intent; a new `POST /v1/relux/prime/reset` (and a small in-UI Clear button) wipes only
  this advisory memory. Built reference-first per
  `docs/reference-driven-development.md` (Hermes `run_conversation` history threading +
  `build_memory_context_block` fence + redact; openclaw hook-history slice +
  `buildCliSessionHistoryPrompt` + transcript-redact). Build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. This version line is the `relux-kernel` crate
  version (separate from the legacy Relix workspace versions in the dated sections below).
  See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.11 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.10` to `0.1.11`, bundling the post-v0.1.10 **plugin
  tool-invocation** slice into a fresh Windows release. v0.1.10 closed the Prime
  observe-then-act + governed-orchestration line; this line makes the ToolSet-plugin
  tool-invocation surface honest and usable end-to-end on the dashboard, with no
  master-plan safety property weakened. Headlines: **in-UI tool configuration for
  metadata-only plugin wrappers** — a new fail-closed `plugin_tool_config` parser
  (allowlisted fields, sanitize/clamp, `RiskLevel` allowlist) plus
  `KernelState::configure_plugin_tool` / `remove_plugin_tool` add or replace one tool on
  an installed, non-bundled ToolSet manifest, transactionally on a re-validated clone,
  with the permission DERIVED (`tool:<id>:<verb>`, never operator-supplied), exposed via
  `POST`/`DELETE /v1/relux/plugins/:id/tools` and an in-UI add-a-tool form (the
  copy/download manifest template drops to an Advanced fallback). An **honesty fix** makes
  the manifest `approval` field load-bearing for the first time via
  `relux_core::approval_blocks_direct_invocation` behind a new
  `ToolExecutability::NeedsApproval` refusal in `call_tool`/`invoke_tool`, so a
  non-low-risk configured tool is never runnable just because a loopback runtime is
  enabled (bundled fixtures are `approval:never`, so unchanged). A single **honest
  readiness classifier** (`toolReadiness` in `apps/dashboard/src/plugins.ts`, mirroring
  openclaw `approval-classifier`) maps the kernel's six executable states to
  `{ runnable, label, tone, reason, nextStep }` with `runnable` true only for `ready`;
  every non-ready tool now renders an inline "Why not?" panel stating the refusal/disabled
  reason + next step instead of a terse "not callable", and tools stay inline on the
  Plugins page (a non-ready tool never opens a blank page). The capstone is a **real
  per-tool-call approval flow** for gated tools: an operator requests approval for ONE
  specific invocation (tool id + exact args) via `request_tool_invocation_approval`
  (`POST /v1/relux/tools/request-approval`) — validating the tool exists, the subject
  holds its permission, the tool actually requires approval, and the args are bounded —
  which creates a Pending Approval + a `PendingToolInvocation` binding to the exact
  `(plugin, tool, agent, args snapshot + SHA-256)`; `execute_approved_tool_invocation`
  (`POST /v1/relux/approvals/:id/execute`) runs only when Approved AND unconsumed,
  re-validates existence/permission/args-hash, executes the STORED snapshot (never
  client-resupplied args), and consumes the binding on a single attempt (success or
  failure), with the Approvals page showing the bound tool + a secret-redacted args
  preview and an Execute-once button. Built reference-first per
  `docs/reference-driven-development.md` (openclaw two-phase
  `registerExecApprovalRequest` + consume-once handoff + approval-classifier;
  `readPlanSteps`/`sessions-spawn` per-entry validation). No blanket/reusable grant; no
  remote/non-loopback execution; `decide → prime_execute / approval` stays the sole path
  that changes durable state. Build the bundle with `scripts\relux-package-local.ps1
  -FullE2E`. This version line is the `relux-kernel` crate version (separate from the
  legacy Relix workspace versions in the dated sections below). See
  `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.10 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.9` to `0.1.10`, bundling the post-v0.1.9 **Prime
  observe-then-act + governed orchestration** slice into a fresh Windows release. v0.1.9
  gave the brain a single-shot governed tool surface; this line lets one turn *inspect
  then act* and extends the safe write surface to orchestration, while every master-plan
  safety property holds and the brain still changes no state directly. Headlines: a
  **bounded observe-then-act decision loop** — the unified `PrimeBrainDecision` call now
  loops (`DecisionLoop` / `MAX_DECISION_ROUNDS`), so each round the brain may request
  read-only context tools (run deterministically against the pre-taken snapshot and
  re-asked, grounded in the results) or commit one decision; the observe phase has no
  mutation path, the eventual action still flows through the unchanged fail-closed
  `reconcile_intent` gate + `decide → prime_execute` (safe Act) / human approval (risky
  Propose), and the loop is bounded, stops on no-progress, and yields an interim decision
  on failure (the first round's prompt is byte-for-byte the prior single-shot). A
  **governed `orchestration.create` write tool** maps to the EXISTING deterministic
  `plan_orchestration → prime_orchestrate` (OrchestrateGoal) path — the brain proposes
  only the goal text (advisory step hints); the deterministic planner keeps full
  authority over briefs, role classification, live-roster agent grounding, the step cap,
  the dependency DAG, and the multi-agent gate it can never bypass, and the
  sensitive-intent gate keeps guarded chat from ever triggering a create. A governed
  **`orchestration.start` write tool** (new `PrimeIntent::OrchestrationRun` /
  `PrimeAction::RunOrchestration`) runs an EXISTING governed batch: `prime_execute`
  validates the `orch_` id against live records (unknown → honest reply, fail closed)
  then runs the existing `run_orchestration` batch (max 25, concurrency 2), with
  multi-turn clarify memory ("run the orchestration" → "which one?" → "orch_0001") and a
  deterministic run reply. On the dashboard, the **Plugins page now shows live adapter
  runtime state inline** (read from the same `GET /v1/relux/adapters` probe the Crew
  section uses: `local_deterministic` / `available` / `missing_binary` / `disabled` /
  `needs_configuration`, fail-closed to an honest "status unavailable" on an errored
  probe), and **protected Claude/Codex adapter rows now expose a real "Configure" path**
  to `/crew` instead of a dead-end "locked" action (protected = locked against removal
  only, not against use). Built reference-first per
  `docs/reference-driven-development.md` (Hermes `run_conversation` bounded loop +
  allowlist validation + bounded result injection; Paperclip/openclaw fail-closed
  read-only mutation gate, `update-plan`/`sessions-spawn` per-entry validation, and
  manifest tool-availability surfaced honestly) and audited in
  `docs/prime-processing-audit.md`. Build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. This version line is the `relux-kernel`
  crate version (separate from the legacy Relix workspace versions in the dated sections
  below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.9 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.8` to `0.1.9`, bundling the post-v0.1.8 **Prime
  tool-use loop** into a fresh Windows release. v0.1.8 made Prime brain-mediated for
  intent/slots/wording; this line gives the brain a *governed tool surface* — first to
  read live control-plane state, then to request a single mutating action — while every
  master-plan safety property holds and the brain still changes no state directly.
  Headlines: a **safe read-only context/tool loop** so Prime inspects live state through
  a fail-closed, bounded allowlist (`get_run`, `list_plugins`, `list_approvals`, and the
  state views) before answering — the brain proposes tool names, an allowlist gate drops
  any mutating/unknown name at parse time, the loop is capped by `MAX_TOOL_ROUNDS`, and
  the reply is grounded only in the redacted observations (no raw provider envelope, no
  path to `prime_execute`); these read requests now also ride the **unified decision
  envelope** (validated through the same allowlist the sidecar loop uses), with
  **dashboard provenance** surfacing a compact `used: <tool>` chip plus a bounded,
  collapsed per-read detail. The capstone is the **first safe WRITE-capable tool
  surface**: a configured brain may request ONE governed mutating tool per turn
  (`task.create`, `task.update`, `task.assign`, `task.start`, `agent.create` as safe
  Acts; `plugin.install` and `permission.grant` as approval-gated Proposes), which Relux
  desugars into an EXISTING Prime action/proposal and routes through every current
  slot/intent/approval gate — the fail-closed intent gate still vetoes a mutating tool on
  guarded chat, every id is validated against live state, batched mutating requests are
  refused, and `decide → prime_execute / approval` stays the sole path that changes
  durable state. Finally, **safe post-execution after-action narration**: after the
  kernel has already executed (or proposed) an action through the unchanged path, a brain
  may re-word the FINAL confirmation grounded ONLY in a sanitized, bounded result
  envelope and validated against it (completion claims honored only when the fact is
  confirmed; success-on-failure, installed/granted-on-proposal, and invented ids
  rejected; secrets/paths redacted), changing no state. Built reference-first per
  `docs/reference-driven-development.md` (Hermes' allowlist-validated tool loop +
  inject-the-real-bounded-result grounding; Paperclip/openclaw's fail-closed mutation
  gate and exec-approval followup; open-webui's collapsed tool-call display) and audited
  in `docs/prime-processing-audit.md`. Build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. This version line is the `relux-kernel`
  crate version (separate from the legacy Relix workspace versions in the dated sections
  below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.8 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.7` to `0.1.8`, bundling the post-v0.1.7
  **Prime intelligence** slice into a fresh Windows release. This line makes Prime
  *brain-mediated end to end* while keeping every master-plan safety property: the
  deterministic keyword cascade is now only the **fallback rail**, a configured brain
  (OpenRouter or the local Claude/Codex CLI) genuinely decides each turn, and every
  brain output is validated against the live state behind a **fail-closed gate**
  before anything mutates. Headlines: **brain-mediated intent classification** (the
  brain proposes a `PrimeIntent`, validated against the allowlist and reconciled by a
  safety gate that may sharpen but never weaken a misread); **brain-assisted, validated
  slots** for task creation (title/details/assignee/priority), agent hiring, plugin
  install, and permission grants, plus **brain-refined clarification wording with a
  persona seed**; **multi-turn clarification memory** so a follow-up answer ("task_0001
  to 8") continues the prior clarify instead of starting over; **roster-aware fuzzy
  assignee resolution** and **brain-assisted assignment continuation**; **by-id run
  start** with a resolvable run-start clarification; **safe by-id task UPDATE** as a
  real mutating action (allowlisted fields, clamped/sanitized values, terminal-state
  guard, no fake completions); and the capstone **unified Prime brain decision
  envelope** — one call now carries intent + slots + clarification wording + the
  conversational reply + the plan-preview polish, computed off-lock and validated
  post-turn through the existing chokepoints (`validate_polish`, `parse_adapter_result`,
  the slot/intent gates) so a single brain round trip drives the whole turn without
  loosening any guard. Built reference-first per `docs/reference-driven-development.md`
  (Hermes' allowlist-validated tool loop + `coerce_tool_args` sanitization;
  Paperclip/openclaw's fail-closed mutation gate, balanced-JSON parsing, and
  `update-plan-tool` status allowlist) and audited in `docs/prime-processing-audit.md`.
  Build the bundle with `scripts\relux-package-local.ps1 -FullE2E`. This version line is
  the `relux-kernel` crate version (separate from the legacy Relix workspace versions in
  the dated sections below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Safe by-id task UPDATE for Prime (post-v0.1.7).** `PrimeAction::UpdateTask
  { task_id, patch }` is now a REAL, safe mutating action instead of an always-clarify
  dead end (`crates/relux-kernel/src/prime_update_slots.rs`). A deterministic rail
  parses simple commands ("rename task_0001 to Fix the login blank page", "set
  task_0001 priority to 8", "cancel task_0001", "reassign task_0001 to the researcher")
  and a configured brain resolves the references the extractors miss ("change task
  priority" → a validated `{task_id, priority}`), both validated hard before any
  mutation. **Supported fields:** title, details (folded into the task input),
  priority (clamped 1-9), status (operator-settable **blocked / cancelled** only),
  assignee (resolved to an existing agent). Safety: the `task_id` must exist; field
  names are allowlisted; values are sanitized/clamped; an assignee must match the live
  roster; a **terminal-state guard** refuses editing a completed/failed/cancelled/
  expired task; and Prime **never decrees a fake completion** — "mark it done" is
  honestly refused (completion flows through the run lifecycle). The brain may promote
  an under-specified `TaskUpdate` clarify to the same safe action ONLY when both the
  task and the change validate against the live state; any failure leaves the
  deterministic clarify/honest-reply in place. The multi-turn clarify memory now
  records a `TaskUpdate` clarify ("change task priority" → "task_0001 to 8" continues
  it), the chat shows a "what changed" card with a `🧠 <source>` chip when a brain
  resolved the change, and the classifier recognizes a task-anchored field command as
  an update (a *question* about a task stays a conversation). Reference-grounded in
  openclaw's `update-plan-tool` (schema + status allowlist), `tool-mutation`
  (mutating-action classifier), `sessions-spawn-tool`/`common.ts` (reject unsupported
  keys, require/clamp), and Hermes' `coerce_tool_args` / sanitization
  (`docs/reference-driven-development.md`, `docs/prime-processing-audit.md`).

- **Brain-mediated Prime intent classification (post-v0.1.7).** Prime's intent is
  no longer decided by the keyword cascade alone. When a real brain is configured
  (OpenRouter, or the local Claude / Codex CLI) it now *proposes* the intent of a
  message through a structured, JSON-only decision stage
  (`crates/relux-kernel/src/prime_intent.rs`): the proposed label is validated
  against the `PrimeIntent` allowlist (an off-list label is rejected), and a
  **fail-closed reconciliation gate** keeps the master-plan safety semantics — a
  brain may sharpen a misread intent (e.g. "could you take care of the login bug"
  now becomes a task instead of a generic chat reply), but it can **never** mint or
  run work from guarded chat (ideation/questions without an explicit command),
  low-confidence proposals keep the deterministic intent, and a
  create-and-run without explicit run language is downgraded to create (no silent
  auto-run). Any brain failure (no key, disabled, timeout, error envelope,
  unparseable reply) falls back to the deterministic classifier, so the brain is
  strictly additive. The CLI path lifts the human text out of the `--output-format
  json` envelope **before** validating, so the raw envelope never reaches the UI.
  The chat card shows a small **🧠 brain-classified** chip only when the brain
  genuinely decided the intent. Built reference-first per the new
  `docs/reference-driven-development.md` rule (read: Hermes' allowlist-validated
  tool loop, Paperclip/openclaw's fail-closed mutation gate and balanced-JSON
  output parsing). (`docs/RELUX_MASTER_PLAN.md` §10.1 Intent Layer, §17.1.)
- **Relux local release v0.1.7 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.6` to `0.1.7`, bundling the post-v0.1.6
  product work into a fresh Windows release. Headlines: a **first-class idea →
  plan → tasks rung** — Prime renders an action-free **plan-preview proposal card**
  (goal, *N steps across M agents*, per-step role + assignee) that commits nothing
  until an explicit one-click **Create these tasks** / **Turn this into a task**,
  with an optional **advisory LLM polish** of the *wording only* (summary, step
  titles, clarifying questions, risk notes) that works through the same
  `validate_polish` chokepoint on both the OpenRouter and the local Claude/Codex
  CLI brains and shows *which* brain refined it on the card; a **conversation
  guard** so questions and musing stay chat and never silently mint work; and a
  **route-level `ErrorBoundary`** so a single page crash degrades to an in-app
  error card with Reload/Retry instead of blanking the whole SPA. Also folds in the
  blank-Crew-page fix and the reflect-and-clarify follow-ups (Brainstorming /
  Orchestration single-step / TaskUpdate) recorded in the entries below. Build the
  bundle with `scripts\relux-package-local.ps1 -FullE2E`. This version line is the
  `relux-kernel` crate version (separate from the legacy Relix workspace versions
  in the dated sections below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **A page crash no longer blanks the whole dashboard — route-level `ErrorBoundary`.**
  Implements `relix-dashboard-design.md` §2 and `RELUX_MASTER_PLAN.md` §17.6. Every
  routed page now mounts inside a React `ErrorBoundary`: a render-time throw in one
  view (e.g. a hook misuse or an unexpected shape) is caught and shown as an in-app
  error card with **Reload** / **Retry** affordances and a copyable detail, instead
  of an unrecoverable white screen that takes down the rest of the SPA with it. A
  pure `errorBoundaryMessage` helper normalizes the displayed text (Error / string /
  unknown throw) and is pinned by `apps/dashboard/test/error-boundary.test.ts`; a new
  `apps/dashboard/test/work-render.test.mjs` SSR-renders **Work** under the plain
  declarative `<BrowserRouter>` the app actually uses, so the page is proven to mount
  without a data-router-only hook throw. Dashboard bundle rebuilt into
  `crates/relix-web-bridge/dashboard-dist`.
- **Conversation guard — questions and musing stay chat, never mint work.**
  Implements `RELUX_MASTER_PLAN.md` §10.5 (Conversation Rules) and §17.1 (smart &
  grounded), grounded in `docs/prime-processing-audit.md`. `classify_intent` now
  treats interrogatives and musing lead-ins as a **conversation** even when the
  sentence happens to contain an action-shaped verb — so *"should we create a task
  for this?"* or *"I was thinking we could orchestrate the agents"* gets a real
  answer instead of silently minting a task or kicking off a run. An **explicit
  command** (`create a task to…`, `orchestrate`, `assign`, `start it`) still
  overrides and mints/runs work; the deterministic classifier remains the sole owner
  of the action decision and the action-free wall is intact. New `relux-kernel`
  regression tests pin the question/musing phrasings against the explicit-command
  override.
- **The plan-preview card now shows *which* brain refined the wording, visibly.**
  Implements `RELUX_MASTER_PLAN.md` §10 (planning layer), §11.1, §17.1, and closes the
  documented "surface the CLI brain's provenance on the card the way the OpenRouter
  model id already is" follow-up (`docs/prime-processing-audit.md`). When an advisory
  polish overlay is present, the proposal card's **"AI-refined wording"** badge now
  reads **"AI-refined wording · `<source>`"** with the source shown inline (no longer
  hover-only): the **OpenRouter model id** (e.g. `anthropic/claude-3.5-haiku`) on the
  HTTP path, or the **CLI brain label** (`Claude CLI` / `Codex CLI`) on the local
  adapter path — both come from the same `polish.model` field stamped by the one
  `validate_polish` chokepoint, so a CLI polish reads as cleanly as an OpenRouter one.
  No wire change: the field already carried this for both brains; this is the dashboard
  catching up. A new pure `polishProvenance` helper centralizes the display (model id /
  CLI label / generic "AI brain" when an older kernel left `model` unstamped / `null`
  when there is no overlay), pinned by new `apps/dashboard/test/prime.test.ts` cases.
  The authoritative steps/order/agents and the commit button are untouched. Dashboard
  bundle rebuilt into `crates/relix-web-bridge/dashboard-dist`.
- **Advisory plan-preview polish now works with the CLI brains too (Claude / Codex),
  through the same validation chokepoint.** Implements `RELUX_MASTER_PLAN.md` §10
  (planning layer), §11.1, §17.1. The OpenRouter brain could already refine only the
  *wording* of a `PlanRequest` proposal card (summary, per-step titles, clarifying
  questions, risk notes) while the deterministic planner stayed the sole authority on
  step count/order/agent grounding/`goal`/commit. That advisory polish now extends to
  the local Claude/Codex CLI brains: `compose_polish_prompt` hands the adapter a
  strict-JSON polish instruction plus the authoritative steps on stdin (mirroring
  `compose_chat_prompt`), the kernel spawns it in the same bounded, non-bypass mode as
  the conversational path (`polish_proposal_via_cli`), lifts the reply out of the
  result envelope with `parse_adapter_result` (the same shape seam), and runs it
  through `polish_from_cli_text` → **the same `validate_polish`** the OpenRouter path
  uses. So a CLI brain can only ever change titles/questions/risks/provenance — never
  the step count, order, or agent ids — and an error envelope, prose with no JSON, a
  timeout, a missing/disabled adapter, or any suggestion that fails validation simply
  leaves the deterministic card in place with **no user-facing failure**. Polish is
  gated on a **non-actionful** turn (only a `PlanRequest` carries a proposal; the
  "Create these tasks" commit is a separate `Orchestration` turn), so the commit path
  never invokes it. New `ai.rs` tests pin the prompt contract and the
  `polish_from_cli_text` chokepoint (valid JSON accepted, prose-wrapped JSON tolerated,
  malformed/objectless ignored, added/dropped/reordered steps rejected), and new
  `server.rs` `cli_polish_*` tests pin the envelope seam (result-envelope and plain
  JSON accepted, prose / error envelope ignored, structural drift rejected, no-adapter
  → unpolished). No test calls a paid provider. Dashboard unchanged (the card already
  renders `polish` when present).
- **Prime plan previews render as a proposal card, not just prose.** Implements
  `RELUX_MASTER_PLAN.md` §11.1 (Prime Chat shows *"plugin/action results"* and
  *"suggested next actions"*) and §10 (planning layer). A `PlanRequest` turn now
  carries a STRUCTURED, action-free `proposal` on the wire (`PrimeProposal` /
  `PrimeProposalStep` in `relux-core`): the goal, whether it is a genuine
  `multi_step` plan, the ordered steps (1-based index, title, the specialist role,
  and the agent each would land on — `"prime"` when no specialist fits), and the
  distinct agents. The dashboard Prime page renders it as a compact B&W **plan
  preview** card — goal heading, an *N steps across M agents* summary, and the
  proposed steps with their role + assignee. **Nothing runs from showing it:** the
  card commits nothing; the explicit **Create these tasks** (multi-step) /
  **Turn this into a task** (single-step) button — still a pre-written `send:false`
  suggestion — is the lone commit path, keyed off the SAME decomposition the card
  shows. The proposal carries no action and is omitted on every non-plan turn, so
  existing clients see the same JSON. New core/kernel tests pin the wire shape
  (present for a plan, absent for normal chat/task-creation, descriptive-only) and
  `apps/dashboard/test/prime.test.ts` pins the card helpers; dashboard bundle
  rebuilt into `crates/relix-web-bridge/dashboard-dist`.
- **Reflect-and-clarify for the Orchestration single-step and TaskUpdate arms.**
  Implements `RELUX_MASTER_PLAN.md` §10.5 ("ask clarifying questions when needed"),
  per `docs/prime-processing-audit.md` ("Next recommended slice"). Both arms used to
  emit one fixed prompt that ignored what the user already said; they now reflect the
  parsed target/goal back (mirroring `brainstorm_reply`): the Orchestration
  single-step Clarify quotes the stripped `orchestration_goal` and asks for the
  distinct steps, and TaskUpdate reflects the parsed task id and/or field
  (priority/title/assignee/status) and asks only for the missing piece. Both stay
  `PrimePlan::Clarify` — no `UpdateTask` action is invented and the action-free wall
  holds — pinned by `orchestration_clarify_reflects_the_parsed_goal` and
  `task_update_clarify_reflects_target_and_field`.
- **Fix the blank Crew page + content-aware Prime brainstorm follow-up.**
  Implements `relix-dashboard-design.md` §9 / App routing and `RELUX_MASTER_PLAN.md`
  §10.5. **Crew:** the page called react-router's `useLoaderData()`, but the SPA
  mounts under a plain `<BrowserRouter>` (a declarative router, not a data router),
  so the hook threw and white-screened `/crew`. Crew now loads its own data through
  the same `useAsync` hook every other Relux page uses and renders honest
  loading / error (with Retry) / empty / list states plus the adapter runtime
  controls — never a blank page — and the rail's **Crew** entry now points at `/crew`
  instead of the legacy `/agents` console. **Prime:** the Brainstorming arm reflects
  the recovered topic (lead-ins stripped, quoted) and asks one concrete follow-up,
  falling back to the open-ended prompt when nothing is nameable; it stays a Reply,
  nothing is created or run. Pinned by crew-render SSR + shipped-bundle guards and
  `brainstorm_reply` reflection/clarify tests; dashboard bundle rebuilt.
- **Prime suggested next actions — one-click buttons replace "type this" copy.**
  Implements `RELUX_MASTER_PLAN.md` §11.1 (Prime Chat shows *"Prime suggested next
  actions"*), with §10.5 (Conversation Rules) and §17.1 (smart & grounded). Each
  Prime turn can now carry `suggested_actions` — a list of `{label, message, send}`
  buttons the chat surface renders under the reply. **(1) Task creation** offers a
  **Start the run** button (sends `start it`) instead of the old awkward copy
  *Say "start it" when you want me to run it* — that sentence is gone. **(2)
  Brainstorming is useful, not a dead end:** the reply engages the idea and offers
  a **Turn this into a task** button that *pre-fills* `create a task to <the work>`
  (recovered from the message by stripping ideation lead-ins) for the user to
  confirm or edit — `send: false`, so nothing is created until they hit Send. A
  suggestion is never a privileged path: acting on one routes a pre-written user
  message through the same grounded `prime_turn`, so a button can do nothing the
  user could not type. New unit tests pin the candidate extraction and the §11.1
  attach semantics; the empty list is omitted on the wire so existing clients are
  unchanged. Dashboard bundle rebuilt into `crates/relix-web-bridge/dashboard-dist`.
- **Relux local release v0.1.6 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.5` to `0.1.6` for a user-facing patch that
  keeps **Prime conversational on ideation** and records the post-v0.1.5 session
  work that had not yet shipped in a bundle. The headline fix (detailed in the
  entry below): **brainstorming no longer auto-creates tasks** — musing lead-ins
  ("I was thinking…", "what if we…", "I have an idea…") classify as a conversation
  even when the sentence carries a creation verb, while an **explicit command**
  (`create a task to…`, `orchestrate`, `assign`, `start it`) still mints/runs work;
  **Prime task/run links deep-link into Work** via `/work?task=<id>` (and
  `/work?run=<id>`), opening that item focused; and the **Prime page is chat-first**
  (Autonomy + Orchestration moved into a collapsed *Advanced* disclosure below the
  input). This release also bundles the post-v0.1.5 operator-session work recorded
  in the entries below — **restart-persistent sessions** (auth v1.2), **live
  session-file reconcile** so `reset-admin` revokes without a `serve` restart (auth
  v1.3), and the **absolute-session-cap decision** ruled intentional (auth v1.4):
  the hard ceiling is wall-clock from session mint and only a fresh re-auth
  re-anchors it — activity never extends it. Build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. This version line is the
  `relux-kernel` crate version (separate from the legacy Relix workspace versions
  in the dated sections below). See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Prime stays conversational on ideation, deep-links tasks, chat-first page.**
  Three user-facing product fixes to Prime (`RELUX_MASTER_PLAN.md` §10.5
  Conversation Rules, §11.1 Prime Chat). **(1) Ideation no longer mints tasks.**
  `classify_intent` now treats musing lead-ins ("I was thinking…", "what if we…",
  "I have an idea…") as **Brainstorming** even when the sentence carries a creation
  verb — so *"I was thinking to create an n8n-like program using 20 agents"* stays a
  conversation instead of silently creating a task. An **explicit command**
  (`create a task to…`, `orchestrate`, `assign`, `start it`) still overrides and
  mints/runs work. **(2) Task/run links deep-link into Work.** The link after task
  creation pointed at a bare `/work` and opened with nothing focused; it now uses
  `/work?task=<id>` (and `/work?run=<id>` for runs). Work reads `?task=` URL-driven
  and **mutually exclusive** with `?run=`, opening that task's detail panel and
  degrading honestly when the id is missing. **(3) Prime page is chat-first.** The
  Autonomy and Orchestration panels moved out of the top of the page into a
  collapsed **Advanced** disclosure below the input, so the chat is the primary
  surface, with an honest hint that brainstorming stays a conversation and only
  explicit commands create/run work. Regression tests pin the exact musing
  sentence, the explicit-command override, that "start it" still acts on a ready
  task, and the `?task=`/`?run=` routing helpers (mirroring the existing run
  routing). `cargo test -p relux-kernel` green; dashboard routing tests green; the
  built `dashboard-dist` bundle is refreshed. See `docs/RELUX_MASTER_PLAN.md` →
  *Conversation Rules* / *Prime Chat*.
- **The absolute session cap is intentional, not a sliding bug (auth v1.4).** The
  hard **absolute** session ceiling (`SESSION_ABSOLUTE_MAX_SECS`) is **wall-clock
  from the moment a session is minted** and is **never** extended by activity — only
  a fresh re-auth (logout + new login) re-anchors a new window. This was reframed
  from a "caveat" into an explicit, tested decision: the `auth.rs` doc comment now
  states it so the constant is not mistaken for a sliding cap, and a new lib test
  (`a_fresh_login_re_anchors_the_absolute_window_but_activity_never_does`) pins both
  halves at the kernel-unit level. No behavior change. `cargo test -p relux-kernel`
  green (lib + bin); clippy clean. See `docs/RELUX_MASTER_PLAN.md` → *Local operator
  login v1* and the divergence ledger in `docs/product-spine-implementation.md`.
- **Live session-file reconcile — `reset-admin` no longer needs a `serve` restart
  to revoke sessions (auth v1.3).** A **running** `relux-kernel serve` now picks up an
  out-of-band change to the persisted session file without a restart. Before every
  session operation the store cheaply re-`stat`s its backing file (a fingerprint of
  mtime + length, plus a "file absent" state) and only when that differs from what it
  last wrote does it reconcile its in-memory table with disk: a **deleted** file (what
  `reset-admin` does) drops all in-memory sessions — fail-closed — and an external
  **rewrite** is reloaded so the running process adopts it instead of overwriting it on
  its next persist. The fast path (the process is the only writer) is a single `stat`,
  no per-request read/parse. `create`/`refresh` reconcile *before* they persist, so a
  fresh login right after a delete cannot rewrite the just-revoked sessions back to
  disk. **Net effect:** `relux-kernel reset-admin` now invalidates old cookies on a
  running server on the **next request** — the previous "restart `serve` to finish
  revocation" step is gone (a restart is only still needed to load a new credential
  into a *stopped* process, or as a fallback for a wedged one). Sliding-refresh,
  logout, password-change invalidation, restart-persistence, and the
  `RELUX_AUTH_DISABLED` dev bypass are all unchanged. Proven by `relux-kernel` unit
  tests (external delete revokes a live session on the same handle with no restart;
  delete + new login does not resurrect old sessions and persists only the new one; an
  external rewrite is adopted; an unchanged file is never reloaded so own writes are
  not lost) and an in-process HTTP test (one running server: login → protected route
  200 → delete the session file out of band → next request 401 → fresh login still
  works). `cargo test -p relux-kernel` green; clippy clean. *Caveat:* detection is
  per-operation `stat` granularity (revocation bites on the next session-touching
  request, not instantly); a same-mtime-and-same-length external *rewrite* could be
  missed, but *deletion* — the recovery case — flips present→absent and is always
  detected. See `docs/RELUX_MASTER_PLAN.md` → *Local operator login v1*.
- **Relux local release v0.1.5 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.4` to `0.1.5` for the first build that puts a
  **single-admin local operator login** in front of the standalone dashboard/API.
  This release bundles the post-v0.1.4 auth work (detailed in the entries below):
  **First-run admin setup + login** — on first launch the dashboard shows a one-time
  setup screen to set the local admin password; thereafter a sign-in screen gates
  the dashboard and the `/v1/relux/*` API, with the session carried in an HTTP-only
  `relux_session` cookie. The credential lives next to the DB (Argon2-hashed, never
  the plaintext); `relux-kernel reset-admin [user] [pw]` is the password-recovery
  path when the current one is unknown, and `RELUX_AUTH_DISABLED=1` is a documented
  dev/test bypass that `serve` warns about loudly. **Password change in-console** —
  the dashboard **Account** panel changes the admin password (verifies the current
  one, enforces the length floor) without touching the session. **Sliding session
  refresh** — an active session slides forward on each authenticated request up to a
  hard **absolute** ceiling; idle past the rolling window signs out. The public,
  **non-sliding** `GET /v1/auth/me` exposes safe, secret-free session metadata
  (idle/absolute deadlines + seconds remaining, the policy windows, server clock —
  never the session id, cookie, or hash). **Account session readout + expiry
  warning + one-click re-auth** — the Account panel shows the idle/absolute policy
  with live countdowns; the shell topbar shows a quiet expiry chip (amber for idle,
  red for the absolute ceiling) that opens Account; and Account offers a *"Sign out
  and sign back in"* re-auth action — promoted to the primary action inside the
  absolute warning window — that ends the session via `POST /v1/auth/logout` and
  re-shows sign-in (never auto-submits credentials, never weakens auth). Proven by
  `relux-kernel` unit + in-process HTTP tests (setup/login/logout, sliding refresh,
  old-cookie server-side invalidation on re-auth, the `/v1/auth/me` no-secret
  contract), dashboard decision-helper tests (`sessionWarning` / `reauthCallout` /
  the local countdown basis), render/static proofs of the chip + Account promotion,
  and the standalone `scripts\relux-e2e-smoke.ps1` full E2E over HTTP against the
  real release binary. *Known caveats:* one admin only (no multi-user, roles, or
  per-operator audit); sessions are **in-memory** and do not survive a `serve`
  restart (everyone re-signs-in); the loopback API has **no transport TLS**; the
  absolute ceiling can only be cleared by a fresh sign-in (no console action extends
  it); and `RELUX_AUTH_DISABLED` leaves the surface fully open by design. This
  version line is the `relux-kernel` crate version (separate from the Relix
  workspace version below); build the bundle with
  `scripts\relux-package-local.ps1 -FullE2E`. See `docs/RELUX_MASTER_PLAN.md` →
  *Release history*.
- **End-to-end proof of the re-auth / session-reset flow.** Two complementary
  checks pin the Account *"Sign out and sign back in"* path — the only way to clear
  the hard **absolute** 7-day ceiling. A deterministic in-process HTTP test
  (`reauth_logout_then_login_resets_the_absolute_window_and_kills_the_old_session`)
  drives setup → `/v1/auth/me` → logout → re-login and asserts the old session is
  invalidated server-side (both `/v1/relux/*` and `/v1/auth/me` reject the old
  cookie) while the new session mints a **distinct** id and a **reset** absolute
  window (ceiling pushed forward, ~the full cap again). The standalone e2e smoke
  (`scripts/relux-e2e-smoke.ps1`) mirrors the same flow against the real release
  binary over HTTP, replaying the raw old cookie on a no-jar client to prove true
  server-side invalidation rather than a merely-cleared browser jar. No production
  constant is mutated for the proof; the reset is shown **relative to the first
  session** plus old-cookie invalidation. Backend untouched. See
  `docs/RELUX_MASTER_PLAN.md` → *Local operator login v1*.
- **One-click re-authentication from the Account panel.** The **Account** control
  now offers a *"Sign out and sign back in"* button — the one reliable way to clear
  the hard **absolute** 7-day ceiling, which no in-console action can extend. It
  ends the current session via the existing `POST /v1/auth/logout` so the normal
  sign-in screen reappears; the operator then logs in themselves, minting a fresh
  session that resets the cap. It **never** auto-submits credentials and never
  weakens auth. The button is always present, and is **emphasised** — promoted to
  the primary action with an alert banner — exactly when the absolute ceiling is
  inside its warning window (the same ≤30 min the red expiry chip uses, so the chip
  → Account → re-auth path is coherent); otherwise it stays a quiet secondary
  control. Signing out this way leaves other sessions untouched, and the
  password-change form is unchanged (a failed sign-out keeps the session intact and
  surfaces why, with the topbar **Sign out** control as the fallback). Tests pin the
  decision helper (`reauthCallout` — fires only on the absolute window, ignores
  idle, honours elapsed time, silent under the dev bypass / older kernel). Backend
  untouched. See `docs/RELUX_MASTER_PLAN.md` → *Local operator login v1*.
- **Passive session-expiry warning in the Relux shell.** The dashboard topbar now
  shows a quiet chip when the signed-in session is close to ending — amber for the
  rolling **idle** window (*"Signs out for inactivity in 8m"*, ≤10 min left) and
  red for the hard **absolute** 7-day ceiling (*"Re-sign-in required in 25m"*, ≤30
  min left). Clicking it opens the **Account** panel, where the full readout and
  the `reset-admin` recovery note live; the absolute case makes clear a fresh
  sign-in is the only fix (it never slides). It reads the SAME safe, non-sliding
  `GET /v1/auth/me` metadata the Account control uses — once on shell mount, then
  re-anchored only on sparse, event-driven moments (the tab regaining visibility,
  the Account panel closing), never a busy poll (and pointless to poll anyway:
  `/v1/auth/me` does not slide the session). A single un-noisy **per-minute** timer
  counts down locally between fetches. When both windows are close the more urgent
  one shows (a tie favours absolute); the chip stays hidden under the
  `RELUX_AUTH_DISABLED` dev bypass and for an older kernel that omits the deadlines.
  Tests pin the warning decision helper (`sessionWarning` — thresholds, which
  window wins, elapsed-time handling, the silent cases). Backend untouched. See
  `docs/RELUX_MASTER_PLAN.md` → *Local operator login v1*.
- **Session expiry / idle visibility in the dashboard Account control.** The
  Account modal now shows the signed-in operator their session policy at a glance:
  *"Signs out after 12h of inactivity"* and *"Re-sign-in required after 7d"*, each
  with a live *"… left"* countdown. `GET /v1/auth/me` is extended to return safe,
  secret-free session metadata alongside the username — the idle and absolute
  deadlines (`idle_expires_at` / `absolute_expires_at`, unix seconds), the seconds
  remaining on each (`idle_expires_in_secs` / `absolute_expires_in_secs`, clamped
  ≥0), the configured policy windows (`idle_timeout_secs` / `absolute_max_secs`),
  and the server clock (`server_now`). It **never** exposes the session id, the
  cookie value, or the admin hash (a test asserts the body contains neither).
  **Pre- vs post-refresh (documented):** `/v1/auth/me` is public — it sits outside
  the sliding `require_session` middleware and reads via a **non-mutating**
  `session_meta`, so polling it does **not** slide the idle window; the deadlines
  returned are the **current, pre-refresh** values (a real protected `/v1/relux/*`
  request still slides the session). This means the Account modal can poll the
  readout without keeping an otherwise-idle console alive. The dashboard counts
  down locally from the kernel-computed remaining seconds (skew-free) with a single
  un-noisy **per-minute** timer, started only when there is a window to show (never
  under the `RELUX_AUTH_DISABLED` dev bypass, which surfaces an honest *"Session
  expiry is disabled"* note instead). An older kernel that returns only
  `{ username }` simply hides the readout — the password-change form is unchanged.
  Tests pin the kernel metadata + non-sliding read semantics and the frontend
  formatting helpers (`formatDuration`, `idle/absoluteRemaining`, the policy
  descriptions). See `docs/RELUX_MASTER_PLAN.md` → *Local operator login v1*.
- **In-product password change for the local operator login.** The signed-in
  operator can now change the local admin password from the dashboard — no CLI for
  the normal case. The Relux shell's signed-in name is an **Account** control that
  opens a change-password dialog (current password + new password + confirm, with
  friendly validation and an explicit success state); **Forgot password** still
  points at the local `reset-admin` CLI. New protected endpoint
  `POST /v1/auth/change-password` (`{ "current_password", "new_password" }`,
  behind the session guard): it verifies the current password against the stored
  Argon2id hash, enforces the same 8-char minimum as setup, and writes a fresh
  Argon2id hash with the **same atomic write** as first-run setup/reset — the
  plaintext and the hash are **never logged or returned**. **Session policy on
  change (documented):** the caller's own session is preserved while **every other
  live session is invalidated**, so changing the password boots any other
  browser/device but does not sign the operator out of the tab they just used.
  Recovery when the current password is unknown remains the local `reset-admin`
  CLI. Tests pin the semantics: wrong current password is refused and changes
  nothing, a too-short new password is refused, a success swaps the on-disk hash
  (old password stops working, new one works after a simulated restart), other
  sessions are invalidated while the current survives, and the endpoint requires a
  session (the dev/test `RELUX_AUTH_DISABLED` bypass refuses the change rather than
  rewrite an ignored credential). A new dashboard unit test pins the form's
  client-side validation, and `relux-e2e-smoke.ps1` drives the live flow end to end
  (two cookie jars prove the invalidation; old-fails/new-works login round-trip).
  See `docs/RELUX_MASTER_PLAN.md` → *Local operator login v1*.
- **Local operator login for the standalone Relux dashboard/API (auth v1).** The
  `relux-kernel serve` surface is no longer unauthenticated: it now requires a
  simple local username/password sign-in, replacing the dashboard token weirdness
  with a browser session cookie. On first launch the dashboard shows a one-time
  **setup** form that creates a single local admin; the password is stored only as
  an **Argon2id** PHC hash at `dev-data/relux/dashboard-admin.json` (next to the
  DB, gitignored, OS-restricted, never plaintext, never returned by the API). A
  successful setup/login mints an **HTTP-only** `relux_session` cookie
  (`SameSite=Lax`, `Path=/`, 12-hour expiry; **no** `Secure` because the console
  runs over loopback `http://` — documented honestly, a TLS-terminating proxy can
  re-add it). New public endpoints `GET /v1/auth/status`, `POST /v1/auth/setup`,
  `POST /v1/auth/login`, `POST /v1/auth/logout`, `GET /v1/auth/me`; a middleware
  protects every other `/v1/relux/*` route behind a valid session, while the
  static dashboard (so the login screen always renders) and `/v1/relux/health`
  (liveness) stay public. The dashboard gates the whole shell on the session and
  adds a **Sign out** control showing the signed-in operator. Sessions are
  in-memory (single-process): a `serve` restart drops them and re-prompts login
  while the admin credential stays durable. **Password recovery** is the local
  `relux-kernel reset-admin [username] [password]` CLI (filesystem-only, no
  network/unauthenticated reset; generates + prints a strong password once when
  omitted; restart `serve` to drop live sessions). The CLI (`prime`,
  `task run-assigned`, `tools`, autonomy, …) talks to the durable store directly
  and is unaffected. A dev/test-only escape hatch `RELUX_AUTH_DISABLED` leaves the
  API open (OFF by default; flagged loudly by `serve` and `doctor`). Tests pin the
  doc semantics: password is stored as Argon2id (never plaintext), setup is
  first-run only, login rejects a wrong password, a protected route is 401 without
  a session and 200 with one, logout re-locks, health stays public, and the bypass
  opens the API only when explicitly set. The `relux-e2e-smoke.ps1` serve checks
  now log in (cookie jar) and assert the no-session negative control. This
  **partially closes** the long-standing "standalone API is loopback-only and
  unauthenticated by design" caveat — the loopback bind stays, but the surface is
  now gated by a single-admin local login (still a single-operator local console,
  not a multi-tenant/internet production surface). See `docs/RELUX_MASTER_PLAN.md`
  → *Local operator login v1*.
- **Relux local release v0.1.4 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.3` to `0.1.4` for the first build on top of
  v0.1.3 that makes the orchestrator's **run results reviewable and applyable** and
  its **live progress honest**, while fixing a user-facing Prime-chat regression.
  This release bundles the post-v0.1.3 work (detailed in the entries below):
  **Prime CLI brain raw-JSON fix** — the Claude/Codex conversational path showed
  the whole result envelope instead of the human answer; the reply is now shaped
  through the same `parse_adapter_result` the assigned-run path uses
  (`shape_cli_brain_reply`, never the raw JSON), and the Prime conversational brain
  handles the `proposed_changes` envelope honestly. **First real Relux diff/apply
  model** — a run captures read-only **artifacts** promoted into reviewed, applyable
  **proposed changes** that **replace / create / rename-move / delete** files,
  applied as a **single multi-file transactional apply** (all-or-nothing: a per-
  change precondition/traversal failure rolls the whole batch back). **Live-tail +
  stalled signals** — both the Relux **Work** Run Detail and the legacy **Run
  transcript** do an efficient **incremental live-tail** and show an honest
  **stalled / "No activity for Xs"** badge-chip when an in-flight run goes quiet,
  with consistent wording across both surfaces. **Orchestration cancel / resume /
  restart-honest** — cooperative cancel/stop for live multi-brief jobs,
  resume-after-cancel, and restart-honest status reconstructed from the durable
  record with an interrupted-job callout + **Continue** resume. **Run Detail deep
  links + UX polish** — URL-driven in-shell Run Detail with orchestration `run_id`
  deep links, a **Copy link** action, consolidated in-shell run navigation, honest
  review/apply parity, per-brief recorded run duration, and a **status badge that
  carries the error tone** for failed runs. Also an actionable **port-conflict**
  message on `serve` bind failure plus a matching bundle-launcher preflight with
  pinned wording parity. Every v0.1.3 safety property holds on every path:
  dependency gating, at-most-once per round, permission + adapter-runtime gating
  before any spawn, secret redaction, the durable run transcript, audit, retry,
  sibling failure/panic isolation, and **no auto-run of downloaded plugin code**.
  Proven against the real Claude and Codex CLIs and by deterministic unit/HTTP
  smokes. *Known caveats:* the transactional apply is the **Relux kernel**
  proposed-change surface (separate from the legacy `relix-runtime` brief-runs
  apply); the in-memory job registry still does not survive a restart for
  **by-job-id** polls (the by-orchestration-id poll stays restart-honest);
  live-tail is incremental polling, not a server-push event stream; retry/resume is
  a fresh attempt or a continued batch, not a partial-CLI-run resume; and the
  standalone API remains loopback-only and unauthenticated by design. This version
  line is the `relux-kernel` crate version (separate from the Relix workspace
  version below); build the bundle with `scripts\relux-package-local.ps1 -FullE2E`.
  See `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Failed-run status badge carries the error tone (relix-dashboard-design
  §12).** `runStatusTone` mapped a `failed` run to the neutral `backlog` chip —
  the same muted-grey tone used for unknown / pre-terminal statuses — so a failed
  run's **status badge** (on the Run Detail header and the Work run list) read as
  unemphasized as a `completed` one, even though the **failure-reason** text right
  below it renders in red (`var(--err)`). The design system's status vocabulary
  explicitly includes an **error** tone ("blocked, live/running, done/healthy,
  error" as restrained badges) and mandates "No silent failures: every failed run
  is visible." `failed` now maps to the existing shared error-red chip
  (`.badge.blocked` — the same muted `var(--err)` chip the transcript uses for
  "permission denied"), so the status badge reads consistently with the red
  failure reason and is never rendered in the neutral tone. `cancelled` is a
  terminal **non-error** and stays neutral; `completed` / `running` / pre-terminal
  tones are unchanged. Pure tone-mapping reuse of an existing chip (no new CSS, no
  behavior change); the `runStatusTone` semantics are pinned by a unit test and
  the dashboard bundle was rebuilt.
- **Compact-resilient Relux Work Run Detail header (relix-dashboard-design §8 /
  §11).** The `RunDetailPanel` header on the **Work** page laid its title +
  `status` badge + `live · No activity for Xs` cue and its **Copy link / Retry /
  Close** controls on a single non-wrapping `.row` with a `flex:1` spacer. That
  panel is a full-width `.card` (not a `.context-panel`, so the
  `.context-panel .row { flex-wrap: wrap }` rule never applied), so in a narrow
  card a long stalled cue squeezed the three action buttons — shrinking them and
  wrapping their labels (`Copy`/`link`). The header now reuses the same
  `.xtr-bar` / `.xtr-bar-meta` / `.xtr-bar-actions` split the legacy
  `<RunTranscript>` header adopted: the meta group (title, status, live/stalled
  cue) takes the flexible track and wraps **within itself**, while the action
  buttons stay together as one `flex:0 0 auto` unit and wrap as a block under the
  meta when the card is narrow — no squeeze, no label-wrap, and **no state is
  hidden**. Pure-markup/CSS reuse (no new CSS, no behavior change beyond layout);
  the existing transcript helpers stay unit-tested and the dashboard bundle was
  rebuilt.
- **Honest stalled / no-activity signal on the legacy Run transcript too
  (relix-dashboard-design §8 / §11).** The Relux Work Run Detail already showed a
  `No activity for Xs` cue when an in-flight run went quiet; the **legacy bridge**
  `<RunTranscript>` (Runs page + the Brief workroom embed) had the efficient
  `?since=` live-tail but no equivalent stalled hint. It now shows the same honest
  cue: while a Shift is `running`, a once-a-second wall clock ages the transcript
  bar so the last-event "ago" advances smoothly between the 2.5 s tail polls, and
  when no new transcript event has landed for **≥ 10 s** the bar shows a
  **`◌ No activity for Xs`** badge. The legacy `run_events.ts` is a real
  wall-clock unix time, so staleness is measured **directly against the live
  clock** off the last event (unlike the Relux surface, whose `ts` is a logical
  clock and is therefore aged against a client wall clock). **No fabricated
  progress bar** — it only reports elapsed silence, and the badge title is honest
  that it's *observed* silence, not a guaranteed stall. The threshold + the
  `No activity for Xs` copy are now a **single shared pure helper**
  (`noActivityLabel` + `RUN_STALL_SECS` in `apps/dashboard/src/runstall.ts`),
  re-exported by both `runtranscript.ts` (legacy) and `reluxruntranscript.ts`
  (Relux) so the two surfaces read identically without coupling their (different)
  event models. Unit-tested on the legacy surface (shared-helper identity, the
  threshold, and the wall-clock-`ts`-driven label); the dashboard bundle was
  rebuilt.
- **Incremental live-tail + honest stalled signal for the Relux Work Run Detail
  (relix-dashboard-design §8 / §11).** The previous transcript live-tail slice
  improved only the **legacy bridge** surface (`/v1/runs/:id/events`, the
  `<RunTranscript>` renderer on the Runs page); the **Relux local-shell** Run
  model (`/v1/relux/runs/:id`, the `RunDetailPanel` on the **Work** page) was
  untouched and still re-fetched the **whole** transcript every 1.5 s while a run
  was in flight. This slice brings the same efficiency to the Relux run model and
  adds a stalled-run indicator. (1) **API** — `GET /v1/relux/runs/:id/events`
  gains an optional **`?since=<event_id>`** exclusive cursor that returns only the
  events strictly after that id (oldest-first); **absent/empty `since` returns the
  full transcript, exactly as before** (`GET /v1/relux/runs/:id` is unchanged). A
  new `KernelState::run_events_since` backs it; `run_events` now delegates with no
  cursor. The cursor compares on the **numeric** suffix of the `revent_NNNN` event
  id (correct even past the 4-digit zero-pad width), and a `None`/empty/unparseable
  cursor degrades to the full transcript so a malformed `since` never hides
  history. (2) **UI** — the Work `RunDetailPanel` now keeps the accumulated events
  and, while the run is in flight, re-fetches **only the tail past its cursor**
  (1.5 s cadence) and merges the new events on (deduped by id); the small run
  record is still re-fetched whole. (3) **Honest stalled signal** — when an
  in-flight run goes quiet (no new transcript event and no run phase/status change
  for ≥ 10 s), the header + transcript show **`No activity for Xs`** (real
  wall-clock elapsed, since the Relux event `ts` is a logical clock) instead of
  the normal `live · refreshing…` cue. **No fabricated progress bar** — it only
  reports elapsed silence. New pure helpers
  (`latestReluxEventId`/`mergeReluxRunEvents`/`reluxEventSeq`/`noActivityLabel`)
  live in `apps/dashboard/src/reluxruntranscript.ts` and are unit-tested; a kernel
  store test pins the exclusive-cursor semantics (full → tail → caught-up,
  None/empty/unparseable → full).
- **Efficient incremental live-tail for the Run transcript (relix-dashboard-design
  §8 / §11).** The Run Detail transcript now stays current during a long
  Claude/Codex Shift **without a manual refresh** — and does it cheaply. (1)
  **API** — `GET /v1/runs/:id/events` gains an optional **`?since=<event_id>`**
  exclusive cursor that returns only the events newer than that id (the new tail),
  oldest-first; **without `since` (or `since=0`) it returns the full transcript,
  exactly as before** (`GET /v1/runs/:id` is unchanged). A new
  `TaskStore::list_run_events_since` backs it over the existing
  `run_events(run_id, event_id)` index; `list_run_events` now delegates to it with
  cursor `0`. The bridge forwards `run_id` (full) or `run_id|since` (tail) to the
  peer `run.events` capability; a non-positive cursor degrades to the full
  transcript so a malformed `since` never hides history. (2) **Why polling, not a
  new SSE** — the existing per-tenant execution-event SSE firehose
  (`/v1/runs/events/stream`) carries only **coarse lifecycle transitions** (run
  start/finish, board move, review, apply) from `task_events`, **not** the per-run
  transcript lines (`tool_use`/`assistant_message`/`command`/`result`) that live in
  the separate `run_events` table — so it cannot drive a mid-run transcript without
  overloading tenant-wide stream semantics. Instead `<RunTranscript>` polls the
  per-run tail on a **steady 2.5 s cadence while the run is `running`**, fetching
  only `?since=cursor` and **merging** the new events on (deduped by `event_id`),
  and keeps the lifecycle SSE subscription as an **immediate nudge** (so the
  terminal result lands promptly). (3) **Honest in-flight summary** — the
  transcript bar now shows **real event count · current phase · last-event clock**
  while running (e.g. `12 events · tool call · 3s ago`) — derived only from
  recorded events, **no fabricated progress bar**. New pure helpers
  (`mergeRunEvents`/`latestEventId`/`runTranscriptProgress`/`lastEventAgo`/`kindLabel`)
  live in `apps/dashboard/src/runtranscript.ts` and are unit-tested; an end-to-end
  mini-mesh test proves the `?since=` tail (full → tail → caught-up) over the real
  bridge proxy path, and a store test pins the exclusive-cursor semantics.
- **Safe `delete`/remove action for proposed changes (master plan §15 / §9.6).**
  Extends the proposed-change model with a fourth action that removes an existing
  file: a change now carries `action: "replace"` (default), `"create"`, `"rename"`,
  or `"delete"` (alias `"remove"`). A delete names a `path` and the **baseline
  hash** of the file it expects to remove; it carries **no new content** and **no
  destination**. **Backward compatible** — `action` stays a `#[serde(default)]`
  field, so older envelopes and persisted records (replace/create/rename) are
  unchanged. (1) **Core** — a `Delete` variant on `ProposedChangeAction`
  (`requires_baseline()` now includes it; `has_destination()` does not); capture
  records the source baseline like a replace, drops any declared content/destination
  (a delete only removes the `path`), and an unsafe/excluded `path` still drops the
  whole change. (2) **Kernel apply (the safety bar)** — a delete requires the same
  explicit **approval**, the same strict **safe relative / excluded-path** gate, and
  the same **workspace-root confinement** (resolve inside the canonical
  `working_dir`, no `..`/symlink escape) as a replace; it **refuses without a
  baseline** (no force in v1), verifies the target is an **existing regular file**
  (never a **directory or symlink** — both are refused) that **still matches its
  baseline** (a mismatch is an honest **conflict**, the file left untouched), and
  then removes it (`std::fs::remove_file`). The **transactional set apply** treats a
  delete's `path` as an occupied target — **distinct across the whole set** (a set
  that wants `replace` + `delete`, or `rename` onto, the same path is refused as a
  conflicting target) — validates all changes **together first** (no writes), and a
  mid-apply fault rolls back: replaces restored, creates deleted, renames moved
  back, and **deletes recreated** from their captured bytes (content restored as far
  as practical — file metadata such as permissions/timestamps is **not** preserved
  across the round-trip; the failure message is honest about an incomplete
  rollback). (3) **API** — unchanged routes; a delete's applied result reports its
  `path` and the **removed file's size**, a baseline conflict maps to the existing
  `409`, structural refusals (unsafe/overlapping path, directory/symlink target, no
  baseline) to `422`. (4) **Dashboard** — Run Detail shows the **Delete** action, a
  "delete" marker instead of a byte count, a delete-specific helper note, and **no
  content preview** (the file is removed). (5) **Tests** — core: delete capture with
  a baseline + no content/destination, the `remove` alias, delete on the wire,
  baseline-optional capture, drop on an unsafe/excluded path; kernel: delete removes
  a file / refuses a baseline-mismatch, missing-target, directory, or (Unix)
  symlink, end-to-end review→apply delete, a **mixed delete+replace+create set**
  applied atomically, a delete-baseline-conflict set leaving everything untouched, a
  **delete+replace same-path** refusal, a genuine **phase-2 rollback that recreates a
  deleted file**, and a **fake-CLI envelope with one delete** captured, approved, and
  applied into a temp workspace; dashboard `runview` delete parsing/label/`isDelete`
  + delete apply-eligibility; and the PowerShell smoke
  (`scripts/smoke-proposed-change-apply.ps1`) extended with the twelve new delete
  kernel tests. **Caveats / still not done:** arbitrary patch/diff parsing is
  deliberately not built (replacement is safer); a delete restores only **content**
  on rollback, not file metadata; and the transaction is still over **one run's**
  changes (one adapter → one workspace root). With delete modeled, the four core
  filesystem actions (replace/create/rename/delete) are now complete.
- **Safe `rename`/move action for proposed changes (master plan §15 / §9.6).**
  Extends the proposed-change model with a third action that relocates an existing
  file: a change now carries `action: "replace"` (default), `"create"`, or
  `"rename"` (alias `"move"`). A rename names a source `path`, a destination
  `dest_path` (aliases `to`/`to_path`/`dest`/`destination`/`new_path`), and the
  **source baseline hash**; it moves the file **intact** (no new content).
  **Backward compatible** — `dest_path` is a `#[serde(default)]` optional field, so
  older envelopes and persisted records (replace/create) stay valid. (1) **Core** —
  a `Rename` variant on `ProposedChangeAction` (+ `requires_baseline()` /
  `has_destination()` helpers) and a `dest_path: Option<String>` on
  `ProposedChange`; capture requires a **safe, relative, non-excluded destination
  distinct from the source** (else the change is dropped), drops any declared
  content (a move preserves bytes), and keeps the source baseline like a replace.
  (2) **Kernel apply (the safety bar)** — a rename requires the same explicit
  **approval**, the same strict **safe relative / excluded-path** gate on **both**
  source and destination, and the same **workspace-root confinement** (resolve
  inside the canonical `working_dir`, no `..`/symlink escape) as a replace; it
  **refuses without a baseline** (no force in v1), verifies the **source still
  matches its baseline** (a mismatch is an honest **conflict**, nothing moved),
  refuses if the **destination already exists** (a conflict — never overwritten) or
  equals the source, creates any **missing destination parent directories** (same
  safe policy as create), and then moves the file with `std::fs::rename` (atomic
  within the root's filesystem). The **transactional set apply** treats a rename as
  occupying **two** paths (source consumed + destination produced): both must be
  **distinct across the whole set** (no two changes may write/create/rename onto an
  overlapping path), all changes are validated **together first** (no writes), and a
  mid-apply fault rolls back — replaces restored, creates deleted, **renames moved
  back** to their source — leaving no net change. (3) **API** — unchanged routes; a
  rename's applied result reports the **destination** path and the moved file's
  size; a dest-exists / baseline conflict maps to the existing `409`, structural
  refusals (unsafe/overlapping path, no baseline) to `422`. (4) **Dashboard** — Run
  Detail shows the **Rename** action, the **`source → destination`** path, a "move"
  marker instead of a byte count, a move-specific helper note, and no content
  preview (the file is moved intact). (5) **Tests** — core: rename capture with a
  destination + no content, `move` alias + destination aliases, rename on the wire,
  drop on missing/unsafe/same-path destination, baseline-optional capture; kernel:
  rename moves a file / makes dest parent dirs / refuses a dest-exists,
  baseline-mismatch, missing-source, unsafe-destination, or same-path rename,
  end-to-end review→apply rename, a **mixed rename+replace+create set** applied
  atomically, a rename-dest-conflict and rename-baseline-conflict set leaving
  everything untouched, **overlapping rename/create + rename/replace targets**
  refused, a genuine **phase-2 rollback that moves a renamed file back**, and a
  **fake-CLI envelope with one rename** captured, approved, and applied into a temp
  workspace; dashboard `runview` rename parsing/labels/`source → destination`
  helper + rename apply-eligibility; and the PowerShell smoke
  (`scripts/smoke-proposed-change-apply.ps1`) extended with the ten new rename
  kernel tests. **Caveats / still not done:** `delete` is still not modeled (the
  next recommended action); a rename ignores any declared content (use a separate
  replace to also change content); and the transaction is still over **one run's**
  changes (one adapter → one workspace root).
- **Safe new-file `create` action for proposed changes (master plan §15 / §9.6).**
  Extends the proposed-change model beyond replace-over-an-existing-baseline with a
  second action: a change now carries `action: "replace"` (the default and the
  historical behavior) or `action: "create"` (a brand-new file). **Backward
  compatible** — a missing `action` (older envelopes and persisted records)
  deserializes as `replace`, and an unknown action string drops the change at
  capture (we never store a change we could not safely interpret). (1) **Core** —
  `relux_core::ProposedChangeAction` + a `#[serde(default)]` `action` field on
  `ProposedChange`; capture parses the action, and a `create` is recorded with
  **no baseline** (there is no prior file, so any declared baseline is dropped).
  (2) **Kernel apply (the safety bar)** — a `create` requires the same explicit
  **approval**, the same strict **safe relative / excluded-path** gate, and the
  same **workspace-root confinement** (resolve inside the canonical `working_dir`,
  no `..`/symlink escape) as a replace, but: it needs **no baseline**; the target
  **must NOT already exist** (an existing file, dir, or symlink is an honest
  **conflict** — never overwritten); any **missing parent directories** are created
  (each component is a sanitized, non-excluded, in-root name and the existing
  prefix has no symlink, so directory creation cannot be redirected outside the
  root — chosen policy: create parents when every component is safe, else refuse);
  and the file is placed **atomically** via an O_EXCL `create_new` reservation (so
  a racing creator loses) followed by a temp-file + rename (crash-atomic content).
  The **transactional set apply** validates every create/replace **together first**
  (no writes) and only then writes all; on a mid-apply fault it rolls back —
  replaces restored to their captured originals, **creates deleted** — leaving no
  net change, with an honest message if a rollback could not fully complete.
  (3) **API** — unchanged routes; a create-over-existing conflict maps to the
  existing `409`, structural refusals to `422`. (4) **Dashboard** — Run Detail
  shows the **action** (Create / Replace) per change, offers approve/apply and safe
  batch apply for creates (a create is apply-eligible once approved — no baseline
  needed), and replaces the "no baseline" note with an honest "New file — created
  only if it does not already exist" note for creates. (5) **Tests** — core:
  create capture, missing-action-defaults-to-replace, unknown-action-dropped,
  legacy-record deserialization, action-on-the-wire; kernel: create writes a new
  file (with parent dirs) / refuses an existing target as a conflict / refuses an
  excluded path, end-to-end review→apply create, a **mixed create+replace set**
  applied atomically, a create-conflict set leaving everything untouched, a genuine
  **phase-2 rollback that deletes a created file**, and a **fake-CLI envelope with
  one create + one replace** captured, approved, and set-applied into a temp
  workspace; dashboard `runview` action parsing/labels + create apply-eligibility;
  and the PowerShell smoke (`scripts/smoke-proposed-change-apply.ps1`) extended with
  the eight new create kernel tests. **Caveats / still not done:** only `replace`
  and `create` are modeled (no rename/delete); a create of an empty file is dropped
  at capture (same non-empty-content rule as replace); and the transaction is still
  over **one run's** changes (one adapter → one workspace root).
- **First safe multi-file transactional apply for a run's proposed changes
  (master plan §15 / §9.6).** Extends the single-file apply below with an
  **all-or-nothing transaction** over a *set* of a run's proposed changes: every
  selected change is validated **together first**, and the files are written only
  if **all** checks pass — otherwise **no file is modified** and every status stays
  honest. The single-change review/apply path is unchanged (and, per the doc, "if
  only one, the existing flow remains fine"). (1) **Kernel** —
  `apply_proposed_change_set(run_id, indices)` requires the selection to be
  non-empty with no duplicate/unknown indices, then requires **every** selected
  change to be `Approved`, to carry a **baseline hash** (no force), to have a
  **safe relative path distinct from every other** change in the set (no two
  changes may target the same file), and to resolve inside the run's single
  configured **`working_dir`** (one run → one adapter → one root) with **no
  `..`/symlink escape** to an **existing regular file** whose current SHA-256 still
  **equals the baseline**. Only then does the pure
  `apply_change_set_to_workspace` write each file **atomically** (a temp file then
  a rename); if a write fails mid-apply it **rolls the already-written files back**
  to the originals captured during validation and reports an honest failure
  (strict up-front validation is preferred precisely so this path is essentially
  unreachable). On success every change flips to `Applied` with one shared
  `applied_at` stamp, a `proposed_change_set_applied` transcript event and a
  `proposed_change:apply_set` **success** audit land; on any refusal each selected
  change records the honest reason and a **failed** audit lands. (2) **API** —
  `POST /v1/relux/runs/:id/proposed-changes/apply` with `{ "indices": [..] }`; a
  workspace baseline conflict is a `409`, any other inapplicable set (not approved
  / no baseline / no workspace / unsafe or duplicate target / unknown index) is a
  `422`, and an empty/absent selection is a `400` — never a fabricated `2xx`.
  (3) **Dashboard** — Run Detail shows a batch toolbar **only when a run has more
  than one proposed change**: **Approve all** (approves every still-reviewable
  change) and **Apply all approved (N)** (applies every approved+baselined change
  as one transaction, surfacing the honest all-or-nothing refusal reason).
  (4) **Tests** — kernel: multi-file atomic apply (shared stamp, one transcript
  event + success audit), **partial conflict leaves ALL files untouched** (both
  stay `Approved`, failed audit, no success), **duplicate target** refused,
  apply-time **unsafe-path** re-validation, missing-baseline-anywhere refused,
  empty/duplicate/unknown index selections refused, no-workspace refused, the pure
  set writer, and a **fake-CLI-envelope end-to-end with TWO proposed changes** that
  captures both, approves both, and applies both into a temp workspace; API
  status-mapping (409 vs 422); dashboard `reviewableProposedChangeIndices` /
  `applyEligibleProposedChangeIndices` / `showBatchProposedChangeControls`; and the
  PowerShell smoke (`scripts/smoke-proposed-change-apply.ps1`) extended with the
  transactional kernel tests + the new set-apply route refusing honestly.
  **Caveats / still not done:** the transaction is over **one run's** changes (a
  run has a single adapter, so one workspace root — cross-run/cross-root apply is
  out of scope); each change is still a **single-file full-content replacement over
  an existing baseline file** (no new-file create, rename, or delete — a missing
  target is a conflict); approval is still per-change (the UI "Approve all" loops
  the existing review endpoint — approval touches no files, so the transactional
  guarantee is specifically on the **writes**); and rollback is best-effort on a
  genuine mid-apply I/O fault — when it cannot fully restore, the failure message
  says so explicitly rather than overclaiming a clean revert.
- **Prime conversational brain handles a `proposed_changes` envelope honestly —
  no silent drop, no hidden work (master plan §15 + the AI "Conversational
  Shaping / Actionful Safety" section).** The Prime chat/brain path is
  **action-free by design** (it only runs on non-actionful turns, the chat prompt
  forbids claiming any state change, and `run_cli_brain` never performs a durable
  action), so — unlike the assigned-run path — it does **not** capture proposed
  changes into a run: there is no chat-turn run to hang a review/apply flow on,
  and synthesizing one would manufacture hidden, mutable work from a casual
  message. The chat bubble already shows only the human `result` text
  (`shape_cli_brain_reply`); now, rather than drop a declared change silently, the
  kernel surfaces a bounded, secret-free **advisory note** (`brain_envelope_advisory`)
  telling the operator a change was proposed during chat and to **create a task
  assigned to that adapter and run it** — the documented path that captures
  proposed changes with the safe review/apply flow. **Nothing is auto-created and
  nothing is auto-applied.** Hard tests pin the contract: a chat envelope carrying
  `proposed_changes` shows only the reply (no JSON, no `path`/`content`/baseline
  leak), the advisory is honest and secret-free, a plain greeting produces **no**
  advisory, and the `PrimeTurn` chat wire structurally carries no
  `proposed_changes`/`artifacts` field. (Considered and rejected: auto-attaching
  chat-turn changes to a synthetic run — it invents an undocumented surface and
  creates hidden mutable work; the assigned-run path is the real review/apply
  model.)
- **First real Relux diff/apply model — reviewed, applyable proposed changes
  (master plan §15 / §9.6).** Builds directly on the read-only artifact-reference
  capture below: a run can now carry **proposed file changes** an adapter declares
  in a dedicated `proposed_changes: [...]` envelope field, which the operator can
  **review (approve / reject)** and — once approved — **explicitly apply** into the
  run's controlled workspace root. The model is deliberately the smallest safe
  one: a **single-file, full-content replacement** with a **baseline hash**, NOT
  arbitrary patch/diff parsing (a replacement applies cleanly or refuses — no fuzzy
  hunk matching). **Nothing is ever auto-applied.** (1) **Capture** —
  `relux_core::capture_proposed_changes` (pure, never touches the filesystem) reads
  each item into a bounded `ProposedChange` (`path` / `new_content` /
  `baseline_sha256?` / computed `new_sha256` / `bytes` / `source` / `status`),
  computing the content's SHA-256 with `sha256_hex`. **Safety:** count capped
  (`MAX_PROPOSED_CHANGES = 32`), content capped (`MAX_CONTENT_BYTES = 256 KiB`) and
  required to be text (no NUL); the `path` must be **relative + safe + not excluded**
  (absolute / drive / UNC / `..` and vcs/build/secret paths are dropped, so a change
  can never target `.git`, a build dir, or a `.env`/`*.pem`); a `baseline_sha256`
  is validated as 64-hex or dropped to `None`. (2) **Apply** (the one place the
  kernel writes an agent-proposed file) refuses honestly and never fabricates
  success: it requires an explicit **`Approved`** state, **refuses without a
  baseline hash** (no force in v1), requires the run's adapter to have a configured
  **`working_dir`** (the controlled root), resolves the target **inside** that root
  with **no `..`/symlink escape**, requires an **existing regular file** whose
  current SHA-256 **equals the baseline** (a mismatch is an honest **conflict**, the
  file left untouched), and then writes **atomically** (temp + rename). Every
  outcome is audited and recorded on the run transcript; a refusal records the
  honest reason on the change. (3) **API** — `GET /v1/relux/runs/:id` flattens
  `proposed_changes` (with `status`) onto the detail; `POST …/proposed-changes/
  :index/review` records approve/reject; `POST …/proposed-changes/:index/apply`
  applies (409 not-approved / baseline-conflict, 422 no-baseline / no-workspace /
  unsafe target — never a fabricated 2xx). (4) **Dashboard** — Run Detail gains a
  **Proposed Changes** section: per-change path / status badge / size, a collapsible
  **content preview**, **Approve / Reject** (while proposed) and **Apply** (once
  approved, gated on a baseline hash) controls, and the honest refused-reason /
  applied / no-baseline lines; `reviewApplyAvailability` now returns
  `available:true` when a run proposed changes (apply is real for them) and adapts
  the reason otherwise. (5) **Tests** — core capture + path/baseline/size/content
  safety; the pure apply function (write-on-match, baseline-conflict-leaves-file,
  missing-target, path-escape); kernel review→apply, approval-required,
  no-baseline, no-workspace, reject, and a **fake-CLI-envelope end-to-end** that
  captures a proposed change, survives a snapshot round-trip, then approves +
  applies into a temp workspace; API `RunRecord` flatten; dashboard
  `runProposedChanges` / status helpers / `canReview`·`canApply` gating /
  availability; a stale-dist bundle guard for the new copy; and a PowerShell smoke
  (`scripts/smoke-proposed-change-apply.ps1`) wrapping the e2e test + HTTP route
  wiring. **Caveats / still not done:** apply is **single-file full-content only**
  (no multi-file transaction, no rename/delete, no new-file create — a missing
  target is a conflict in v1); the baseline must be a real SHA-256 the agent
  computed from the file it edited; live event streaming is still poll-based.
- **First real Relux run artifact model — read-only reference capture (master
  plan §9.6 / §15).** A Relux run can now record the **artifact references** an
  adapter declares in its structured result envelope, closing the gap where the
  run model carried no artifacts so review/apply could not be honestly built.
  This slice is deliberately **capture-only, never apply.** (1) **Parser** —
  `relux_core::capture_run_artifacts` reads an envelope's `artifacts: [...]`
  (objects or bare-string names) into bounded `RunArtifact` references
  (`name` / `type` ∈ file·diff·patch·log·url·note·other / `summary` / `source` +
  optional sanitized relative `path` + `bytes`); `parse_adapter_result` captures
  them only from a recognized envelope (an arbitrary JSON blob never qualifies)
  and tags each with the adapter source label. **Safety:** count capped
  (`MAX_ARTIFACTS = 64`) and every field capped, secrets redacted, and an unsafe
  declared path (absolute / Windows drive / UNC / `..` traversal) is **dropped**
  while the reference is still kept — the kernel **never reads the underlying
  file**. (2) **Persistence** — `Run.artifacts` is set on the durable run record
  on both completion and an `is_error` envelope failure, survives a snapshot
  round-trip (a dashboard refresh / restart), and the `adapter_output` transcript
  event records the captured count. (3) **API** — `GET /v1/relux/runs/:id`
  flattens `artifacts` onto the detail when present, omits the key entirely when
  empty (an honest empty state). (4) **Dashboard** — Run Detail gains a read-only
  **Artifacts** table (name / type / summary / source, with the sanitized path and
  size) plus an empty state; **apply stays unavailable** — `reviewApplyAvailability`
  now always returns `available:false` (capturing references does **not** enable
  apply; there is still no Relux diff/apply or review verdict), with the reason
  adapting between references-are-read-only and no-data-at-all. (5) **Tests** —
  parser (capture, unknown-type→other, traversal/absolute/UNC drop, count + field
  caps, secret redaction, bare-string, non-array), kernel persistence + snapshot
  round-trip driven by a **fake CLI emitting a structured envelope with artifact
  refs** (the integration smoke), API `RunRecord` flatten + empty-state omission,
  the dashboard `runArtifacts`/`reviewApplyAvailability`/`artifactTypeLabel`
  helpers, and a stale-dist bundle guard for the new copy. **Next slice:** the
  captured references define the contract for a real Relux diff/apply + review
  model; this slice does not build it (and never fakes it).
- **Relux local release v0.1.3 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.2` to `0.1.3` for the first build that turns
  Prime from a single local task runner into a governed **multi-agent
  orchestrator**. This release bundles the post-v0.1.2 orchestration work
  (detailed in the entries below): **multi-agent orchestration** — Prime
  decomposes a goal into role-typed briefs assigned to different agents and runs
  them as a governed batch (goal → brief → agent → run); **dependency-aware,
  round-based execution** — the planner infers simple ordering (implementation
  waits on research; testing/review/documentation wait on implementation) recorded
  as a DAG, and a round scheduler honestly marks dependents of a failed/blocked
  brief as blocked; **non-blocking, pollable jobs** — `…/orchestrations/:id/run-async`
  returns a job id immediately and `GET …/orchestration-jobs/:job_id` polls
  queued → running → completed/failed with live per-round/per-brief progress;
  **true bounded OS-parallel round execution** — independent briefs ready in a
  round run as real concurrent OS adapter processes (one thread per brief, up to a
  1..=4 concurrency cap) with the kernel lock released around the spawn window;
  and **sync API / CLI parallel parity** — the synchronous `POST …/run` and
  `prime orchestration run --concurrency N` now drive the **same** shared parallel
  executor as the job worker, so there is one execution implementation, not two.
  Every safety property is preserved on every path: dependency gating, at-most-once
  per round, permission + adapter-runtime gating before any spawn, secret
  redaction, the durable run transcript, audit, retry, sibling failure/panic
  isolation, and **no auto-run of downloaded plugin code** (only an explicitly
  enabled, operator-configured local binary spawns). Proven by deterministic
  rendezvous tests (two slow fake adapters that finish only if running at the same
  instant) and against the **real Claude CLI**. *Known caveats:* the in-memory job
  registry does not survive a server restart, but a poll **by orchestration id**
  (`GET …/orchestrations/:id/job`) stays restart-honest by reconstructing a job
  status from the durable record (`completed`/`interrupted`) — only the raw
  **by-job-id** poll 404s, since process-local job ids cannot be mapped back to an
  orchestration after a restart; the concurrency cap is 1..=4 and
  the per-call round budget is 1..=25; dependency inference is conservative
  role-co-occurrence (not a full task graph); planning does not auto-create agents;
  no background timer drives orchestrations (operator-triggered only); and a retry
  is a fresh attempt, not a partial-run resume. This version line is the
  `relux-kernel` crate version (separate from the Relix workspace version below);
  build the bundle with `scripts\relux-package-local.ps1 -FullE2E`. See
  `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **One shared parallel orchestration executor across the sync API, CLI, and async
  job.** The synchronous `POST /v1/relux/prime/orchestrations/:id/run` and the
  `prime orchestration run --concurrency N` CLI now perform the **same true bounded
  OS-parallel execution** the non-blocking job path already had — independent briefs
  ready in a round run as **real concurrent OS adapter processes**, not one-at-a-time
  under the lock (master plan §10.4 — "multiple tasks can run in parallel"). There is
  now **one execution implementation**, not two: the kernel's `run_orchestration`
  (used by the CLI and the blocking API handler) and the dashboard's background job
  worker both drive the same primitives — `prepare_orchestration_round` (schedule the
  ready set, start runs, resolve local-echo/pre-spawn-blocked briefs inline, return
  enabled-CLI spawn plans), the shared `run_briefs_in_parallel` (one OS thread per
  brief), and `finalize_prepared_brief` (merge each result via the shared
  `record_brief_outcome`). The prior sequential single-lock round loop
  (`run_one_orchestration_round`) is **gone**, so the two paths can no longer diverge.
  Safety is unchanged on every path: dependency gating, the at-most-once-per-round
  rule, permission + adapter-runtime gating before any spawn, secret redaction, the
  run transcript, audit, retry, failure/panic isolation between siblings, and **no
  auto-run of downloaded plugin code** (only an explicitly enabled, operator-configured
  local binary spawns). The job path keeps releasing the kernel lock around the spawn
  window and persisting between rounds for responsive polling; the synchronous API/CLI
  own the kernel for the whole batch (the API on the blocking pool so the async reactor
  is never parked), so two concurrent runs can never double-execute a brief. The
  synchronous `/run` and CLI **block until the whole batch is done** and return the
  final result; `run-async` still returns a job id immediately and is polled for live
  progress. Proven by a deterministic **rendezvous** test driving `run_orchestration`
  directly: two independent slow fake adapters each complete only if the other is
  running at the same instant (impossible if executed sequentially), finishing in ~1s.
- **True bounded OS-parallel execution for independent ready briefs.** Briefs
  that are ready in the same round now run as **real concurrent OS processes**, not
  one-at-a-time under the kernel lock (master plan §10.4 — "multiple tasks can run
  in parallel"). The CLI execution path is split into three phases around the
  single-owner lock: **prepare** (locked, persists) resolves the ready set, starts
  each brief's run, runs local-echo briefs inline, and hands enabled-CLI briefs back
  as fully-resolved spawn plans with their step already stamped (run id / start /
  round) so a poll sees them in flight; **spawn** runs every prepared brief's adapter
  process on its own OS thread **with the lock released**, so up to the concurrency
  cap (default 2, clamp 1..=4) run at once; **finalize** (locked, persists) merges
  each result back independently. Every safety property is preserved — permissions
  and adapter-runtime gating (a disabled/unconfigured runtime or missing binary is
  still blocked before any spawn), secret redaction, the run transcript, audit, and
  retry semantics all happen under the lock; **no downloaded plugin code is ever
  auto-run** (only an explicitly enabled, operator-configured local binary spawns).
  Each brief runs **at most once per round**; a failure — or even a panic — in one
  brief's thread never corrupts a sibling (each owns its own run/task records and
  merges separately). Dependencies still gate future rounds (a dependent is never
  even prepared while its dependency is pending). The non-blocking job now reports
  **multiple in-flight briefs** when several run together, and the dashboard surfaces
  the real parallelism ("round N · K briefs in parallel (cap C)"). Proven by a
  deterministic **rendezvous** test: two independent slow fake adapters each complete
  only if the other is running at the same instant — they finish in ~1s where a
  sequential round would spin ~30s — plus tests for safe merge, failure isolation
  (one brief fails, the sibling completes), and dependency preservation across the
  prepare/finalize split. *(Originally landed on the non-blocking job path only; the
  synchronous `POST …/run` and `prime orchestration run` CLI were brought onto the
  same shared parallel executor in the follow-up entry above — they are no longer
  single-lock sequential.)*
- **Dashboard polish for interrupted (restart-honest) orchestration jobs.** When a
  poll by orchestration id returns a status RECONSTRUCTED from the durable record
  (no live worker; master plan §15), the orchestration panel now renders a
  **distinct restart-honest callout** instead of the live-job banner: it labels the
  status as reconstructed — explicitly *not* a live run — shows the completed-vs-
  pending split, and points at **Continue** to resume only the pending briefs
  (completed briefs are never re-run). A reconstructed status is detected by its
  synthetic `durable:<id>` (`jobIsReconstructed`), so that id is never shown as a
  live worker. The panel also **hydrates** the durable job status once on load for
  any `running` orchestration, so a page reload after a restart still surfaces the
  callout (not only the session that pressed Run) — and the same path reconnects to
  a still-live job and resumes polling it. Because `interrupted` is terminal
  (`jobIsTerminal`), a reconstructed status schedules no further polling (no broken
  loop). New `orchestration.ts` helpers (`jobIsReconstructed` / `jobIsInterrupted` /
  `jobPendingCount`) and the refined cause-neutral phase label ("Interrupted — no
  live worker", no longer over-claiming a restart as the only cause) are pinned by
  added frontend tests (reconstructed-id detection, terminal/non-cancelable state,
  the poll gate scheduling nothing, and the Continue CTA). Dashboard-only; no backend
  or API change.
- **Per-brief recorded run duration in the orchestration view.** Each brief row now
  shows the duration its run actually took, next to the round (master plan §15: the
  view surfaces "real, already-recorded per-brief start/finish/round"). The new
  `stepDurationLabel` helper derives it purely from the kernel's recorded
  `started_at`/`finished_at` stamps and reuses the run view's single duration
  formatter, so timings read identically everywhere. Honest by construction: a brief
  that started but has not finished shows **no** duration (no fabricated live timer),
  and an unparseable or backwards stamp pair shows nothing rather than a wrong number.
  Pinned by `stepDurationLabel*` unit tests. Dashboard-only; no backend or API change.
  *(A live-browser click smoke for the interrupted **Continue** flow was evaluated and
  deliberately declined — it would need a 100s-of-MB browser engine or a DOM shim that
  still would not drive the real kernel, while the resume API is already proven by the
  resume/restart unit tests + smokes and the button by the render harness; see
  `apps/dashboard/README.md`.)*
- **Cooperative cancel/stop for orchestration jobs.** A running non-blocking job
  can be stopped honestly (master plan §15). `POST
  /v1/relux/orchestration-jobs/:job_id/cancel` sets a `cancel_requested` flag the
  worker checks **between** rounds (lock free, the prior round already persisted);
  it does **not** kill an adapter process mid-flight. The round already running
  finishes — every brief in it keeps its real recorded outcome — and the worker then
  stops *before* the next round and marks the job terminal `canceled`, leaving the
  remaining briefs `pending` for a human to resume with a fresh run. The endpoint
  only sets the flag (the worker owns the state transition, so cancel never races
  the worker on the state field): 200 + the updated job while active, 404 for an
  unknown job, 409 for an already-finished job; a cancel that arrives after the job
  finished leaves it `completed` — never a faked cancellation. The dashboard gains a
  Cancel button (disabled + "Canceling…" once requested) and surfaces the canceling
  phase and the canceled state. The cancel state machine and the cooperative worker
  stop (with a positive control proving the same plan runs to completion without a
  cancel) are unit-tested; a dedicated **live mid-flight cancel** HTTP smoke
  (`scripts/smoke-orchestration-cancel.ps1`) routes the first brief to a slow local
  CLI adapter spawned through the **real** adapter path, polls until it is genuinely
  `running`, cancels, observes `cancel_requested` while still `running`, then asserts
  the terminal `canceled` state with the in-flight brief recorded `completed` and
  every downstream brief left `pending`. A companion **multi-brief in-flight cancel**
  smoke (`scripts/smoke-orchestration-cancel-multi.ps1`) proves the same honesty
  contract for the harder case — a cancel that arrives while **two** independent
  briefs run together in one round: at `concurrency=2` it routes a research brief and
  an operations brief to two separate slow local CLI adapters (both via the real
  adapter path), polls until a single snapshot shows **both** `running`, cancels,
  observes `cancel_requested` while still `running`, then asserts terminal `canceled`
  with **both** in-flight briefs recorded `completed` honestly and the downstream
  implementation + documentation briefs left `pending`.
- **Resume-after-cancel for orchestration jobs (proven).** The other half of the
  cancel contract (master plan §15): a partially-done orchestration left behind by a
  canceled job is genuinely **resumable** — a fresh job picks up exactly where the
  canceled one stopped and never re-runs completed work. No production change was
  needed; the behavior falls out of the existing design (the duplicate-job guard only
  blocks `queued`/`running` jobs, so a terminal `canceled` job no longer counts, and a
  round only schedules `pending` briefs whose dependencies are `completed`). It is now
  **pinned**: a deterministic unit test
  (`a_second_job_resumes_only_pending_briefs_and_preserves_completed_runs`) budgets a
  first job to one brief, then starts a second job and asserts it ran *only* the
  still-pending briefs (`ran == pending count`), that each completed brief kept its
  **original** run id and round byte-for-byte, that each resumed brief earned a
  **new** run id, and that the orchestration ends fully `completed`. A dedicated
  **live resume-after-cancel** HTTP smoke (`scripts/smoke-orchestration-resume.ps1`)
  proves it end-to-end against real spawned processes: it runs the multi-brief cancel
  scenario (two slow CLI briefs caught `running` together, canceled mid-round, both
  recorded `completed`, downstream left `pending`), snapshots each brief's run id and
  round, then starts a **fresh** job on the same orchestration and asserts it is
  accepted (not a 409 duplicate), runs **only** the two pending downstream briefs
  (`job.ran == 2`), preserves the round-1 briefs' original run ids/rounds, gives the
  downstream briefs brand-new run ids distinct from the round-1 ones, and drives the
  record to fully `completed`.
- **Non-blocking orchestration jobs + live, pollable progress.** Running an
  orchestration no longer blocks on one long request (master plan "Orchestration
  (First Multi-Agent Slice)" — the previously-deferred non-blocking job model).
  `POST /v1/relux/prime/orchestrations/:id/run-async` starts a background job and
  returns immediately with a job id + `status_url`; `GET /v1/relux/orchestration-jobs/:job_id`
  (and `GET …/orchestrations/:id/job` by orchestration id) polls **queued →
  running → completed/failed** with the current round, per-brief statuses (briefs
  executing this round reported as `running`), running tallies, and the final
  aggregate result. The worker drives the SAME governed, tested `run_orchestration`
  one round at a time — releasing the kernel lock and **persisting the record
  between rounds** — so a mid-batch poll sees real, already-recorded progress;
  nothing fabricates in-flight work. **Duplicate starts are rejected** (409, one
  active job per orchestration) and the fleet is capped (429 past `MAX_ACTIVE_JOBS`).
  **Honest restart contract:** the job registry is in-memory only — a server restart
  mid-job loses the job record (a poll 404s) and the dashboard falls back to the
  durable orchestration record, which still carries whatever rounds actually
  completed. The dashboard **Run/Continue** now starts a job and polls it every 1s,
  rendering the live phase, a running tally, the worker's last event, and a real
  `running` badge on in-flight briefs (no bare spinner); the button is disabled
  while a job is active to prevent a duplicate start. Backend job
  lifecycle/duplicate/cap/aggregate logic and the frontend polling/progress helpers
  are unit-tested; end-to-end HTTP smokes (`scripts/smoke-orchestration-job.ps1` +
  a real-Claude-CLI variant `scripts/smoke-orchestration-job-claude.ps1`) prove the
  start → poll → terminal path against a live kernel.
- **Orchestration depth: dependency-aware, round-based batch execution.** The
  multi-agent batch is no longer a flat sequential loop (master plan §10.4
  Delegation Rules — "multiple tasks can run in parallel"; "Orchestration (First
  Multi-Agent Slice)"). The planner now **infers simple dependencies** when obvious
  roles co-occur in the goal — **implementation waits on research**, and
  **testing/review/documentation wait on implementation** — recorded as
  `depends_on` indices that only ever point at earlier briefs (a DAG by
  construction: no cycles, no deadlock). Goals without co-occurring roles get no
  dependencies and behave exactly as before (backward compatible). The run loop is
  a **dependency-gated, round-based scheduler**: each round it honestly marks any
  brief whose dependency failed/blocked as **blocked** (with a note naming the
  upstream brief — never run, never faked), collects the **ready** briefs (pending
  with every dependency completed), and runs up to a **concurrency cap** of them
  (`concurrency`, default 2, clamp 1..=4); it repeats until nothing is ready or the
  per-call `max` budget (clamp 1..=25) is spent. Termination is structural (every
  round moves ≥1 brief to a terminal outcome). Each brief records its
  **start/finish + round**; the batch result reports rounds, the cap, briefs
  **waiting** on a dependency, and briefs **blocked by a failed dependency**.
  Surfaces: `POST …/orchestrations/:id/run` accepts `{ max?, concurrency? }`;
  `prime orchestration run <id> [--max N] [--concurrency N]`; `prime orchestration
  show` lists each brief's dependencies + round. The dashboard panel shows the
  inferred dependencies in the preview, a per-orchestration **ready / waiting /
  blocked** readiness line, per-brief derived lifecycle badges
  (ready/waiting on a still-pending brief), the **round** each brief ran in, and the
  last batch's rounds + concurrency. **Proven against the real Claude CLI:** a
  mixed orchestration ran a real Claude research brief alongside a local-prime doc
  brief in **one round** (27s billed run), and a dependent chain ran a real Claude
  research brief in round 1 that **gated** a downstream implementation brief into
  round 2 (34s billed run) — fully traced goal → brief → agent → run.
  *Honest limits (when shipped; now superseded for the job path — see "True bounded
  OS-parallel execution" above):* briefs **within** a round executed sequentially
  through the kernel's single-owner lock (the cap bounded round size + pinned the
  contract; no OS-parallel CLI spawns yet), and an HTTP run is synchronous so the dashboard shows
  recorded round/timing/dependency state **after** the batch returns rather than a
  live mid-run feed (no fabricated in-flight progress). Backend tests pin
  dependency ordering, the concurrency cap (independent briefs share a round; cap 1
  serializes), a failed/blocked dependency honestly blocking its dependent with no
  run spawned, bounded no-runaway, and backward compatibility; frontend tests pin
  the readiness/lifecycle/dependency/round derivations.
- **Multi-agent orchestration (first slice): Prime as an orchestrator.** Prime can
  now decompose a multi-step goal into role-typed **briefs assigned to different
  agents** and run them in a **governed multi-agent batch**, instead of being a
  single local task runner (master plan section 10.4 Delegation Rules, section 15).
  Planning is a pure, deterministic brain
  (`relux_core::plan_orchestration`): it splits a goal into clauses, classifies
  each to a role (`research`/`implementation`/`testing`/`review`/`documentation`/
  `operations`/`general`), and grounds each role to a real agent on the roster (or
  falls back to Prime with an honest "hire a specialist" note). It is conservative
  — a goal that does not split into ≥2 briefs is not treated as multi-agent, so
  greetings and single tasks never storm. Creating an orchestration mints one brief
  (task) per step, assigns each to its agent, and records a durable
  `Orchestration` linking **goal → brief → agent → run** (persisted in the kernel
  snapshot/store, survives a refresh). Running is a separate governed batch: each
  pending brief runs through **its assigned agent's own adapter** (local Prime
  echoes; an **enabled** Claude/Codex CLI agent spawns the real CLI; a
  disabled/unconfigured runtime or missing permission is recorded as **blocked**,
  never faked), bounded by `max` (1..=25), running each brief at most once,
  recording per-agent outcomes + the next human action, and **stopping safely** (no
  loops, no runaway, never auto-runs downloaded plugin code). Surfaces:
  `relux-kernel prime orchestrate "<goal>"` / `prime orchestration list|show|run`;
  `POST /v1/relux/prime/orchestrate/preview`, `…/orchestrations` (create/list),
  `…/orchestrations/:id` (get), `…/orchestrations/:id/run`; a Prime-page
  **Orchestration** panel (goal → preview → create → run/continue with per-agent
  briefs and outcomes) and a Home summary card (pure logic in
  `apps/dashboard/src/orchestration.ts`, unit-covered). The background autonomy
  timer is unchanged — still deterministic, echo-only, never a paid CLI;
  orchestration is operator-triggered. **Proven against the real Claude CLI:** a
  two-agent orchestration where Prime (local echo) handled the research brief and a
  Claude-CLI `code-agent` handled the implementation brief — a real 44s Claude run
  with reported token usage and cost, fully traced goal → brief → agent → run.
  *Caveats (this first slice):* briefs ran sequentially with no dependency ordering
  — both addressed by the dependency-aware round scheduler above; planning still
  does not auto-create agents, and no background timer drives orchestrations yet.
- **Relux local release v0.1.2 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.1` to `0.1.2` for the first build that closes
  the three honest post-v0.1.1 gaps. **First-run brain onboarding:** Home's
  first-run checklist now derives a **live "connect Prime to a brain" step** from
  the control plane (`/v1/relux/ai/status` + `/v1/relux/adapters`) — it detects
  whether the Claude/Codex CLI is on PATH, reports whether the selected brain is
  actually usable, and routes the operator to Health → *Prime Brain / AI Runtime*
  with the exact next step (pure derivation in `apps/dashboard/src/onboarding.ts`,
  unit-covered). **Honest plugin install UX for metadata-only wrappers:** a
  generated metadata-only GitHub/zip wrapper is badged **Needs configuration**
  (never "enabled"/"ready"); its honest next step is **add tool definitions** (a
  one-click *Set up* with a copy/download manifest template), the install flow
  shows a **result summary** (tools discovered vs wrapper generated vs adapter),
  and the Tools list shows **only runnable tools** by default
  (`apps/dashboard/src/plugins.ts`, unit-covered). **Adapter run depth:** a CLI
  adapter run is now observable and recoverable — Run Detail shows the adapter,
  status, phase, a real measured duration, a redacted **output excerpt**, a clear
  failure reason, and (when reported) cost/usage, all from the durable transcript;
  the Claude adapter requests a **structured JSON result envelope** parsed into an
  honest summary + metrics (`relux_core::parse_adapter_result`, an envelope
  `is_error` is a failure even on a clean exit), Codex/generic commands degrade
  honestly to plain text, and a **failed run is retryable** as a fresh run
  (`prime.retry_run` → `POST /v1/relux/runs/:id/retry`) with lineage recorded
  (`retried_from`). Proven against the **real Claude and Codex CLIs**. *Caveats:*
  runs are synchronous (the page polls/refreshes rather than tailing live events),
  Codex/generic output is plain text (no structured envelope), and retry is a
  fresh attempt — **not** a resume of a partial CLI run. This version line is the
  `relux-kernel` crate version (separate from the Relix workspace version below);
  build the bundle with `scripts\relux-package-local.ps1 -FullE2E`. See
  `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Relux local release v0.1.1 (Windows bundle).** The `relux-kernel` /
  `relux-core` crates move from `0.1.0` to `0.1.1` for the first build that makes
  **Prime brain selection** a first-class dashboard surface. Health → *Prime
  Brain / AI Runtime* lets the operator choose who answers Prime's conversational
  turns — Local (deterministic), Claude CLI, Codex CLI, or OpenRouter — with a
  one-click *"Use Claude/Codex for Prime"* that enables the adapter and selects
  the brain together, plus live adapter status and the exact install/sign-in next
  step. No JSON editing or CLI flags are needed for normal Claude setup. The
  dev/test `echo` tool is no longer presented as a product path (internal smoke
  plumbing only), and the blank/legacy-route fix is retained. This version line is
  the `relux-kernel` crate version (separate from the Relix workspace version
  above); build the bundle with `scripts\relux-package-local.ps1 -FullE2E`. See
  `docs/RELUX_MASTER_PLAN.md` → *Release history*.
- **Release readiness CLI.** `relix release readiness` now prints the
  local first-release gate, current binary version, expected tag, git
  HEAD, clean/dirty state, and local/origin tag presence; it can run the
  full Windows-local release gate with `--require-clean --run-local-gate`
  without enabling GitHub Actions or spending model provider credits.

### Documentation

- Recorded the operator-console redesign that shipped in **v0.3.0**
  but was never logged here. The original single-page dashboard (task
  ledger plus topology and chronicle-retention widgets) was rebuilt
  into a multi-panel console; the current build carries twenty-two
  panels: Overview, Tasks, Scheduled Jobs, Chat, Memory, Approvals,
  Skills, Sessions, Reasoning, Credentials, Identity, Cost & Metrics,
  Observability, Policy Denials, Multi-Tenant, Planning, Workflows,
  Email, Plugins, MCP Servers, Configuration, and Logs. Source of
  truth: the `SECTIONS` array in
  `crates/relix-web-bridge/src/dashboard.html`.
- Corrected README and the dashboard docs to the real panel
  inventory and removed the false `#/...` hash-route claims; the
  console has no hash routing. Each panel is backed by a real route
  (for example `/v1/tasks`, `/v1/cron/jobs`, `/v1/policy/denials`,
  `/v1/mcp/servers`). There is no standalone Audit-log panel; audit
  data is reachable through the Credentials, MCP, and Multi-Tenant
  panels and the hash-chained `audit.log` files (read with
  `relix-flow-inspect`).
- Marked `ADVERSARIAL_AUDIT.md` (2026-05-29) as superseded. Its top
  findings were remediated across v0.3.0–v0.4.3-beta.1 (real approval
  channel dispatch, Argon2id credential KDF, fail-closed agent
  admission, intentional manual-only CI), so it overstates current
  risk and is retained for history only.

## [0.4.3-beta.1] - 2026-06-01

First build on the **beta** channel (GitHub pre-release; not "Latest").

### Added

- **Beta install channel.** `RELIX_CHANNEL=beta` (install.sh /
  install.ps1) installs the newest pre-release; `RELIX_VERSION` pins any
  exact tag. Per-OS one-liners documented in the README.

## [0.4.2] - 2026-06-01

Self-healing, long-lived node identities; documentation reconciliation;
manual-only CI; beta/stable release channels.

### Fixed

- **Identity bundles no longer lapse.** Locally-minted node/service
  identities now default to a **365-day** lifetime (was 24h), and the
  mesh-up scripts self-heal at boot via `relix identity ensure` —
  (re)minting any bundle that is missing, expired, signed by a stale org
  root, or within its renewal window. A fresh install always boots; a
  long-running mesh renews ahead of expiry. Expiry remains enforced.
- Stopped committing pre-minted `dev-keys/*.bundle` files (carried a
  wall-clock expiry + a local org root absent on fresh checkouts).

### Added

- **`relix identity ensure`** — self-healing/renewing mint used by boot
  and the mesh-up renewal loop. `BundleHeader::needs_renewal` /
  `seconds_until_expiry` renewal primitives in `relix-core`.
- **Beta + stable release channels** driven by tag shape: `vX.Y.Z` =
  stable (Latest), `vX.Y.Z-beta.N` = GitHub pre-release. See
  `docs/releasing.md`.

### Changed

- **CI is manual-only** (`workflow_dispatch`) — no pass/fail status on
  every commit; the CI badge was removed from the README.
- **Documentation reconciled with the 0.4 codebase** — 78 docs updated,
  8 new (planning, four-layer-memory, memory-security, reasoning-pipeline,
  credentials, approval-tokens, embedded, channels/email).
- Workspace version bumped to `0.4.2`.

## [0.4.1] - 2026-06-01

Release engineering fix for the `aarch64-unknown-linux-gnu` cross build.

### Fixed

- **`Cross.toml` arm64 OpenSSL**: added `pre-build` hook that installs
  `libssl-dev:arm64` inside the cross container, fixing the link
  failure for `aarch64-unknown-linux-gnu` release targets.

### Changed

- Workspace version bumped to `0.4.1`.

## [0.4.0] - 2026-05-31

Headline features shipped in the 0.4 series (on top of the 0.1 mesh
foundation). No wire-format or config-breaking changes from 0.3.

### Added

- **Multi-agent planning pipeline** (`[planning]`) — coordinator-side
  planner + critic that decomposes natural-language specs into
  delegated sub-tasks. Inspect via `relix planning plan`.
- **Knowledge-share** (`[knowledge]` + `[knowledge_trust]`) —
  peer-to-peer observation transfer with Ed25519-bound provenance.
  Source trust configured per public key; `allow_unbound_sources = false`
  is the fail-closed default.
- **Training pipeline** (`[training]`) — interaction recording to
  SQLite, optional PII anonymisation, quality scorer, OpenAI-format
  export via `relix training export`.
- **Confidence / reasoning engine** (`[confidence]`) — per-method
  rolling-window confidence scorer; feeds the judge + belief-state
  engine. Inspect via `relix confidence history`.
- **Metrics, observability, and alerting** (`[metrics]`,
  `[observability]`) — SQLite metrics store, cost-by-model tracking,
  OTLP export, configurable alert thresholds with fan-out targets.
  Live TUI via `relix observe`.
- **Credentials vault** (`[credentials]`) — AES-GCM encrypted at-rest
  credential store; JIT secret injection into tool args via
  `{{secret:<name>}}`. Managed via `relix credentials`.
- **Approval gate + Ed25519 approval tokens** (`[approval]`) —
  per-method approval requirements; `coord.approval.decide` mints
  Ed25519-signed tokens (TTL 30–86400 s, default 300 s). Standing
  approvals and out-of-band delivery channels supported.
  `RELIX_APPROVAL_SIGNING_KEY` env var required for token minting.
- **Mesh PII gate** (`[mesh_pii]`) — inline regex scan of every
  inbound `RequestEnvelope.args` before handler dispatch; actions
  `block`, `redact` (default), `log_only`. Writes `pii_events.sqlite`
  chronicle; queryable via `relix pii stats/events`.
- **Plugin sandbox** — `plugin_host` node type; each capability
  registered under bare name + `plugin_host.<method>` alias.
- **Tenant isolation** — per-tenant policy files (`[policy] dir`);
  per-tenant SQLite audit mirror (`[audit] partition_by_tenant`);
  queryable via `node.audit.tenant_list` / `node.audit.tenant_recent`.
- **Budget enforcer** (`[budget]`) — per-caller spend caps; dormant
  when no caps are configured.
- **`email` controller node type** — SMTP outbound + IMAP inbound
  channel bridge; manageable via `relix email`.
- **YAML flow format** — `.yml`/`.yaml` flows lowered to SOL before
  VM execution; dispatched by `FlowRunner` alongside `.sol` and
  `.sflow`.
- **Streaming `remote_call_stream`** — SOL VM opcode + flow-runner
  dispatcher over `/relix/rpc/stream/1` substreams with chunk
  observer and cancel signal.
- **Per-tenant audit partition** (GAP 23C) — `AuditPartitionStore`
  SQLite mirror with tenant sanitisation; two new built-in caps
  `node.audit.tenant_list` and `node.audit.tenant_recent`.
- **Transactional gateway** (`[execution]`) — three-tier action
  classification (auto-compensated / human-rollback / blocked),
  persistent `TransactionStore`, `EvidenceStore` with PII redaction
  and state-diff capture. CLI surface: `relix execution`.

### Changed

- **`validate_controller_node_type` (SEC §13)** — unknown `node_type`
  values are now hard errors at boot. Previously they produced a
  silent no-op process that appeared healthy.
- **Node-type set expanded** — `SUPPORTED_CONTROLLER_NODE_TYPES` now
  includes `email` alongside `memory`, `ai`, `coordinator`,
  `telegram`, `discord`, `slack`, `plugin_host`, `tool`.

## [0.1.5] - 2026-05-25

Boot-loop polish on top of the v0.1.4 install fixes. No
mesh-protocol or wire-format changes — same binaries, same flow
templates, same configs.

### Fixed

- **`relix boot` now blocks the terminal until the mesh stops**
  instead of returning the prompt as soon as the bridge becomes
  healthy. Previously the boot script's cleanup output raced the
  shell prompt — operators saw their prompt back before the
  controllers had finished tearing down on `relix stop` from
  another terminal. The boot command now waits on the script's
  exit and forwards Ctrl-C through to it.
- **PowerShell mesh script: replaced `TreatControlCAsInput` loop
  with a 500ms poll loop** that works correctly when the script is
  launched via `Command::spawn` from `relix boot`. The old loop
  silently no-op'd in non-interactive spawned contexts, leaving
  the script running forever after a clean `relix stop`.

## [0.1.1] - 2026-05-24

Zero-configuration install. After this release the
`curl | bash` / `irm | iex` one-liner ends with a running mesh
and an open dashboard — no env vars to export, no scripts to
clone, no flags to remember.

### Added

- **`relix setup`** — guided interactive wizard. Five pages
  (welcome → provider picker → hidden API-key input → channel
  multi-select with per-channel secret follow-ups → confirm and
  save). Runs automatically at the end of `install.sh` /
  `install.ps1`; can be re-run any time to change provider,
  rotate keys, or add a channel. crossterm-driven raw terminal
  input; Ctrl-C exits 130 with the terminal restored.
- **`~/.relix/config.toml`** — persistent operator config. Holds
  `[provider]` (name + api_key), `[channels]` (per-channel
  toggle + token + channel-id), and `[mesh]` (data_dir,
  bridge_port). Written `chmod 600` on POSIX via tmp-write +
  rename so an interrupted save can't half-write the file.
  Every field has a serde default so partial configs deserialise.
- **Config-driven `relix boot`** — reads
  `~/.relix/config.toml` on startup and translates it into the
  env vars the mesh-up script consumes. The right
  `OPENROUTER_API_KEY` / `OPENAI_API_KEY` / etc. is set
  automatically from `provider.api_key`; channel toggles +
  tokens are wired through. Explicit `--with-*` flags still
  stack on top.
- **`memory.recent_for_session` auto-injection** — `[ai.memory_peer]
  max_history_turns = N`. With this set, the AI node fetches
  recent turns itself and merges them with any caller-supplied
  history, so flow templates no longer need to chain
  `memory.recent_for_session` → `ai.chat` manually. Silent skip
  on memory peer failure.
- **RAG retrieval** — `[ai.memory_peer] rag_enabled = true` +
  `rag_top_k` + `rag_min_score`. When set, the AI node embeds
  the user prompt locally and queries `memory.search` across
  both agent and user vector stores, formatting the top-K hits
  as a "Relevant context from memory" block prepended to the
  system prompt. `memory.search` wire grew an optional
  `embedding=<base64-LE-f32>` 5th field so the precomputed
  vector skips the responder's own embed RPC. Silent skip on
  empty results, embedding failure, or peer unreachable.
- **`GET /ws/chat`** — WebSocket streaming endpoint. JSON
  request `{session_id, message, model?}` followed by a stream
  of `{type: "chunk", text: "..."}` frames terminated by
  `{type: "done", session_id, text}`. Bearer auth on the
  upgrade (`Authorization: Bearer <token>`; loopback alpha
  accepts any non-empty token). `ChatProvider` gained
  `generate_reply_stream`; the mock provider streams
  word-by-word with a 20ms gap, and the OpenAI-compatible
  provider parses real `delta.content` deltas from the upstream
  SSE response.
- **`relix boot` / `relix stop` / `relix status`** — top-level
  CLI subcommands implemented in `crates/relix-cli/src/mesh.rs`.
  Cross-platform shim around the mesh-up scripts; `stop` kills
  by name (`taskkill /F /IM` on Windows, `pkill -x`
  elsewhere); `status` polls `/health` + `/v1/topology` and
  prints a peer-by-peer table.
- **`relix setup` bundled with install** — install scripts now
  call `relix setup` as their last step. They also fetch the
  mesh-up + mesh-down scripts from the main branch and drop
  them in `~/.local/scripts/` so `relix boot` has them after a
  binary-only install. `scripts/relix-mesh-down.ps1` ships as
  the Windows counterpart to `relix-mesh-down.sh`.
- **All three binaries in each release archive** — every
  per-target archive now contains `relix` (= `relix-cli`),
  `relix-controller`, and `relix-web-bridge` so `relix boot`
  can spawn its siblings from the same directory.

### Changed

- **Default data dir** is now `~/.relix/data/<run>/` instead of
  the repo-relative `dev-data/<run>/`. Repo-checkout
  development still uses `dev-data/` automatically. Docs and
  README updated.
- **README + getting-started** rewritten around the wizard
  flow. Env-var exports for API keys are no longer the
  recommended path — config-file primary, env-var fallback.
- **CI workflow** runs on manual `workflow_dispatch` only;
  contributors run the same gates locally
  (`cargo fmt --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`). Re-enable push
  triggers when CI gates are needed on every commit.

### Fixed

- `install.ps1` no longer crashes with "the property 'Count'
  cannot be found on this object" under PowerShell strict mode
  when the release zip contains a single `relix.exe`.
- `parse_literal_ip` in `tool.web_fetch`'s SSRF guard now
  strips brackets from IPv6 hosts (`url::Url::host_str()`
  returns IPv6 with brackets); previously `[::1]` and
  `[fe80::1]` fell through to DNS and were rejected as
  `DnsFailed` on Linux/macOS instead of `IpForbidden`.
- `.sflow` parser preserves the user's dotted target verbatim
  as `wire_method`, and plugin capabilities are double-
  registered (bare name + `plugin_host.<method>` alias) so the
  natural `step x: plugin_host.hello.greet "..."` form admits
  against the bridge handler.

## [0.1.0] - 2026-05-23

First public alpha. Everything below is real and ships.

### Mesh and dispatch

- Mesh of OS-process peers connected via libp2p (`/relix/rpc/1`
  over TCP + Noise XK + Yamux). CBOR envelopes carry caller's
  signed `IdentityBundle`, method, args, deadline.
- Six controller node types (`memory`, `ai`, `tool`, `coordinator`,
  `router`, `plugin_host`) plus the `relix-web-bridge` HTTP front.
  Each node is its own OS process with its own dispatch bridge.
- Admission pipeline on every responder: decode → identity verify
  → deadline check → `PolicyEngine` evaluate → handler dispatch
  → audit append. The audit log is signed and hash-chained
  (`relix-core/src/eventlog.rs`).
- Five built-in capabilities on every node: `node.health`,
  `node.manifest`, `node.dispatch.stats`, `node.policy.simulate`,
  `node.policy.recent_denials`.

### AI and memory

- `ai.chat` and `ai.embed` on the `ai` node, with provider routing
  for `mock`, `openai`, `openrouter`, `xai`, `anthropic`, `gemini`,
  and a `local` Ollama-compatible base URL. Provider keys live only
  in the AI node's local config.
- `memory.write_turn`, `memory.recent_for_session`,
  `memory.search_turns` (FTS5) on the `memory` node — SQLite-backed
  per-session conversation history.
- Vector memory: `memory.embed`, `memory.search` (cosine,
  top-K up to 20), `memory.embed_all`. Default 8-dim mock vectors;
  switch the AI node to OpenAI-compatible to get real
  `text-embedding-3-small`. See `docs/vector-memory.md`.
- Persistent agent memory: `memory.agent_read`, `memory.agent_write`,
  `memory.agent_curate`, `memory.curator_status`.

### Tools

- File system: `tool.read_file`, `tool.write_file`, `tool.append_file`,
  `tool.patch`, `tool.patch_preview`, `tool.fuzzy_replace`,
  `tool.search_files`, `tool.list_dir`, `tool.fs.tree`,
  `tool.fs.stat`, `tool.binary_sniff`, `tool.fs.audit_recent` —
  all scoped to operator-configured jail roots.
- Web: `tool.web_fetch`, `tool.web_get`, `tool.web_search`,
  `tool.web_extract`, `tool.web.post`, `tool.web.robots_check`,
  `tool.web.blocklist_summary` — SSRF-guarded, blocklist-aware.
- Terminal: `tool.terminal.run` and friends — allowlisted commands
  only, via `portable-pty`. Sessions are pausable, resumable, and
  fully audited.
- Browser automation: `tool.browser.*` — headless Chrome / WebDriver
  with per-session lifecycle.
- MCP integration: `tool.mcp.list_servers`, `tool.mcp.list_tools`,
  `tool.mcp.invoke` — registers external MCP servers as proxied
  capabilities.
- PDF and text: `tool.pdf`, `tool.text.chunk`.

### Coordinator

- Durable task ledger: `task.create`, `task.update`, `task.event`,
  `task.list`, `task.get`, `task.attempt`, `task.todo`,
  `task.metadata`, `task.link_parent`, `task.cancel`, `task.retry`,
  `task.recover`, `task.replay`, `task.lineage`, plus pause/resume/
  freeze/unfreeze and note/investigation.
- Multi-agent coordination: `delegate.spawn`, `delegate.result`,
  `delegate.cancel`, `delegate.list` with a configurable depth cap.
- Inter-task messaging: `msg.send`, `msg.inbox`, `msg.read`,
  `msg.thread`, `msg.delete` with TTL.
- Cron / scheduler: `cron.create`, `cron.list`, `cron.get`,
  `cron.update`, `cron.delete`, `cron.trigger` — supports cron
  expressions, duration intervals, and one-shot.

### Channels

- Telegram, Discord, and Slack channel controllers. Each polls the
  bot platform's API, forwards messages to AI through the same SOL
  flow used by the HTTP bridge, and persists conversation history
  in `memory`. Opt-in per channel via env vars.

### Plugins

- `plugin_host` node type with `relix-plugin-v1` HTTP/JSON protocol
  for subprocess plugins. SDK crate (`relix-plugin-sdk`) for Rust
  authors; the protocol is the contract, so plugins in any language
  that can speak HTTP are supported (Python example ships).
- Management capabilities: `plugin.list`, `plugin.status`,
  `plugin.reload`, `plugin.disable`. Each registered under both the
  bare name and a `plugin_host.<method>` alias so both SOL and
  `.sflow` can call them.

### Orchestration

- **SOL** — a small Rust-like imperative DSL with one mesh primitive,
  `remote_call(peer, method, args)`. Typed `str` values, `let`, `if`,
  `while`, `for`, function definitions, `print`, `return`.
- **`.sflow`** — a line-oriented step-based DSL with `if`/`elif`/
  `else`, `loop N times`, `while`, `until`, `try`/`catch`/`rethrow`,
  `set var = ...`, `${var}` interpolation, and `sol.log` /
  `sol.sleep` / `sol.assert` / `sol.set_result` built-ins. The
  parser preserves the user's dotted target verbatim as
  `wire_method`, so plugin and multi-segment capabilities admit
  correctly.

### HTTP bridge

- OpenAI-compatible `/v1/chat/completions` (including SSE
  streaming via `/chat/stream`) routed through the SOL chat flow.
- Operator dashboard at `/dashboard`: a single page with the task
  ledger plus collapsible mesh-topology and chronicle-retention
  dry-run widgets.
- Direct HTTP surfaces for every operator workflow listed above —
  see `docs/configuration.md` and the route list in
  `crates/relix-web-bridge/src/main.rs`.

### CLI

- `relix-cli` (installed as `relix`) with subcommands `identity`,
  `ping`, `task`, `capability`, `topology`, `ops`, `router`, `mcp`,
  `fs`, `web`, `browser`, `sol`, `doctor`, `terminal`, `flow-run`.
- New top-level wrappers: `relix boot`, `relix stop`, `relix status`
  — cross-platform mesh control over the underlying PowerShell /
  bash boot scripts.

### Tooling

- GitHub Actions CI (`fmt`, `clippy -D warnings`, `test --workspace`
  on Linux / macOS / Windows).
- Cross-platform install: `install.sh` (Mac / Linux) and
  `install.ps1` (Windows) that fetch pre-built release binaries.
- Mesh boot scripts: `scripts/relix-mesh-up.ps1` (Windows) and
  `scripts/relix-mesh-up.sh` (POSIX), with `relix-mesh-down.sh` for
  shutdown.

<!--
  Link targets. This repo's real, published releases use the `relux-v0.1.x` tag
  scheme (see the "Relux local release v0.1.x" bullets under [Unreleased] and the
  GitHub Releases page). The dated sections below are the LEGACY Relix workspace
  versions; they were never cut as individual GitHub releases in this repo, so they
  point at the Releases list rather than non-existent `vX.Y.Z` tags (which 404).
-->
[Unreleased]: https://github.com/itsramananshul/Relix/compare/relux-v0.1.10...HEAD
[0.4.3-beta.1]: https://github.com/itsramananshul/Relix/releases
[0.4.2]: https://github.com/itsramananshul/Relix/releases
[0.4.1]: https://github.com/itsramananshul/Relix/releases
[0.4.0]: https://github.com/itsramananshul/Relix/releases
[0.1.5]: https://github.com/itsramananshul/Relix/releases
[0.1.1]: https://github.com/itsramananshul/Relix/releases
[0.1.0]: https://github.com/itsramananshul/Relix/releases
