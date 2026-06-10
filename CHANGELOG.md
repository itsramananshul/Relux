# Changelog

All notable changes to Relix are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once a stable release is cut.

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/itsramananshul/Relix/compare/v0.4.3-beta.1...HEAD
[0.4.3-beta.1]: https://github.com/itsramananshul/Relix/releases/tag/v0.4.3-beta.1
[0.4.2]: https://github.com/itsramananshul/Relix/releases/tag/v0.4.2
[0.4.1]: https://github.com/itsramananshul/Relix/releases/tag/v0.4.1
[0.4.0]: https://github.com/itsramananshul/Relix/releases/tag/v0.4.0
[0.1.5]: https://github.com/itsramananshul/Relix/releases/tag/v0.1.5
[0.1.1]: https://github.com/itsramananshul/Relix/releases/tag/v0.1.1
[0.1.0]: https://github.com/itsramananshul/Relix/releases/tag/v0.1.0
