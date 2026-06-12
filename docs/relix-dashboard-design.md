# Relix Dashboard — Deep Design

> **Companion to `relix-company-model.md` and `relix-execution-and-issue-design.md`.** Ideas and structure only — no code. Grounded in a complete read of Paperclip's React dashboard (all 171 components + 61 pages + the realtime layer) and Relix's current operator console (the single-file 22-panel `dashboard.html` served by the web-bridge).
>
> **The goal:** reshape Relix's operator UI from a *grid of feature panels* into a *goal-facing company console* — organized around work objects, with the issue board as the spine and the issue as a live chat thread — keeping every existing Relix capability, just relocated to where it belongs.

---

## 0. The starting point (Relix today vs the target)

> **STATUS (Phase 2 complete):** the React SPA in `apps/dashboard` is now the canonical and ONLY dashboard, served at `/dashboard`. The original self-contained `dashboard.html` console described below has been **deleted** (Phase 2 Slice 3); the interim `/spine` board was deleted in Slice 2 (`/spine` now 308-redirects to `/dashboard`). The text below is kept as the design rationale for that migration — it is historical context, not the current state.

**Relix originally (the starting point this doc reshaped):** one self-contained `dashboard.html` (CSS + ~8K lines of inline JS, no build step) served by the web-bridge, with **22 flat panels** in five groups (Overview, Tasks, Cron, Chat, Memory, Skills, Sessions, Reasoning, Approvals, Credentials, Identity, Cost, Observability, Denials, Tenant, Planning, Workflows, Email, Plugins, MCP, Config, Logs). It already had a Paperclip-inspired grouped nav and spine-status badges — a real head start — but it was organized **by feature**, so it felt like a control panel.

**The target:** a console organized **by work object** — Inbox · Issues (board) · Projects · Goals · Org · Agents · Approvals · Costs · Activity · Chat — where the 22 feature panels become *detail tabs on the object they describe* (memory on an agent, confidence on a run, etc.). Same power, goal-facing shape.

---

## 1. Decision: the front-end stack

This is the one upfront decision the reshape forces. The target experience — a drag/drop kanban board, an issue rendered as a streaming chat thread, a pan/zoom org chart, surgical realtime updates — is genuinely hard to build well in one hand-written HTML file. Paperclip uses a React + Vite SPA (TanStack Query for data, a custom company-prefix router, an agent-chat runtime) precisely because this class of UI needs it.

**DECIDED (now DONE): build a real React SPA. The vanilla `dashboard.html` is retired — not grown.** (React + a query cache + a component library), served by the web-bridge the same way the HTML file was, so deployment doesn't change. The 22-panel vanilla console was thrown away, not preserved — it stayed reachable only until the SPA reached parity, and was **deleted in Phase 2 Slice 3**. Paperclip's React app is the proven structural blueprint to mirror (not copy visually — see §12 for the look).

---

## 2. The shell (three zones)

Mirror Paperclip's proven shell:

- **Left nav rail** (collapsible, resizable, persisted width; collapses to icons). Swaps by context: the main company nav, an **Instance/Settings** nav under settings routes, and a **Company-settings** nav under company-settings routes.
- **Main content** (full-width) with a **breadcrumb bar** on top.
- **Right "properties" panel** (contextual, ~320px) that pages inject content into for the selected entity — *not* a fixed sidebar; it appears on detail views (issue properties, goal properties, agent config) and slides away elsewhere.
- **Mobile:** the left nav becomes an off-canvas drawer (edge-swipe to open), plus a fixed **bottom nav** (Home / Issues / **Create** / Agents / Inbox) — so the whole thing is usable from a phone, which matters for "monitor my autonomous company on the go."

Global singletons mounted once in the shell: the command palette (⌘K), the New-Issue / New-Agent / New-Project / New-Goal dialogs, a toast viewport, and the keyboard-shortcuts cheatsheet.

**Never blank (RELUX_MASTER_PLAN §17.6).** The routed workspace is wrapped, *inside*
the shell, by a route-level **ErrorBoundary** (`apps/dashboard/src/components/ErrorBoundary.tsx`):
a render-time throw in any one page renders a readable error card (with the honest
message, a "Try again", and a "Reload") instead of white-screening the whole SPA.
The sidebar/topbar stay usable, and the boundary resets on navigation (keyed on the
pathname), so a bad page never strands the operator. This is the structural backstop
behind the per-page loading/error/empty states — together they make a blank page
impossible regardless of API state or an unanticipated data shape. The pure
message formatter (`errorBoundaryMessage`) is unit-tested; each Relux page also has
a server-render test (e.g. `test/work-render.test.mjs`, `test/crew-render.test.mjs`)
that fails if the page throws on mount under the app's declarative router.

---

## 3. Navigation (grouped around work objects) + where the 22 panels go

**Top of the rail:** a **tenant/company switcher** (Relix is multi-tenant — this is the "which company am I running" selector), a global **search** button, and a **New Issue** button.

**Personal section:** **Inbox** (badge = pending approvals + budget alerts + failed runs + stranded work) and **My Issues**.

**Work section:** **Issues** (the board — the workhorse), **Routines**, **Goals**.

**Company section:** **Dashboard** (the glance view), **Org** (the chart), **Agents**, **Costs**, **Activity**, **Chat** (the companion), **Settings**.

**Where the current 22 Relix panels relocate** (nothing is lost — everything gets a home on the object it describes):

| Current Relix panel | New home |
|---|---|
| Tasks | becomes **Issues** (the board) — the spine |
| Cron | **Routines** |
| Chat | the **Chat companion** (upgraded — see Part 13) |
| Overview | the **Dashboard** glance view |
| Memory | a tab on the **Agent** detail (its memory) + a company-level knowledge view |
| Skills | a tab on the **Agent** detail (and a company Skills library under Settings) |
| Sessions | a tab on the **Agent** detail / surfaced per **Run** |
| Reasoning / Confidence / Judge / Belief | tabs on a **Run** detail ("how sure / how it decided") |
| Approvals | the **Approvals** surface + the **Inbox** |
| Credentials / Secrets | **Settings → Secrets** + per-agent "secrets it may use" toggles |
| Identity | **Settings → Access/Identity** + the agent's identity on its detail |
| Cost | **Costs** (with budgets) + per-issue/agent/project cost |
| Observability / Metrics | **Costs/Activity** or a **System** area; per-agent health on the agent |
| Denials (policy) | **Activity** (governance) + **Settings → Policy** |
| Tenant | the tenant switcher + **Settings → Multi-tenant** |
| Planning | folded into **Issues** (the plan-as-document + decomposition flow) |
| Workflows | **Routines** / the orchestration view; or kept under **System** |
| Email / Channels | **Settings → Channels** + channel status tiles |
| Plugins / MCP | **Settings → Plugins / MCP** + per-agent tool toggles |
| Config | **Settings → Configuration** |
| Logs | **Settings → System → Logs** (and per-run transcript inline) |

The rule: **a feature lives on the object it acts on.** An agent's memory/skills/sessions show on the agent. A run's confidence/reasoning shows on the run. Governance (policy/secrets/tenants/plugins) lives under Settings. The top-level nav is *only* work objects.

---

## 4. The tenant/company-prefix router (steal this exactly)

Paperclip's cleverest structural trick: it re-exports the router but **overrides the navigation primitives** so any link written as `/issues` is automatically rewritten to `/{companyPrefix}/issues`. Every page is written company-agnostic; the prefix is woven in transparently. URLs are shareable and company-scoped, but pages never think about the prefix.

For Relix (multi-tenant mesh), the same idea applies with a **tenant prefix**. The selected tenant lives in app state (not forced into every URL the developer writes), and the router injects it. This one pattern is what keeps the whole app clean as it grows — adopt it on day one of the SPA.

---

## 5. The Inbox (the operator's home)

The single action center showing only **what needs you**, in priority order, computed from live state (not a notification table):

1. **Approvals** (hire / strategy / budget-override / high-risk) — inline Approve/Reject.
2. **Recovery decision cards** (the diagnosis-driven escalations — see `relix-execution-and-issue-design.md` §3.3b): when work can't self-heal, a card explains the root cause *in plain language* and offers one-click choices — **Retry now / Block / Reassign / Investigate (opens the chat companion) / Dismiss**. Transient failures retry silently and never appear here, so the Inbox stays signal, not noise.
3. **Alerts** (agent errors, budget thresholds crossed, failed runs).
4. **Stranded / blocked work** (issues stuck with nobody moving them — surfaced by the recovery layer's liveness signals).

This is where the Board (you) lives day-to-day. The sidebar Inbox badge is the sum of these.

---

## 6. The Issue board (the spine)

One surface owns list↔board, and it's the most-used screen. Mirror Paperclip's `IssuesList` design:

- **List ↔ Board toggle.** Board = kanban; list = grouped rows.
- **Columns** = the issue statuses (Backlog → Todo → In Progress → In Review → Blocked → Done → Cancelled). Drag a card to a column → status mutation (with transition validation; an invalid drop shows a toast).
- **The kanban is "dumb" and density-driven** — the list container owns the brains. Above a high-volume threshold it auto-switches to compact cards and collapses cold lanes (backlog/done/cancelled), with per-column "show more" pagination so columns aren't truncated.
- **Grouping** (list mode): by status / priority / assignee / project / parent / none. **Sub-issue nesting** (a tree, indented).
- **The "workflow checklist" rendering** (the goal-facing magic): when sorted by workflow with nesting, issues render as a numbered plan (`1`, `1.1`, …) with inline "blocked by X · step N" chips — a tree of work reads like a goal-facing checklist. This is a big part of *why* Paperclip feels organized; copy it.
- **A progress strip** on a parent: done/in-progress/blocked counts, a segmented bar, "next up / waiting on blockers," and live cost (tokens + spend) for the subtree.

Cards show: identifier (mono), title, priority icon, assignee avatar, a pulsing **"Live"** dot when an agent is running it, and a "Next step" chip when a successful run needs a disposition.

### 6.1 Board Oversight v1 (IMPLEMENTED)

The Relux-shell **Work** page (`apps/dashboard/src/pages/Work.tsx`) is the operational board today, and it now makes live work **visible and controllable at a glance** — not just a static task list (design §5 Inbox + §11 Active Runs, scaled to the current single-board surface).

**What is visible.**
- **Four board columns** — Open · Running · **Blocked / Failed** · Done. Every `TaskStatus` maps to exactly one column (`apps/dashboard/src/oversight.ts::taskBucket`, unit-tested), so blocked / waiting-on-approval / failed work is now on the board. (Previously the page computed an "other" bucket that was *never rendered* — that work was invisible. Fixed.)
- **An Oversight strip** at the top, fed by one composed read: dense count chips (active runs · open · blocked · failed · waiting-approval · pending approvals), an **In flight** run list, a **Needs attention** (failed/cancelled) run list, the **pending approvals** gate list, and any **resumable Prime continuation**.

**What is controllable** (every control reuses an EXISTING backend route — nothing new executes):
- **Continue** a paused Prime agent loop. The continuation is read from the kernel (not just the live turn), so it **survives a dashboard refresh**; a loop still awaiting a tool approval routes the operator to Approvals first rather than offering a dead Continue.
- **Cancel** an in-flight, process-backed run (the honest `canCancelRun` gate; the kernel reports `not_running` for a non-cancellable run).
- **Retry** a failed/cancelled run that is retryable (`canRetryRun`), jumping to the fresh attempt.
- **Decide a pending approval INLINE.** Each pending-approval row in the strip offers the common low-friction decisions without leaving Work: for a per-call tool invocation, **Approve & run** (`decide(approved)` → `execute` once), **Allow always** (`allow-always` persists a standing grant → `execute` once), and **Deny** (`decide(rejected)`); for a generic approval, **Approve** and **Deny** only (it records the decision but executes nothing, and allow-always does not apply). The applicable set per row is decided by `apps/dashboard/src/approvalactions.ts::approvalInlineActions` (unit-tested), every button drives the SAME `reluxApprovals` route the dedicated Approvals page and the Prime approval card use (no new authority), and after a decision the strip refreshes in place with a compact, shaped result/error (never the raw tool envelope). **Open →** stays on each row as the link to the detailed Approvals audit surface (typed payload, grants, permissions); an approval that can't be decided inline degrades to Open → only with an honest reason.
- **Inspect** any run → the existing Run Detail panel (transcript, logs, proposed-changes, retry/resume/cancel).

**Backend (one small composed route).** `GET /v1/relux/oversight` (`crates/relux-kernel/src/server.rs::get_oversight`) stitches `inspect_state` counts + the in-flight/attention `Run` records (the same shape `list_runs` serves, filtered — not re-projected) + pending approvals (the same `approval_record` shape as `/approvals`) + the resumable continuation handle (`KernelState::current_prime_continuation_handle`, read-only — it grants no authority; resume still flows through the unchanged continue route + gates). It composes existing honest state so the UI does not fan out to four endpoints and glue them poorly. Read-only; mutates nothing. Tests: `oversight_route_composes_counts_runs_approvals_and_continuation` (server), `current_continuation_handle_reads_the_live_record_without_a_token` (state), `apps/dashboard/test/oversight.test.ts` (bucketing + summary helpers), `apps/dashboard/test/approvalactions.test.ts` (the inline action model), `apps/dashboard/test/oversight-approvals-render.test.mjs` (the inline approval controls render per approval shape), the `work-render` strip/column assertions, and a live-browser click check (`apps/dashboard/scripts/browser-smoke.mjs`: the strip loads its composed summary via a real `onClick → network → re-render`, the Blocked/Failed column renders, and the inline approval controls are asserted when a pending approval is present — they are not clicked, since each is a real governed mutation/execution and the smoke clicks only non-destructive surfaces).

**Still pending (not built here, deliberately):** free-form **drag**-to-change-status (a compact Block / Cancel move control is now built — see §6.4) and the list↔board toggle. (Per-subtree live **cost** rollup on the board is now built — see §6.5.) These remain §6/§7 targets. (Inline approve/reject/allow-always *on the strip* is now built — see the inline-decision bullet above; the dedicated Approvals surface remains the detailed audit/grants/permissions home. **Sub-issue nesting / workflow-checklist rendering and per-subtree progress strips are now built — see §6.2.**)

### 6.2 Work hierarchy / progress v1 (IMPLEMENTED)

The board now surfaces **sub-work and progress from real data** (design §6 "A progress strip on a parent: done/in-progress/blocked counts, a segmented bar … waiting on blockers" + §6.1's nesting/workflow-checklist target). A **Work groups** card sits on the Work page (`apps/dashboard/src/pages/Work.tsx`, the `WorkHierarchy` section) between the Oversight strip and the columns.

**The two real hierarchies (no fake grouping).** Two parent→child relationships are recorded today. The first is the multi-agent **orchestration**: each goal's `steps[]` carry the real child `task_id`, the specialist `role`, the durable `outcome`, and `depends_on` (indices into the step array — the genuine dependency / blocked-by / blocking edges). The second is the **ad-hoc `parent_task` edge** (now shipped — see §6.3): a `relux_core::Task` can be a hand-made subtask of another task, outside any orchestration. So the orchestration is one kind of parent, a task with `parent_task` set is the other, and a task in neither is genuinely standalone (a flat card in the columns below, never given a fabricated parent). A planned orchestration with no committed steps is not shown as a parent.

**What it composes (two existing reads, no new route).** `apps/dashboard/src/workhierarchy.ts` (pure, unit-tested) joins `reluxOrchestration.list()` (the structure + dependency edges) to the live `reluxWork.listTasks()` (the current per-child status), so a group's progress reflects **what the board columns actually show** — not a stale single summary field. A child is bucketed by its **live** task status (the same `oversight.ts::taskBucket` the columns use, so the strip and the columns can never disagree); a child absent from the current board view falls back to the durable step `outcome` and the card says so honestly ("Progress is from the orchestration record — these briefs are not on the current board view"). No backend was changed.

**What it shows.**
- **Per-group progress strip** — a compact **segmented bar** (`.seg-bar`, one slice per non-empty board bucket: done / running / blocked / open, painted with each bucket's semantic CSS var — color is meaning-only, §12), the **brief count**, and a `2/5 done · 1 running · 1 blocked` label. Done/in-progress/blocked counts and the segmented bar are exactly the §6 progress-strip spec.
- **Nested numbered workflow checklist** — expandable per group: each child is a numbered row (`1`, `2`, `3` …, the step position) with its title (→ Inspect), the specialist **role** badge, the **live status** badge, the assignee (resolved to a crew name), and the **blocked-by / blocking** dependency chips resolved from `depends_on` to sibling task ids. (Orchestration is a single level today, so the plan numbers `1..N`; deeper `1.1` nesting is not fabricated since the data is flat.)
- **Parent context in the task detail** — when a selected task is a brief inside a group, the Task Detail panel shows a "**part of** &lt;goal&gt;" header, the group's segmented progress, and the full numbered plan with the open task **highlighted** (the child-side of the same relationship).
- **Honest empty / degraded states** — no orchestration with committed steps → "**No sub-work yet** — no multi-agent goal has been decomposed into a grouped plan." A failed orchestration read degrades to an inline "Work groups unavailable — the board below still works" note (excluded from the page error gate, like the Oversight strip), never a blank.

**Tests.** `apps/dashboard/test/workhierarchy.test.ts` pins the join + progress semantics (live status wins over `step.outcome`, the durable-outcome fallback, the four-bucket tally, segment ordering, blocked-by/blocking resolution, `groupForTask`, empty inputs); `apps/dashboard/test/work-hierarchy-render.test.mjs` renders the real `WorkHierarchy` with a seeded parent + children in mixed states and asserts the progress strip, the numbered checklist, the role/live-status badges, the dependency chips, and the empty/error states; `work-render.test.mjs` asserts the section is part of the board's first paint. Reference grounding: Hermes' kanban dashboard computes the same parent progress rollup (`{done, total}` per parent from `task_links` joined to child status, `plugins/kanban/dashboard/plugin_api.py`) and the same blocked-by-status dependency model — recorded in `docs/reference-driven-development.md`.

**Still pending (deliberately):** drag-to-reorder the plan. (Per-subtree **cost** rollup on the board is now built — see §6.5.) These remain §6/§7 targets. (Ad-hoc (non-orchestration) task subtrees are now built — see §6.3.)

### 6.3 Ad-hoc task subtrees v1 (IMPLEMENTED)

The board now supports **hand-made parent/child tasks outside orchestration** (design §6 "sub-issue nesting … a tree, indented" + the §6.2 still-pending "ad-hoc (non-orchestration) task subtrees" target). This is the **second** real parent→child link beside orchestration: an operator can break any task down into subtasks, and the subtree renders with the same progress strip + numbered list the orchestration groups use.

**The real edge (the kernel populates `parent_task`).** `relux_core::Task` already carried an inert `parent_task` field; the kernel now **populates it** through a validated create path. `KernelState::create_task_with_parent` (the no-parent `create_task` is a thin wrapper over it) accepts an optional parent and, before any state change, **validates** it: the parent must exist (else `UnknownTask` → 400), it must share the child's namespace (else `TaskParentScope` → 400 — an ad-hoc subtask lives in its parent's scope, never silently crossing a tenant boundary), and the edge must not close a cycle (else `TaskParentCycle` → 400). The cycle/self-parent rule is the pure, unit-tested `relux_core::would_create_task_cycle` (the same bounded, cycle-guarded parent-pointer walk the org-lattice `hierarchy.rs` uses, applied to the task tree — reference-grounded on Paperclip `agentIsInSubtree` / Hermes `delegate_tool` `MAX_DEPTH`). The cycle guard is defence in depth: a freshly minted id is in no existing subtree, so it never rejects a normal create — it protects this path if it is ever reused to reparent. `POST /v1/relux/tasks` gains an optional `parent_task`; a blank/whitespace value is treated as no parent (a cleared UI field never 400s). The list/get reads need **no** change — `TaskRecord` flattens `Task`, so `parent_task` already serializes on every task row, and the board joins it on the client.

**What it shows.**
- **A Subtasks section on the Task Detail panel** (`apps/dashboard/src/pages/Work.tsx`, the `AdhocSubtaskSection`): the parent's direct children joined from the flat task list by the real `parent_task` edge, rendered as the same **segmented progress strip** (`.seg-bar`, the four board buckets) + **numbered checklist** (`.plan-list`) the orchestration groups use, each row showing the child's title (→ Inspect), its **live** board status badge, and the resolved assignee — so the ad-hoc subtree and an orchestration subtree look identical.
- **An inline "Add subtask" form** on that section: a title field that creates a child via the existing `createTask(title, { parent_task })` (the child auto-assigns to Prime like any task), then reloads the board so the new child appears in the strip **and** the columns. Kept minimal (title only) — the richer assignee/adapter pickers stay on the create surfaces.
- **Board-card markers**: a card shows a `↳ N subtasks` chip when it is a parent and a `↑ subtask of <id>` chip when it is a child (both click through to the relevant task), from the client-side `subtaskCounts` over the flat list. The columns stay flat (a child is still its own card, exactly as orchestration children are), with the tree shown in the detail.
- **Honest empty state**: a task with no children shows "**No sub-work yet** — add a subtask to break this task down," with the Add-subtask form still offered.

**Safety / semantics (this slice).** Parent status is **not** auto-mutated by children — the section shows progress only (no rollup of child completion onto the parent), matching the doc's "progress strip" scope. Deleting/closing a parent is unchanged (there is no task-delete surface today). The subtree is a single level in the UI; the kernel edge supports deeper chains and the cycle guard is depth-bounded, but the panel renders direct children only.

**Tests.** `crates/relux-core/src/task.rs` unit tests pin the pure helpers (ancestors walk nearest-first and stop at the root; proper-descendant subtree membership; self/direct/transitive cycle rejection; total under a cyclic map; depth cap). `crates/relux-kernel/src/state.rs`: a valid child persists its parent **through a snapshot roundtrip**, a missing parent is a clean `UnknownTask` (nothing created), a cross-namespace parent is `TaskParentScope`, and a standalone create leaves `parent_task` unset. `crates/relux-kernel/src/server.rs`: `POST /tasks` with `parent_task` links the child and the link is visible on the list read; an unknown parent is a 400; a blank parent is a standalone task. Frontend: `apps/dashboard/test/adhocsubtrees.test.ts` pins the pure join (direct children only, id order, bucket mapping, the four-bucket tally, `parentTaskIds`/`subtaskCounts`, empty inputs); `apps/dashboard/test/work-adhoc-subtree-render.test.mjs` renders the real `AdhocSubtaskSection` with a seeded parent + children in mixed states and asserts the progress strip, the numbered list, the live-status badges, the resolved assignees, the Add-subtask form, and the honest empty state. Reference grounding (the org-lattice bounded parent walk reused for the task tree) is recorded in `docs/reference-driven-development.md`.

**Still pending (deliberately):** free-form drag-to-reorder a subtask; auto-rolling child progress onto the parent's status. (Per-subtree **cost** rollup is now built — see §6.5. Safe **reparent** of a subtask — move it under a new parent or detach it — is now built as a selection control; see §6.6.) These remain §6/§7 targets.

### 6.4 Work board status movement v1 (IMPLEMENTED)

The board can now **move a task between statuses** from the card and the detail panel (design §6 "Drag a card to a column → status mutation, **with transition validation**; an invalid drop shows a toast"). This is the first board *interaction* on top of the read/oversight surfaces — deliberately a reliable **compact move control** built on existing, validated lifecycle rules, not yet free-form drag-and-drop.

**The one safe rule, reused — not re-invented.** The kernel already had exactly one operator-settable task-status mutation: the conversational by-id `UpdateTask` path, whose allowlist (`crates/relux-kernel/src/prime_update_slots.rs::SETTABLE_STATUSES` — `blocked` / `cancelled`) and terminal-state guard (`is_terminal_status`) mean an operator may **block** or **cancel** a task they own, but never decree `running` / `completed` / `failed` (those are machine-driven by the run lifecycle) and never edit a **finished** task. The board move surfaces that SAME mutation — it is not a parallel, looser path:

- **Backend — `POST /v1/relux/tasks/:id/status` (`crates/relux-kernel/src/server.rs::set_task_status_route`).** Session-gated like the assign/start routes. The body `status` is parsed through the SAME `parse_settable_status` allowlist, so a machine-driven / unknown target is an honest **400** naming the allowed set. It calls the new `KernelState::set_task_status`, which **re-checks** the allowlist (defense in depth), refuses a **terminal** task with a **409** (`TaskTerminalStatus`), an unknown task with a **400** (`UnknownTask`), and on success moves the task, advances `updated_at`, and audits **`task:update`** — the same audit row the chat update writes. It touches **no** run, **no** assignee, and rolls **no** child status onto a parent.
- **UI — a compact `StatusMoveControl`** (`apps/dashboard/src/pages/Work.tsx`) on every board **card** and in the **Task Detail** panel: a small design-system `select` offering ONLY the moves `apps/dashboard/src/taskmove.ts::operatorStatusMoves` allows for the task's **live** status (Block / Cancel, the current status excluded). It mirrors the backend allowlist + terminal guard, so the menu never offers a move the route would reject, and it renders **nothing** for a finished task (no dead affordance). On success it reloads the board so the card **re-buckets** into its new column (`oversight.ts::taskBucket` — a blocked task lands in **Blocked / Failed**, a cancelled task in **Done**) and any parent progress strip updates on refresh; the detail control also re-fetches the panel so the shown status is live. A rejected move (state changed underneath) surfaces the **honest backend reason inline**, never a silent no-op.

**Safety / semantics (this slice).** Run and approval semantics are untouched — the move never marks running work *done* (`completed` is not offered) and never cancels the underlying *run* (a process-backed run is still cancelled via the Run Detail `Cancel`, `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8). There is **no auto status rollup** from children onto a parent (that remains a §6 future target). Child / subtask cards move **independently** (each is its own card, exactly as orchestration children are), and the parent's progress strip reflects the change after refresh.

**Tests.** Backend: `crates/relux-kernel/src/state.rs::set_task_status_moves_to_settable_and_guards_the_rest` (a settable move applies + survives a snapshot roundtrip; a machine-driven target / a terminal task / an unknown task each fail closed; both moves audited) and `crates/relux-kernel/src/server.rs::set_task_status_route_moves_settable_and_rejects_the_rest` (200 block, 400 machine-driven, 409 terminal, 400 unknown, `task:update` audited) + `..._requires_a_session` (the route is session-gated). Frontend: `apps/dashboard/test/taskmove.test.ts` pins the pure helper (the offered moves equal the backend allowlist minus the current status; no machine-driven target is ever offered; a terminal task offers none); `apps/dashboard/test/task-move-render.test.mjs` renders the real `StatusMoveControl` (Block + Cancel for a non-terminal task, Cancel-only for a blocked task, nothing for a terminal task); and the live-browser smoke (`apps/dashboard/scripts/browser-smoke.mjs`) drives the seeded card's move select to **blocked** and asserts the card re-buckets — the real `onChange → network → re-render` binding the static render test cannot see (this move IS clicked, unlike the oversight approval controls, because it is a SAFE in-scope edit on a throwaway DB, never a governed/risk-gated action).

**Still pending (deliberately):** keyboard drag/reorder; **reorder** a subtask within a subtree; and a board move to a *non*-settable lane (re-open a blocked task to running) — re-opening is a run-lifecycle action, not a status decree, so it stays out of this slice. (Free-form **drag-and-drop** — drag a card to a column — is now built additively over this select; see §6.7. Safe **reparent** of a subtask is now built as a selection control — see §6.6.) These remain §6/§7 targets.

### 6.5 Per-subtree run/cost rollup v1 (IMPLEMENTED)

The board now surfaces a **per-subtree run / cost rollup** (design §6 "A progress strip on a parent: … and live **cost (tokens + spend)** for the subtree"). Every parent group (an orchestration goal — §6.2) and every ad-hoc subtree (§6.3), plus the parent context in the Task Detail panel, now shows a compact **rollup chip strip** beside its progress bar: how many runs the subtree's work has spawned, the active / failed counts, and — **where the data exists** — the real cost, measured duration, and tokens.

**The data is all real and frequently absent — and the rollup is honest about it.** A `relux_core::Run` (`crates/relux-core/src/run.rs`) carries `task_id`, a lifecycle `status`, and three OPTIONAL measured fields the adapter only reports when it emitted a machine-readable result envelope (`crates/relux-core/src/adapter_result.rs` parses the Claude-style `{ total_cost_usd, duration_ms, usage }`):
- **`cost`** (USD) — only when the envelope carried `total_cost_usd`. The deterministic local-echo path and plain-text adapters (Codex `exec`, a generic command) report **no** cost.
- **`duration_ms`** — the **real** wall-clock of the adapter subprocess, captured in the one place a real process is touched. Absent for the local-echo path (which spawns no process).
- **`usage`** — the token object, only from a structured envelope.

So for many runs cost/duration/tokens are simply **unknown**, and the rollup never fabricates them: it sums **only** the runs that reported each figure, tracks the coverage count, and when **no** run in a subtree reported a cost it shows **"cost unavailable"** (with a tooltip explaining why) — *not* a fake `$0.00`. A genuine reported cost of exactly `$0.00` is distinct from "unavailable" and is shown as a real figure. Partial coverage ("`$0.0150` from 1/2 runs") is disclosed in the chip tooltip. A subtree whose tasks have never run shows a single **"no runs yet"** chip, so the strip is never silently blank where work exists. **Run statuses** bucket into active (pending / running / waiting-for-approval), done (completed), and failed (failed **or cancelled** — a cancelled *run* did not complete its work, unlike the task board where a cancelled *task* lands in Done).

**No new route — a pure client join.** All run data is already served by `reluxWork.listRuns()` (every `ReluxRun` flattens `task_id` + the optional `cost`/`duration_ms`/`usage`), and the Work page already loads it for the Recent Runs table. So, exactly like §6.2/§6.3, this is a **pure client-side join** (`apps/dashboard/src/runrollup.ts`, dependency-free, unit-tested): `rollupRuns(runs, taskIds)` sums the runs whose `task_id` is in a subtree, and `runRollupChips(rollup)` shapes the honest chip strip. An orchestration group rolls up its child tasks' runs; an ad-hoc subtree rolls up the **parent task plus its direct children** (`adhocSubtreeTaskIds`). The chips reuse the B&W badge vocabulary (color semantic-only — §12); **Run Detail remains the source of full logs** — each chip is a glance signal, and the numbered checklist's per-row **Inspect** is the drill-down to the related run/task. No backend was changed.

**Tests.** `apps/dashboard/test/runrollup.test.ts` pins the pure semantics (subtree membership filter; the three-bucket tally with cancelled→failed; cost summed only over reporting runs with coverage tracking; **cost unavailable ≠ a reported $0.00**; real-duration and token sums; partial-coverage disclosure; the formatters; empty subtree). `apps/dashboard/test/work-run-rollup-render.test.mjs` renders the real `RunRollupChips` and asserts a real summed cost + token chip when reported, an honest **"cost unavailable"** chip (and **no** fabricated `$`) when none reported, and the "no runs yet" chip for an empty subtree. The §6.2 (`work-hierarchy-render`) and §6.3 (`work-adhoc-subtree-render`) render tests now seed runs and assert the rollup strip renders on the group card and the ad-hoc subtree from real run data, plus the honest "cost unavailable" path.

**Still pending (deliberately):** a dedicated **Costs** surface (design §10 — spend by company/agent/project with budgets and provider/biller breakdowns) and per-run **token detail** beyond the summed count; surfacing **logical** start/end timing as wall-clock (the kernel's `started_at`/`ended_at` are a deterministic logical clock, not real instants, so only the measured `duration_ms` is summed here — never the logical stamps). These remain §10 targets.

### 6.6 Safe task reparenting v1 (IMPLEMENTED)

The board can now **change a task's place in the ad-hoc tree** — move it under a new parent, or detach it to a top-level task — from the Task Detail panel (design §6 "sub-issue nesting … a tree, indented" + the §6.3/§6.4 still-pending "**reparent** / reorder a subtask" target). This is the second structural board *interaction* (after the status move, §6.4), and like that slice it is a deliberately reliable **selection control**, not yet free-form drag-and-drop. It reuses the EXACT validation the ad-hoc create path (§6.3) already enforces — the board reparent can never be looser than create.

**The cycle guard was already there — this slice exposes it.** §6.3 built the pure, unit-tested `relux_core::would_create_task_cycle` (the bounded, cycle-guarded parent-pointer walk the org-lattice `hierarchy.rs` uses, applied to the task tree) and noted the guard "already covers reparenting safety … it protects this path if it is ever reused to reparent." This slice is that reuse:

- **Backend — `POST /v1/relux/tasks/:id/parent` (`crates/relux-kernel/src/server.rs::reparent_task_route`).** Session-gated like the assign/start/status routes. The body `parent_task` is the new parent id, or `null` / a **blank** string to **clear** the edge (a cleared UI field never 400s — the same forgiving handling create gives a blank parent). It calls the new `KernelState::reparent_task`, which mirrors the in-kernel reparent precedent the org-lattice uses for an operative's **Lead** (`update_agent_with_skills` `reports_to`): it **validates a set parent before mutating anything** — the task and a set parent must **exist** (`UnknownTask` → 400), the parent must share the child's **namespace** (`TaskParentScope` → 400 — a subtask lives in its parent's scope, never crossing a tenant boundary), and the edge must not close a **cycle**, including a self-parent (`TaskParentCycle` → 400, via `would_create_task_cycle` over the live task tree). It is **STRUCTURAL ONLY**: it touches `parent_task` and `updated_at` and **nothing else** — the task's status, assigned agent, and any run are left exactly as they were (moving a task in the tree never re-runs or re-buckets it). On success it audits **`task:update`** — the same row the chat update and the §6.4 board move write.
- **UI — a compact `ReparentControl`** (`apps/dashboard/src/pages/Work.tsx`) on a new **Parent** row in the **Task Detail** panel: a small design-system `select` ("Move under…") offering ONLY the candidate parents `apps/dashboard/src/reparent.ts::candidateParents` allows, plus a **Remove parent** button shown only when the task has one. The candidate list mirrors the backend safety client-side — it **excludes self, every descendant** (a JS port of the same bounded subtree walk, so a move that would close a cycle is never even offered), the **current parent** (a no-op), and any **cross-namespace** task. When **nothing** qualifies it says so honestly ("No other task can be its parent.") rather than presenting an empty control. On success it reloads both the panel and the board so the card re-nests and any parent progress strip / subtree rollup updates; a rejected move (state changed underneath) surfaces the **honest backend reason inline**, never a silent no-op.

**Tests.** Backend: `crates/relux-kernel/src/state.rs` — `reparent_sets_and_clears_the_parent_and_persists` (a valid move sets the edge + survives a snapshot roundtrip, the moved task's status is untouched, then a clear detaches it and also persists), `reparent_rejects_a_missing_parent_and_unknown_task` (a missing parent **and** a missing child each fail closed, nothing mutated), `reparent_rejects_cross_namespace_self_and_cycles` (a cross-namespace parent is `TaskParentScope`; a self-parent and a transitive A→B→A loop are each `TaskParentCycle`); and `crates/relux-kernel/src/server.rs::reparent_task_route_moves_clears_and_rejects_cycles` (200 move with the edge visible on the returned task, 400 self-parent, 400 transitive cycle, 200 blank-clears-the-edge, 400 unknown task). Frontend: `apps/dashboard/test/reparent.test.ts` pins the pure helpers (`taskDescendants` is the transitive subtree; `candidateParents` excludes self + descendants + current parent + cross-namespace, is empty when nothing safe exists, and is in stable id order); `apps/dashboard/test/work-reparent-render.test.mjs` renders the real `ReparentControl` and asserts the safe-candidate options (no descendant, no current parent, no cross-namespace task is offered), the Remove-parent affordance only for a parented task, and the honest empty state with no selector when nothing can be its parent. Reference grounding (the org-lattice `reports_to` reparent reused for the task tree) is recorded in `docs/reference-driven-development.md`.

**Still pending (deliberately):** free-form **drag-and-drop** reparent/reorder and keyboard drag; **reordering** siblings within a subtree (the `parent_task` edge has no sibling order today — children render in stable id order); and auto-rolling child progress onto the parent's status. (Drag-to-column **status** movement is now built — see §6.7.) These remain §6/§7 targets.

### 6.7 Drag-to-column status movement v1 (IMPLEMENTED)

The board now supports **dragging a card to a column** to change its status (design §6 "Drag a card to a column → status mutation, **with transition validation**; an invalid drop shows a toast"). This is the §6.4 "still pending free-form drag-and-drop" target, built **additively on top of** — never replacing — the §6.4 `StatusMoveControl` select. Drag is the pointer affordance; the select remains the keyboard/accessibility path, so the board is operable both ways.

**Drag resolves to the SAME one safe rule — it is not a looser path.** The drop does not invent a new mutation: it resolves the *target column* to exactly the operator-settable status the §6.4 select would offer, then calls the SAME `POST /v1/relux/tasks/:id/status` route (`crates/relux-kernel/src/server.rs::set_task_status_route`, the `blocked`/`cancelled` allowlist + terminal guard). No backend change was needed.

- **Column → status mapping (`apps/dashboard/src/taskmove.ts::columnDropTarget`, pure + unit-tested).** Only two of the four columns map to an operator-settable status: **Blocked / Failed → `blocked`** (Block) and **Done → `cancelled`** (Cancel — the one settable terminal). The **Open** and **Running** columns are *machine-driven lanes* (re-opening / running is a run-lifecycle action, §6.4), so a drop there is **rejected with an honest reason**, never silently applied. A drop on the column a card already occupies (e.g. a blocked card onto Blocked) is a rejected no-op ("already in this column"); a **terminal** card is rejected from every column ("this task is finished and can't be moved"). The resolver mirrors `operatorStatusMoves` exactly (defence in depth), so a drop the route would 4xx is never even attempted — and every rejection is surfaced **inline on the column**, dismissible, not a swallow.
- **The drag payload is a private, fail-closed envelope.** Cards travel under a custom MIME (`application/x-relux-task`, mirroring the Hermes kanban reference's `text/x-hermes-task`), carrying the card's id + live status (`encodeTaskDrag` / `parseTaskDrag`). A foreign drop (text, a file, anything not our JSON) decodes to `null` and is **ignored**, never throwing — the column reacts only to its own task drags. The dragged status is read at the **drop** site (HTML5 `dataTransfer` data is unreadable mid-`dragover`), which is why the payload carries it.
- **UI (`apps/dashboard/src/pages/Work.tsx`).** Every **non-terminal** card is `draggable` with a grab cursor, a drag-handle glyph, and an `aria-roledescription="draggable task card"`; a **terminal** card is **not** draggable (no dead affordance, matching the §6.4 select which renders nothing for a finished task). Each `Column` is a labelled drop region (`role="list"`, `aria-label="… column — drop a task here to move it"`, `data-bucket`) that highlights with a dashed outline on a valid drag-over (B&W only — §12), applies the move on a valid drop and reloads so the card **re-buckets**, or shows the inline rejection reason. The native HTML5 DnD API is used directly — **no drag-and-drop dependency** was added.

**Tests.** Frontend: `apps/dashboard/test/taskmove.test.ts` extends the pure helper coverage — `columnDropTarget` resolves Blocked→block / Done→cancel for every non-terminal status, rejects the Open/Running machine-driven lanes (the Running reason naming the run lifecycle), rejects an already-in-column no-op and every terminal card, never resolves a move `operatorStatusMoves` would not offer, and the `encodeTaskDrag`/`parseTaskDrag` payload round-trips while a foreign/malformed payload decodes to `null` (never throws). `apps/dashboard/test/work-drag-render.test.mjs` renders the real `Column` and asserts the labelled drop region (class + `data-bucket` + `aria-label`), a draggable non-terminal card with its drag handle/role, a **non**-draggable terminal card, and the keyboard select still present alongside (drag is additive). The live-browser smoke (`apps/dashboard/scripts/browser-smoke.mjs`) adds a deterministic assertion that the seeded card is `draggable` and the board column is a labelled drop target — native drag isn't reliably synthesizable via CDP, so the drop→`setTaskStatus` binding stays pinned by the pure + backend route tests, while §6.4's select smoke already exercises the live move→reload edge.

**Still pending (deliberately):** free-form drag-to-**reparent** and sibling **reorder** (the §6.6 structural moves stay selection-controls; the `parent_task` edge has no sibling order); and a richer drag ghost / inter-card insertion indicator. (A clear **keyboard-accessible** path for the status move now ships — see §6.8; a pointer-free pick-up/drop *re-implementation* of native drag stays out of scope as brittle, since the labelled select already gives non-pointer users the full move. Re-opening a blocked task — a board move to a *non*-settable lane — is now built as a run-**lifecycle** action, not a status/drag mutation; see §6.9.) These remain §6/§7 targets.

### 6.8 Keyboard-accessible board movement v1 (IMPLEMENTED)

The board's status move is now **clearly keyboard- and screen-reader-accessible** (design §6 "status (clickable to change)"; the §6.7 still-pending "keyboard drag — today keyboard users use the §6.4 select, which is the accessible equivalent"). §6.7's free-form drag is a *pointer* affordance; this slice makes the **select** — the non-pointer path — a self-describing control instead of a tiny unlabelled "Move…" menu. It is **additive**: no new mutation, the same `operatorStatusMoves` allowlist + `POST /v1/relux/tasks/:id/status` route; only the control's accessibility is enriched. A pointer-free re-implementation of native HTML5 drag (pick-up/drop with the keyboard) was **deliberately not** built — it is brittle across browsers/AT, and the labelled select already gives a keyboard user the entire move.

**The words come from the SAME allowlist the control offers — they can never disagree.** A new pure, unit-tested helper `apps/dashboard/src/taskmove.ts::statusMoveGuidance(status)` derives, from `operatorStatusMoves(status)`, an accessible description of the moves: a **descriptive `aria-label`** (`"Move task status — Block to hold this task, Cancel to stop it"`, trimmed to only the offered verbs — a blocked task announces just "Cancel to stop it"), **visible helper text** spelling out the Block/Cancel semantics **and** why the Open/Running lanes aren't settable (`"Block holds the task; Cancel stops it. Open and Running are set by the run lifecycle, not by a board move."`), and — for a **finished** task that offers no move — an honest reason (`"This task is finished and can't be moved."`, word-for-word the `columnDropTarget` terminal reason) rather than an empty label. Because the guidance is computed from the offered moves, it never describes a move the select doesn't show.

- **UI — `apps/dashboard/src/pages/Work.tsx::StatusMoveControl`.** The select now carries the descriptive `aria-label` (not a bare "Move…") and an **`aria-describedby`** pointing to a **visible** helper line (`id="status-move-help-<taskId>"`) that renders the Block/Cancel-plus-machine-lanes explanation — so a screen reader announces the move semantics and a sighted keyboard user reads them inline. A new `showUnsupportedNote` prop (set on the **Task Detail** panel, left off on board **cards**) makes a finished task render a clear `role="note"` line stating why it can't move, **instead of nothing** — so a keyboard user in the detail panel learns *why* there's no control, while a board card stays a no-dead-affordance compact card (§6.4). The drag affordance (§6.7) and the option set are unchanged.

**Tests.** Frontend: `apps/dashboard/test/taskmove.test.ts` pins `statusMoveGuidance` (a non-terminal task names **both** Block and Cancel + the run-lifecycle lanes; a blocked task describes **only** Cancel and drops Block; a finished task reports `canMove:false`, an **empty** `ariaLabel`, and the honest "can't be moved" reason; and **every** offered move has human guidance — a guard against a future settable status with no description). `apps/dashboard/test/task-move-render.test.mjs` renders the real `StatusMoveControl` and asserts the descriptive `aria-label`, the `aria-describedby`↔helper-`id` wiring, the visible Block/Cancel + run-lifecycle helper text, that a board card still shows **no** control for a finished task, and that the detail variant (`showUnsupportedNote`) surfaces the `role="note"` reason for a terminal task while a movable task still renders the real select. `apps/dashboard/test/work-drag-render.test.mjs` updates the "select stays alongside drag" assertion to the new described label + `aria-describedby` wiring (drag stays additive over the now-described keyboard path). Dashboard `tsc --noEmit` clean and `npm run build` green.

**Still pending (deliberately):** a true pointer-free **pick-up/drop** keyboard re-implementation of native drag (out of scope as brittle — the labelled select is the accessible equivalent); and the §6.7 free-form drag-to-**reparent** / sibling **reorder**. (A board move to a *non*-settable lane — re-open a blocked task — is now built as the run-**lifecycle** action it always was, see §6.9.) These remain §6/§7 targets.

### 6.9 Reopen blocked work via a lifecycle action v1 (IMPLEMENTED)

The board can now **reopen a blocked task** — put held work back into the run lifecycle so its assigned operative can run it again — as an explicit **run-lifecycle action**, closing the gap §6.4/§6.7/§6.8 called out: "*re-opening a blocked task to running is a run-lifecycle action, not a status decree*." The board status allowlist (§6.4) deliberately offers ONLY Block / Cancel — the machine-driven lanes (Open / Running) are set by the run lifecycle, never decreed from the board — which left a blocked task a **dead-end** on the board (only Cancel or Inspect). Reopen is the safe inverse of the operator **Block** move, and because it must reach a machine-driven status it cannot be a status set: it is its own validated action.

**One safe rule, validated — not a looser status path.** A blocked task is **re-queued** (`Blocked` → `Queued`, the normal pre-run state), after which the existing **Run (Assigned)** path runs it through the unchanged run gate. Reopen never marks a task `running` itself, never touches a run, and never auto-executes — it makes held work *runnable* again; running it stays the operator's explicit next action.

- **Backend — `POST /v1/relux/tasks/:id/reopen` (`crates/relux-kernel/src/server.rs::reopen_task_route`).** Session-gated like the assign/start/status/parent routes. It calls the new `KernelState::reopen_task`, which **validates eligibility before any mutation** (fail-closed): the task must **exist** (`UnknownTask` → 400); it must currently be **`Blocked`** — a terminal/running/queued/created task is **not** reopenable (`TaskNotReopenable` → **409**, naming the live status; reopening is the inverse of Block, not a generic set); and it must have an **assigned agent** (`TaskNotAssigned` → 400 — a run needs an assignee, so reopening unassigned work would only dead-end at the run gate). On success it moves the task `Blocked` → `Queued`, advances `updated_at`, and audits **`task:update`** (the same row the §6.4 board move writes, tagged `reopen`). It touches **no** run and rolls **no** child status onto a parent. A task **waiting on an approval** is in `WaitingForApproval`, not `Blocked`, so it never reaches this path — pending approvals/continuations route to the **Approvals** surface / the Oversight strip (§6.1), not a raw resume.
- **UI — a compact `ReopenControl`** (`apps/dashboard/src/pages/Work.tsx`) on every board **card** and in the **Task Detail** panel: a small **Reopen** button shown ONLY for a blocked task that `apps/dashboard/src/taskmove.ts::reopenEligibility` finds eligible (blocked **and** assigned) — it mirrors the kernel guard, so the button never appears where the route would reject. A blocked task with **no assignee** renders **nothing** on a card (no dead affordance, matching §6.4/§6.8), and in the Task Detail panel renders the **honest reason** as a `role="note"` line ("Assign an operative before reopening — a run needs an assignee."), so a keyboard / screen-reader user learns *why* there is no button. A **non-blocked** task gets no reopen affordance at all. On success it reloads the board so the card **re-buckets** (Blocked → Open, where the existing **Run (Assigned)** action then runs it) and the panel re-fetches; a rejected reopen (state changed underneath) surfaces the **honest backend reason inline**, never a silent no-op. The same control also offers a second **Reopen & run** button (below) for the same eligible work, so the operator can chain the two steps in one click.

**One-click Reopen & run (IMPLEMENTED).** Closing this section's first pending item: a **`POST /v1/relux/tasks/:id/reopen-and-run`** route (`crates/relux-kernel/src/server.rs::reopen_and_run_task`) chains the re-queue into the existing assigned-run path in **one governed call** — reusing **both** existing chokepoints **verbatim** (`KernelState::reopen_task` for eligibility, then `KernelState::execute_assigned_run` for the run): the **same** eligibility guard, the **same** run gate, **no** auto-approval and **no** bypass flag. It is atomic-ish and honest about staging: (1) an **ineligible** reopen (unknown / non-blocked / unassigned) `?`-errors out of the reopen guard **before any run** is attempted — the standalone 4xx (409 / 400), never a run; (2) when the reopen **succeeds but the run is refused** (adapter not configured/disabled, unknown agent, …) the route returns **200** with `reopened: true`, `run_id: null`, and the honest `run_refused` message (the **same** string the standalone run path returns) — the **reopened state is preserved** (the task has left `Blocked`), never conflated with an ineligibility error; (3) when both succeed, **200** with `reopened: true` and the new `run_id`. Persistence uses `locked_save_persisting`, so the re-queue **and** any failed-run transcript survive in both 200 cases (matching the assigned-run path). Session-gated like every board mutation. The UI's **Reopen & run** button surfaces a refusal inline as a `role="note"` ("Reopened, but the run was refused: …") and reloads the board in every outcome; an ineligible call throws the real backend reason like plain Reopen.

**Safety / semantics (this slice).** Reopen is **status-structural only** in the sense that it changes just the lifecycle status (`Blocked` → `Queued`) + `updated_at`; the assigned agent, any prior run, and approvals are untouched. It does **not** auto-run (running stays the operator's explicit next action through the unchanged run gate, `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8), and it does **not** apply to terminal work — a **cancelled** task is terminal and not reopenable; re-attempting a **failed run** stays the run-level **retry** (`canRetryRun`, §6.1), and continuing a **resumable** Claude session stays `POST /runs/:id/resume`, both unchanged. Reopen targets the *task*, those target a *run*.

**Tests.** Backend: `crates/relux-kernel/src/state.rs::reopen_task_requeues_a_blocked_task_and_guards_the_rest` (a blocked task re-queues to `Queued` + survives a snapshot roundtrip with its assignee preserved and audited as `task:update`; a queued/cancelled task is `TaskNotReopenable`; a blocked task with no assignee is `TaskNotAssigned`; an unknown task is `UnknownTask` — each failing closed with nothing mutated) and `crates/relux-kernel/src/server.rs::reopen_task_route_requeues_blocked_and_rejects_the_rest` (200 reopen with `queued` + the assignee on the returned task, 409 for a queued and a cancelled task, 400 unknown, `task:update` audited) + `..._requires_a_session` (the route is session-gated). Frontend: `apps/dashboard/test/taskmove.test.ts` pins `reopenEligibility` (a blocked + assigned task is eligible; a blocked + unassigned task is applicable-but-ineligible with the honest reason; every non-blocked status — including the machine-driven and terminal ones, and `waiting_for_approval` which routes to Approvals — is not applicable, so the control never appears); `apps/dashboard/test/work-reopen-render.test.mjs` renders the real `ReopenControl` and asserts the Reopen button for a blocked + assigned task, **nothing** on a card for a blocked + unassigned task, the `role="note"` reason in the detail panel for that case, and **nothing** (no button, no note) for every non-blocked status on both card and detail. **Reopen & run** adds: backend `crates/relux-kernel/src/server.rs::reopen_and_run_route_reopens_then_runs_for_local_prime` (a blocked Prime-assigned task re-queues **and** runs → 200 with `reopened: true` + a real `run_id`, no refusal, task left `Blocked`), `..._preserves_reopen_when_run_refused` (a blocked task on an **unconfigured** CLI adapter → 200 with `reopened: true`, `run_id: null`, an honest `run_refused`, and the task no longer `Blocked` — the reopen preserved), `..._rejects_ineligible_before_running` (a queued task is 409 and **no run is created**, an unknown task is 400), and `..._requires_a_session` (session-gated). Frontend `apps/dashboard/test/work-reopen-render.test.mjs` also asserts the **Reopen & run** button renders for a blocked + assigned task with its no-bypass title. Dashboard `tsc --noEmit` clean and `npm run build` green.

**Still pending (deliberately):** auto-rolling a reopened child's status onto its parent. (The one-click **Reopen & run** that chains the re-queue into the assigned-run path is now built — see *One-click Reopen & run* above — reusing both the eligibility guard and the run gate, no bypass. Surfacing a blocked task's **root cause** + the recovery decision cards is now built — see §6.10 below.) This remains a §5/§6/§7 target.

### 6.10 Recovery decision cards v1 (IMPLEMENTED)

Closing §6.9's remaining gap: the board now turns a **failed run** or a **blocked task** into a plain-language **recovery decision card** — `relix-execution-and-issue-design.md` §3.3b's "diagnose the problem, explain it to the operator in plain language, and offer clean choices." The card is a **read-only recommendation built deterministically from data the kernel already recorded** — never a fabricated AI guess: a run's structured `failure_class` + retry/session state (§3.3b Stage 1 classification, which the kernel already computes), or a blocked task's reopen eligibility + its latest failed run's diagnosis. Every offered action is backed by an **existing route**; when the data offers no safe action, the card says **what information is missing** instead of inventing one.

- **Pure model — `apps/dashboard/src/recovery.ts` (`assessRunRecovery` / `assessTaskRecovery` / `latestRunForTask`).** `assessRunRecovery(run)` classifies a failed/cancelled run into a plain-language **root cause** + **recommendation** + ordered **actions**, keyed off the kernel's `failure_class`: *adapter not available* → configure the adapter then retry; *authentication required* → fix the credential (Settings) then retry; *permission denied* → grant the permission (Crew) or reassign; *transient / timed out* → the §3.3b auto-retry lane (says whether a bounded retry is scheduled / due / exhausted, offers Retry-now); *invalid request* / *output validation* → inspect then retry; *cancelled* → a terminal non-error, offer a fresh run; *unknown / unclassified* → inspect the transcript/logs, with an honest `missingInfo` ("no structured failure class was recorded"). A resumable run also offers **Resume session**. `assessTaskRecovery(task, latestRun)` fires only for a **blocked** task: it recommends the §6.9 reopen path (**Reopen & run** / **Reopen**) plus **Reassign**, and folds the task's **latest failed run's** root cause into the explanation ("Its last run stalled: …"); a blocked task with **no assignee** mirrors the kernel reopen guard — its only action is **Assign operative**, with the honest `missingInfo` reason. The model is pure (no clock/network/DOM) and unit-tested.
- **UI — a compact `RecoveryCard` (`apps/dashboard/src/pages/Work.tsx`).** Rendered in the **Run Detail** panel for a failed/cancelled run and in the **Task Detail** panel for a blocked task. It shows the **Recovery** eyebrow + a class badge (the shared B&W `failureClassTone` vocabulary), the **root cause**, the **recommendation**, and the action choices. Each action renders as the right affordance for that surface: a **wired button** (Retry / Resume reuse the Run Detail handlers; Reopen / Reopen & run reuse the §6.9 routes), a **navigation link** to an existing page (Configure → Crew / Settings; Reassign-from-a-run → the task on the board), a **reassign picker** (a `<select>` of operatives driving the existing `POST /v1/relux/tasks/:id/assign` route — which reassigns **and** re-queues, the §3.3b operator reassign), or — for an action the surface can't wire (e.g. Inspect, where the transcript is already on the panel) — a **muted pointer** showing the hint, **never a dead button and never invented authority**. A run-refusal or error from a wired action surfaces **inline**; every success reloads the board + panel.
- **No new authority / no new run system.** The card composes **existing** routes only — retry (`/runs/:id/retry`), resume (`/runs/:id/resume`), reopen + reopen-and-run (`/tasks/:id/reopen[-and-run]`), assign (`/tasks/:id/assign`) — and existing pages (Crew, Settings, Approvals, the board). It adds **no** backend route, no auto-retry/auto-reassign, and no bypass: it is a *legible face* on machinery that already exists (§3.3b's "the operator clicks; the choice drives the next step").
- **Investigate with Prime — `apps/dashboard/src/investigateseed.ts` (NEW).** Closing §3.3b's "Investigate → chat companion pre-loaded with the diagnosis" choice: every recovery card now ends with a non-primary **Investigate with Prime** action (appended centrally by `recovery.ts`'s `withInvestigate`, so it never steals the recommended first action). Clicking it builds a **safe, bounded, redacted investigation seed** — the task id/title/status/assignee, the run id/status/`failure_class`/failure text, the deterministic root cause + recommendation, and (on the Run Detail panel, where it is already held client-side) the most-recent lines of the **bounded, server-redacted run-log tail** — and frames it as a **debugging question that explicitly instructs Prime not to create tasks, start runs, change status, or run any tools**. `buildInvestigationSeed` clamps each field and the log tail (`MAX_FIELD_CHARS` / `MAX_LOG_TAIL_CHARS`) and applies a defensive client-side `redactSecrets` pass (api keys / bearer tokens / `key=value` secrets) before anything leaves for the chat. The seed is handed off through a **one-shot sessionStorage entry** (`stashInvestigationSeed` on click → navigate to `/prime`); the **Prime page consumes it exactly once on mount** (`consumeInvestigationSeed` reads **and removes** it, guarded by a ref against a StrictMode double-invoke) and sends it as the first user message, so Prime answers it as a normal grounded **"answered"** turn — a Hermes-style debugging partner — and materializes nothing. No seed pending → normal Prime chat, untouched. **No new backend route:** the seed is built from data already on the client, and Prime fetches any deeper transcript/logs through its **existing read-only context tools**.

**Tests.** Pure classifier — `apps/dashboard/test/recovery.test.ts` pins each `failure_class` case (adapter / auth / permission / transient-scheduled / transient-exhausted / invalid / output-validation / cancelled / unknown), the retry-eligibility gating (no Retry when not retryable), the resumable add-on, the blocked-task cases (assigned → reopen+reassign, unassigned → assign-first + honest missing-info, last-failed-run root-cause fold), the non-blocked → null rule, `latestRunForTask`, and the **appended non-primary Investigate action** on every card (never substituted, never forced onto a null assessment). Seed builder/handoff — `apps/dashboard/test/investigateseed.test.ts` pins the seed CONTENT (read-only framing, the right task/run/diagnosis fields, no fabricated run when none failed), the redaction (api keys / bearer tokens / `key=value`, prose preserved) and bounding (log tail clamped to the most-recent chars), and the **consume-once** storage semantics (read-then-remove; a second consume / no-seed → null so normal chat is untouched). Render — `apps/dashboard/test/work-recovery-render.test.mjs` + `apps/dashboard/test/work-investigate-render.test.mjs` drive the real `RecoveryCard` and assert the eyebrow/root-cause/recommendation, a wired button, a navigation link, the muted pointer for an unwired action, the `role="note"` missing-info line, the reassign picker (current operative marked), and Investigate rendering as a wired button vs. a muted pointer. Dashboard `tsc --noEmit` clean, `npm test` green (507), `npm run build` green.

**Diagnostic narrative pass (the §3.3b cheap diagnostic LLM pass — now built).** Closing this section's first pending item: every recovery card now ends with an **Analyze failure** choice — an explicit, operator-triggered, **read-only** request for a written narrative diagnosis (the lighter sibling of *Investigate with Prime*: a one-shot explanation rather than a full chat). It is offered on a failed-run card always, and on a blocked-task card **only when the task's latest run actually failed** (otherwise there is nothing for the model to read — `recovery.ts`'s `withFollowups({analyze})` gates it, so the card never shows a useless button). Clicking it calls the new **`POST /v1/relux/runs/:id/diagnose`** route (`crates/relux-kernel/src/server.rs::diagnose_run`; pure model in `crates/relux-kernel/src/run_diagnosis.rs`), which builds a **bounded + redacted** context from data the kernel already holds (run status/`failure_class`/adapter, task id/title, the redacted+clamped failure text, and the most-recent redacted log lines — every field clamped) and hands it **off-lock** to the configured brain under a no-authority system prompt. The brain returns a concise **four-part narrative** (likely cause / evidence / recommended next action / uncertainty), re-redacted and length-clamped. It **mutates nothing** (no tools, no task/run creation), is gated to runs that actually failed, and — **with no provider configured (or on a hiccup)** — returns a clean `mode: "unavailable"` fallback that points back at the deterministic card and offers the configure path, never a fabricated diagnosis. The result renders **inline** on the `RecoveryCard` (its new `diagnostic` block), **below** the still-visible deterministic card. Wired in `apps/dashboard/src/api.ts` (`reluxWork.diagnoseRun`, `ReluxDiagnostic`) + the Run Detail / Task Detail panels in `Work.tsx` (`analyzeRun` / `analyzeTask`). **Tests:** the pure context bounding/redaction, the four-part prompt framing, and the model-output assembly (incl. a provider test-double, the no-provider fallback, and a model-echo redaction) are pinned in `crates/relux-kernel/src/run_diagnosis.rs` unit tests; the route's no-provider fallback, the "nothing to diagnose" gate, session-gating, and the 404 in `crates/relux-kernel/src/server.rs` tests (`diagnose_route_*`); the `analyze` action gating in `apps/dashboard/test/recovery.test.ts`; and the inline render (wired button vs. muted pointer; model narrative + provenance vs. honest unavailable note vs. error/loading) in `apps/dashboard/test/work-diagnose-render.test.mjs`.

**Now built (§6.11 below):** the true cross-Guild **Inbox** queue (§5) that collects the attention items across the whole Guild into one prioritized list. The **Investigate → chat companion seeded with the diagnosis** choice is built (see *Investigate with Prime* above): a future enhancement is having Prime auto-pull the run transcript/logs into that first turn rather than relying on its own read-only tools when it needs more depth.

### 6.11 Cross-Guild Inbox v1 (IMPLEMENTED)

Closing §6.10's last pending item — and delivering **§5 "The Inbox (the operator's home)"** as its own surface: a single, dense, prioritized **attention queue** over the whole Guild, not scattered across Work detail panels. It is a **read-only projection of live state** ("computed from live state, not a notification table" — §5), so it never drifts from reality and stores nothing.

- **Backend — `GET /v1/relux/inbox` (`crates/relux-kernel/src/server.rs::get_inbox`).** A sibling of `get_oversight`, composed under one read-only lock; it adds no authority and mutates nothing. It collects four attention kinds into unified **`InboxItem`s** — each with a **stable, category-prefixed id** (`approval:` / `run:` / `task:` / `continuation:`), a `kind`, a `severity` (`critical` / `warn` / `info`), a plain `title` + `summary`, the related `task_id` / `run_id` / `approval_id` / `continuation_id`, the underlying `failure_class` when any, the recommended **action kinds** (each backed by an existing route), and a dashboard `link`:
  - **pending approvals** — every pending gate (severity from its risk). Each approval item also **embeds the full approval record** (the same `ReluxApproval` shape the Approvals page + Work oversight strip consume, via the shared `approval_record`), so the Inbox row can offer the **inline** decisions with no second fetch — the projection still grants no authority (every decision flows through the existing `decide` / `execute` / `allow-always` routes);
  - **failed runs** — hard-failed runs needing retry/diagnosis, **EXCLUDING the ones silently auto-retrying** (a transient class with a non-exhausted retry budget — §3.3b "transient failures retry silently … so the Inbox stays signal, not noise") and **de-duped** against the blocked-task list (a failed run whose task is blocked is represented by the richer blocked-task item);
  - **blocked tasks** — held work needing a lifecycle decision, folding in the latest failed run's class so diagnose/investigate are honest; the reopen actions are offered **only when the task is reopen-eligible** (assigned), mirroring the kernel guard;
  - **a paused Prime continuation** — a loop with work still to do (or one waiting on a tool approval first).
  The per-kind action gating mirrors the §6.10 recovery model (a Retry only when retry-eligible, a Reopen only when reopen-eligible), and items are ordered **most-urgent first** (severity, then recency). Per-category caps keep the response bounded; a `truncated` flag says so honestly (the rest live on the Work board).
- **UI — `apps/dashboard/src/pages/Inbox.tsx` + the pure `apps/dashboard/src/inbox.ts`.** A new top-level **Inbox** nav entry (with a live **badge** = the attention count, the §5 "sidebar Inbox badge is the sum of these") and an **Attention queue** page that renders the items grouped into dense sections (Approvals · Failed runs · Blocked work · Paused loops) in priority order. `inbox.ts` (pure, unit-tested) maps each action kind to its button **label + invocation mode** — a `post` to an existing route, a `nav` to the surface that owns the richer control, or a Prime investigation `seed` — and builds the safe, redacted investigation seed (reusing `investigateseed.ts`). Every button reuses an **existing** route/affordance: **Open approval** → Approvals; **Retry** → `/runs/:id/retry`; **Reopen** / **Reopen & run** → `/tasks/:id/reopen[-and-run]`; **Analyze failure** → the read-only `/runs/:id/diagnose` (narrative renders inline); **Investigate with Prime** → seeds Prime and routes to `/prime`; **Open continuation** / **Inspect** → the Work board. **No new authority, no auto-run, no auto-approve, no auto-diagnose** — every action fires only on the operator's click. The empty state is **honest** ("Nothing needs you right now").
- **Inline approval decisions on the row (IMPLEMENTED).** A pending-approval row now decides the approval **in place** — the same affordance the Work oversight strip offers — through a **shared `ApprovalInlineDecisions` component** (`apps/dashboard/src/components/ApprovalInlineDecisions.tsx`) that both surfaces render, so the inbox can never offer a decision the Approvals page wouldn't. The action set is the pure `approvalInlineActions` model: a bound **per-call tool invocation** gets **Approve & run** (decide → execute once) / **Allow always** (persist the standing grant → run once) / **Deny**; a **generic** approval gets **Approve** (records the decision, runs nothing) / **Deny** with the honest "nothing runs here" caveat; **Open approval →** to the full record always remains. Each button drives the **exact same** `reluxApprovals.{decide,execute,allowAlways}` routes — **no new authority** — and after any decision the row refreshes the Inbox (and its badge) in place and shows a **compact, shaped** one-line result/error, never the raw tool envelope. If an older projection omits the embedded record, the row degrades to the generic **Open approval** nav button (never a dead end).
- **Tests.** Backend projection — `inbox_route_projects_only_attention_items_and_is_selective` (a fresh store is empty; a freshly *created* task is **not** surfaced — the queue is signal, not a task dump; blocking it surfaces a `blocked_task` item with honest fields + reopen actions). Frontend pure — `apps/dashboard/test/inbox.test.ts` pins the action-spec totality, the post/nav/seed mode split, the nav-target resolution, the grouping order, the badge count, the severity tones, and the per-kind investigation-seed gating. Render — `apps/dashboard/test/inbox-render.test.mjs` mounts the real page under the declarative router (no mount throw, honest pre-data state) and asserts the committed bundle carries it; `apps/dashboard/test/inbox-approval-render.test.mjs` drives the real `InboxRow` with a seeded pending-approval item and asserts the **inline** action set per shape (a tool-invocation approval → Approve & run / Allow always / Deny; a generic approval → Approve + Deny with the "nothing runs here" caveat, no allow-always) and the honest **degrade to Open approval** when no record is embedded. Backend embed — `inbox_pending_approval_item_embeds_full_record_for_inline_decisions` pins that a pending approval surfaces with the full `approval` record (status/action/risk, no bound invocation for a generic approval) and the `open_approval` nav action alongside. The browser smoke clicks through the new **Inbox** nav route.

**Still future (deliberately):** triage SLAs / ageing on items, cross-item grouping (collapsing a whole stalled subtree into one card), and cross-project search/filtering of the queue.

---

## 7. The Issue detail (the heart)

This is where the product lives. Three-pane: **description + conversation** in the middle, **properties** on the right, with sections below. Mirror Paperclip's `IssueDetail` + `IssueChatThread`:

- **Header:** status (clickable to change), priority, identifier, live pill, project/goal links, labels, parent-chain breadcrumb, a pause-hold banner if the subtree is held, action buttons (assign / copy-as-markdown / properties toggle).
- **Title + description:** inline-editable, with @-mentions and image upload.
- **The conversation (the centerpiece):** the issue *is* a chat thread. It merges, in one stable timeline: your comments, agent messages, **live run transcripts** (the agent's tool calls/thinking/output streaming in), system notes (status/assignee changes, run summaries), and **interaction cards**. Built on an agent-chat runtime so streaming feels native. Relix already renders run transcripts — this reuses that rendering inside the thread.
- **Interaction cards** (answerable inline): **Ask questions** (radio/checkbox), **Request confirmation** (yes/no — used for plan approval), **Suggest tasks** (a selectable sub-task tree the agent proposes → you accept and they're created/assigned). A proposed child that carries an **assignee hint** (an Operative id or a role) shows that hint as a chip **before** you accept (so you see who each child would be assigned to — still validated through the assign-Key gate on accept); after accept, each created child shows its resolved state — **assigned: <who>** or **needs assignment** (deep-linked to its board card). This is what turns "the agent asks" from a dead-end comment into a click.
- **The composer:** markdown + mentions + attachments, a **work-mode toggle** (standard / planning), and reopen/interrupt semantics (comment to reopen a done issue, or interrupt a running agent).
- **Sections below:** sub-issues (with progress), documents (plan/design with revisions + the plan-approval flow), blockers, run history (the ledger), work products (deliverables/PRs/preview URLs), attachments.
- **Properties panel (right):** status, priority, assignee, project, goal, labels, blockers (add/remove), parent, reviewers/approvers, the model lane (which model/effort), monitor, and the workspace (branch/folder/service URL).

---

## 8. The Run transcript view

A standalone, reusable renderer (used in the issue thread, on the agent's Runs tab, and in the live-runs view). Mirror Paperclip's block-grouping: fold the raw adapter stream into typed blocks — assistant/thinking message, tool-call cards (matched call→result, status running/done/error), grouped consecutive tools/commands into collapsible accordions, batched stderr/system logs, colorized diffs, and init/result events (with tokens + cost). Two modes — a "nice" grouped view (default) and a "raw" virtualized dump. Live-tailed via the realtime socket + polling fallback. Relix already produces run transcripts; this is the *rendering* pattern to adopt.

---

## 9. The Org chart + the per-agent permission panel (the governance surface)

The **org chart** is a pan/zoom/pinch tree (SVG edges + cards), each node showing the agent's icon, a live status dot (running/idle/paused/error), role/title, and adapter. Clicking a node opens the agent detail — which is *also* where you govern it.

The **agent detail** has tabs: **Overview** (latest run, charts, recent issues, cost), **Instructions** (its markdown bundle — the job description, editable), **Skills**, **Configuration** (adapter/model), **Runs** (the transcript master/detail), **Budget**, and — the one you care most about — **Permissions**.

**The Permissions panel is where Relix deliberately goes denser than Paperclip** (which ships only two toggles). It's a clean, grouped switchboard (main doc §5.2):
- **Org powers:** can spawn/hire agents (+ *directly* vs *route hires through its boss*), can configure other agents (scoped to its subtree), can manage others' work.
- **Work powers:** can assign/delegate work, with a **scope** (anyone / only my subtree / specific agents / specific projects).
- **Capability powers:** tools it may use (per-tool/category toggles), secrets it may access, risk ceiling, actions that always require approval.
- **Autonomy & budget:** scheduled heartbeat on/off + interval, wake-on-assignment, concurrency, monthly budget.

Because Relix's existing agent-gate already understands categories, risk, secrets, and scopes natively, this panel is a *face* on machinery that exists — not new enforcement logic. Consider shipping **role presets** (CEO / manager / worker / read-only) that set sensible toggle bundles, with the raw toggles underneath for power users.

### 9.1 Crew create/edit configuration (IMPLEMENTED)

The full org-chart agent-detail tabs above are the target; the **Crew page** (`/crew`, `apps/dashboard/src/pages/Crew.tsx`) ships the operational core of the **Configuration** tab today, so an operator can configure crew directly (for a product where Prime hires/uses crew, this is table stakes):

- **Create** — a `CrewMemberForm` (name, optional id derived from the name, role/description, **persona** = operating style, a compact **skills/tags** field (comma-separated), and an **adapter/runtime** picker populated from the live adapter roster, defaulting to the local Prime adapter).
- **Edit** — each crew card has an **Edit** action that opens the same form inline, adding a **status** select (the operator-settable `active`/`paused`/`disabled`; machine-driven `Error` is not offered). Absent fields are left unchanged; an empty persona clears it. The skills field is sent on every save — a present (possibly empty) list **replaces** the whole skill set, so an empty field clears all skills.
- **Skills/tags** — each crew card shows the agent's skills as small monochrome chips. Skills are bounded specialty slugs (`research`, `rust`, `frontend`) used by Prime's fuzzy assignee resolution to route work to a unique specialist (see below).
- The existing **Adapters** status cards (enable/disable a CLI adapter) stay on the same page — adapter *runtime* control and agent *configuration* sit side by side.

Backend: `POST /v1/relux/agents` (now accepts `persona`, `skills`, + a validated adapter) and `PATCH /v1/relux/agents/:id` (edit). Both validate/sanitize/clamp through `crates/relux-kernel/src/agent_config.rs` (pure, unit-tested): name required, id/name unique, adapter must resolve to a known/installed adapter, status from the allowlist, persona bounded **and secret-redacted**. **Skills/tags** are validated by `validate_skills`: each entry is reduced to a strict slug (`[a-z0-9-]`, lowercase, separators → hyphen), clamped to `MAX_SKILL_CHARS` (32), deduped (case-insensitive); an entry with real content that sanitizes to nothing is **rejected** with an honest `invalid skill '<x>'` 400, and more than `MAX_SKILLS` (16) distinct skills is a `too many skills` 400. Validation failures surface as honest 400s in the form (duplicate id/name, unknown adapter, bad status, invalid/too-many skills); a missing agent on edit is a 404.

**Skill-aware assignment matching.** `relux_core::Agent` now carries a `skills: Vec<String>` (`#[serde(default)]` so agents stored before this field load fine), surfaced into Prime's grounded `StateSummary.agent_skills`. Prime's fuzzy assignee resolver (`crates/relux-kernel/src/prime.rs` `resolve_assignee`) gained a **skill tier** that sits AFTER an exact id/name match and BEFORE the looser prefix/substring fallback: a phrase like "the rust specialist" resolves to the single agent tagged `rust`, but if two agents share that skill it is **ambiguous → Prime asks which one** (a shared skill is never silently guessed), and an exact id/name always wins over a skill. A resolved id is always taken verbatim from the live roster, so a skill phrase can only ever name an agent that exists (fail closed).

**Role presets (IMPLEMENTED).** The create form offers a compact **Role preset** selector seeded with a small curated list of common crew types — **Researcher**, **Builder / Coder**, **Reviewer**, **Planner**, **Operator / Support**. Picking one and clicking **Apply** fills the **role**, **persona**, and **skills** fields with that type's defaults; the fields stay fully editable before save, and Apply confirms first when any of those three already holds operator-entered text, so it never clobbers in-progress work. A preset touches **only** those three advisory fields — it never changes name/id/adapter/status and, critically, **grants no permissions**: it cannot widen an agent's power. Backend: a new read-only `GET /v1/relux/agent-presets` returns the curated list (`crates/relux-kernel/src/agent_presets.rs`, pure, unit-tested — the single source of truth the UI fetches); `POST /v1/relux/agents` also accepts an optional `preset` id (for API clients) which fills any role/persona/skills the request omitted (the request's own value always wins) and then flows through the **same** `validate_new_agent` validators — no duplicate validation, an unknown preset is an honest 400, and `create_agent` still grants only the minimal echo tool. Structurally the preset type carries no permission/adapter field, so the no-auto-grant rule holds by construction (mirrors openclaw `sessions-spawn-tool.ts`, where a named role is a context label that never expands the worker's toolset — see `docs/reference-driven-development.md`). Presets are offered in **create** mode only (they seed a new member, not reshape an existing one).

**Governance — permissions (IMPLEMENTED).** The first slice of the §9 Permissions panel ships: each crew card now lists the agent's **explicit permissions** (the `<prefix>:<resource>:<action>` strings — least privilege, so this is its full effective power), and the edit card has a compact **Governance** section to **grant** and **revoke** them. Backend: `POST /v1/relux/agents/:id/permissions` (grant, existed) and `DELETE /v1/relux/agents/:id/permissions` (revoke, new — `KernelState::revoke_permission_from_agent`, audited as `agent:revoke_permission`, fails closed with a 404 when the agent does not hold the permission). The `AgentRecord` now carries the explicit `permissions` list. The operator console is the human approval (the same gate as clicking the button), so grant/revoke act immediately and are audited; **nothing dangerous is auto-granted** — `create_agent` still grants only the minimal echo tool, and an elevated (control-plane: `adapter:`/`provider:`/`exec:`/`plugin:`/`agent:`/`approval:`) grant requires a deliberate confirm in the UI (`apps/dashboard/src/governance.ts`, unit-tested; mirrors openclaw's exec/control-plane approval classes — see `docs/reference-driven-development.md`). Prime's own `GrantPermission` stays approval-gated; this surface is the operator governing their own crew.

**Still future work:** **per-agent budget** is not yet modeled (`relux_core::Agent` has no budget field), so it is deliberately absent rather than shown as unenforced UI; the richer scoped org/work/capability/autonomy toggles from §9 above also remain future work. Identity, role, persona, adapter, status, explicit-permission view + grant/revoke, skills/tags, **and role presets** are now modeled. Skills are used by assignment matching but are **not** yet a capability/permission gate (a skill describes specialty, it does not grant power) and there is no curated company-wide skill vocabulary — any valid slug is accepted.

---

## 10. The other surfaces (briefly)

- **Goals:** a goal tree; each goal shows sub-goals + linked projects; the "why" hierarchy.
- **Projects:** workstreams; a project detail has Issues / Overview / Configuration / Budget tabs.
- **Costs:** spend by company/agent/project/issue, budget progress bars, incident cards (resolve = raise-and-resume / keep-paused), provider/biller breakdowns. The issue-tree cost rollup (a planner's issue shows the whole effort's cost) surfaces here and on the issue.
- **Approvals:** the gate list (pending/all) + detail with the typed payload (hire / strategy / budget / high-risk) and Approve/Reject/Request-revision.
- **Agents:** the employee list (status filters) + the org-view toggle.
- **Activity:** the audit/event stream (Relix's hash-chained audit, product-faced).

---

## 11. Realtime (one socket → surgical updates)

Mirror Paperclip's realtime engine, which Relix is well-positioned for because it **already has a per-tenant live-events WebSocket**. The pattern:
- Open **one socket per selected tenant**.
- Incoming events (`run.status`, `agent.status`, `activity.logged`, etc.) drive **surgical cache invalidation** keyed by a centralized, hierarchical **query-key factory** — not blanket refetches.
- **Route-aware optimizations:** if you're *viewing* the affected issue, hydrate the new comment directly into the cache (no flicker); use "inactive" refetch for background queries.
- **Rate-limited toasts** (e.g. max 3 per category per 10s), and suppress toasts for the issue you're already looking at.

This is what makes the console feel alive without thrashing. The query-key factory discipline is the unglamorous thing that makes it tractable — adopt it early.

---

## 12. The design system / feel

- **Aesthetic: Vercel-style black & white — stark, clean, high-contrast, minimal.** True-black / true-white base, generous whitespace, thin low-contrast gray borders, flat surfaces (almost no shadows), tight geometric corners. A clean sans-serif (Geist / Inter family) for text and a **monospace (Geist Mono) for identifiers, numbers, and code**. Both light and dark modes share the same stark B&W base. **Color is reserved for meaning only** (status / priority) and even then kept muted and minimal — never decorative. This is the look: clean, confident, monochrome — *not* Paperclip's charcoal-and-blue Linear style (we copy Paperclip's structure, not its skin).
- **Dense but scannable, keyboard-first** (⌘K palette, `c` = new issue), built on a small primitives library (buttons/badges/cards/dialogs/popovers/tabs/etc.) over CSS-variable tokens.
- **Status/priority vocabulary stays, rendered restrained:** one consistent set (blocked, live/running, done/healthy, error) but as muted dots/badges that fit the B&W minimalism — never loud color fills.
- **Progressive disclosure:** human summary → steps/artifacts → raw transcript. **No log-worship** — raw logs are one click deeper, never the default.
- **No silent failures:** every failed run is visible (this aligns with Relix's existing "honesty contract" — surface problems, don't hide them).
- **Skeletons not spinners; optimistic edits with rollback; placeholder-data to avoid layout jumps.**

---

## 13. The chat companion (deep design)

This is the surface you described — and it's the bridge between "reason with the model" and "issue-first." It is *not* a generic chatbot; it's a **context-aware operating console for the company**.

### What it is
- **Context-aware.** The companion can read the live company state — current issues, the org, who's running, recent activity, costs, budgets. Ask "what's the planner stuck on?" and it actually knows, because it has read access to the same data the dashboard shows.
- **A thinking partner.** You talk through what you're trying to do; it reasons back, proposes options, points out tradeoffs (cost, risk, who should own it).
- **A materializer.** When you like a direction, you say it in plain language and it **creates real, governed work objects**: *"make this an issue and assign it to the CTO," "have the CEO spin up a research team for this," "ship it / put it in production," "raise the marketing budget to $50."*

### How it's built (reusing what Relix has)
The companion is essentially a **special operator-facing agent** with two things wired in:
1. **Read tools over the company state** — list issues/agents/goals/runs/costs (the same API the dashboard uses).
2. **Write tools that create work objects** — create an issue, assign it, create a goal/project, instruct the CEO (i.e. hand the CEO an issue), set a budget, etc.

Relix already has the substrate for this: the OpenAI-shim chat surface, an AI node that can already read memory/state, the tool/dispatch layer, and the permission gate. The companion is that chat surface upgraded with company-state tools and work-creation tools — *not* a new engine.

### Governance (critical)
- **It runs as you (the Board), through the same gates.** Anything the companion creates — an issue, a hire request, a budget change — passes the *same* permission and approval gates as if you'd clicked the buttons. The chat does not bypass governance.
- **Preview-then-confirm for anything that spends money or hires.** For low-risk acts (create an issue, add a comment) it can act directly; for spend/hire/destructive acts it shows a **preview** ("I'll create these 3 issues and assign them to CTO / hire a planner agent — confirm?") and waits for your click. This keeps the conversational speed without surprise side-effects. *(This is one of the main-doc open questions — recommendation: preview-then-confirm for spend/hire/destructive.)*
- **Everything it does is audited.** Because it acts through the normal API + admission pipeline, every action lands in the activity log with the companion as the actor — full traceability.

### Where it lives in the UI
A top-level **Chat** entry (and reachable from anywhere via ⌘K). It can also be **contextual**: open it from an issue ("ask the companion about this issue") and it's pre-loaded with that issue's context. The result of a conversation is usually a link to the issue(s) it just created — so the chat *hands off to the board*, reinforcing "reason in chat, work lands as issues."

### The OpenAI-compatibility side effect
Because the chat surface stays, Relix keeps working as an OpenAI-compatible endpoint for external clients — but the *primary* chat becomes this company-aware companion, not a bare passthrough.

---

## 14. Build order for the dashboard (Phase 6, but seeded earlier)

The dashboard reshape is roadmap Phase 6, but pieces are needed as soon as their objects exist. A buildable order:

1. **SPA shell + tenant-prefix router + realtime wiring** (the foundation; reuses the existing live-events socket).
2. **The Issues board + issue detail (with the chat thread)** — the spine; lands with Phase 1's Issue object.
3. **The Inbox + Approvals** — lands with Phase 2's governance.
4. **The Org chart + the per-agent permission panel** — lands with Phase 2's org model.
5. **Goals / Projects / Costs / Activity** surfaces.
6. **The chat companion** — lands with Phase 4.
7. **Relocate the 22 feature panels** into their object tabs / Settings, retiring the legacy HTML as each lands.

The migration is safe because the web-bridge keeps serving one bundle; the legacy panels stay reachable until each replacement ships.

---

---

## 15. The Home readiness guide (IMPLEMENTED)

> **STATUS: shipped.** Unlike the rest of this doc (ideas-only for the goal-facing
> reshape), this section documents the **current, live** behavior of the standalone
> Relux SPA's Home page (`apps/dashboard/src/pages/ReluxHome.tsx`), served at
> `/dashboard` by `relux-kernel`. It is the first-run/operational guidance surface,
> grounded entirely in the local `/v1/relux` control plane.

**The goal:** a new operator should learn, from Home alone, how to configure
Prime's brain, enable a Claude/Codex adapter, add crew, configure plugins/tools,
and start the first work — without reading scattered docs — and a configured
operator should get a concise operational summary, not a nag.

**Where it lives.** A single compact, app-like card (`ReadinessGuide`,
`apps/dashboard/src/components/ReadinessGuide.tsx`) on Home, between the product
framing card and the orchestration/plugins cards. No hero, no nested cards; it
never blocks normal dashboard use. The **same** card also leads the Health page
(`apps/dashboard/src/pages/Health.tsx`), above the raw diagnostics, so the
first-run guidance and the operational summary are consistent on both surfaces —
built from the same `buildReadiness` derivation over the same local `/v1/relux`
reads (no duplicated logic). The older Home prose card that re-explained the
Claude/Codex real-work path was **removed**: the readiness guide's brain +
real-work-adapter items now cover it, so Home stays compact and non-redundant.

**Honest degradation on Health.** Health's reads are best-effort. If the core
`/v1/relux/health` read fails the page shows its honest "could not reach the
control plane" banner (never a faked-ready guide). If only a secondary read fails,
the guide degrades through `buildReadiness` rather than asserting ready.

The key distinction is **loading vs failed**. `buildReadiness` accepts a `failed`
flag set (`ReadinessFailed`: `state`/`ai`/`adapters`/`plugins`/`tools`) recording
which reads are null because the request **failed**, as opposed to null because it
is still **in flight**. A null read with no flag stays loading — the page renders
`report: null` → the guide's "Checking readiness…". A null read *with* its flag is
surfaced as an explicit, retryable **"… unavailable"** row (e.g. *State
unavailable* / *Tools unavailable* / *Plugins unavailable* / *Adapter status
unavailable*) — never indefinite checking text. The callers learn the distinction
from real page context: Health flags a read once its load has **settled**
(`!loading`, since each secondary read is fetched with `.catch(() => null)`); Home
flags a `useAsync` read once it has an `.error`. This fixes the prior gap where, if
`/v1/relux/health` succeeded but the `state` read failed, the guide could sit on
"Checking readiness…" forever instead of an explicit degraded row.

A report with any failed read is **degraded**: `degraded === true`, and `ready`
is forced false so the guide never paints a green "operational" badge or a faked
summary from partial data. The guide renders a third mode — a "Showing what is
available — retry to refresh" banner above the full checklist (the unavailable
rows carry a **Retry** button wired to the page's Refresh) and a `degraded` badge.

**What it derives (honest, live).** A pure, React-free module
(`apps/dashboard/src/readiness.ts`, `buildReadiness(inputs)`) turns the four reads
Home already makes — `reluxPlugins.state()`, `reluxAi.status()`,
`reluxAdapters.list()`, `reluxPlugins.list()` + `reluxTools.list()` — into one
report. No new endpoint. Each capability is one honest check with the exact page
that fixes it:

- **Prime brain** — reuses `onboarding.ts::primeBrainStep`. A SELECTED-but-broken
  brain (OpenRouter without a key; Claude/Codex CLI selected but off PATH or
  disabled) is the **only** blocker; a local deterministic brain *works* (shown as
  a recommendation to connect a richer brain, not a failure). Action → `/health`.
- **Real-work adapter** — whether a Claude/Codex CLI adapter is enabled and on PATH
  to *execute* assigned tasks (distinct from the brain). Optional, so an
  unavailable/disabled adapter is an actionable link, never a blocker. Action →
  `/crew`.
- **Crew** — at least one agent, else the honest local fallback ("Prime is your
  built-in operative and can do the work itself"). Action → `/crew`.
- **Plugins & tools** — reuses `plugins.ts::pluginCategory`/`toolReadiness`. A
  metadata-only wrapper (generated, zero tools) or a tool needing a loopback
  runtime is **attention** (`warn`); ready tools are `done`; approval-gated tools
  are noted, never counted as ready; a tools probe still **loading** stays an
  honest neutral `info` ("unavailable right now"), while a tools/plugin read that
  **failed** becomes an explicit `warn` *Tools unavailable* / *Plugins unavailable*
  row with a Retry — never "no tools configured". Action → `/plugins`.
- **Pending approvals** — surfaced only when something actually waits on a decision.
  Action → `/approvals`.

**Three modes (the no-nag rule).** `ready = blockers.length === 0 && !degraded`.
- **Degraded** (a read failed): the full checklist renders with the explicit
  "… unavailable" Retry rows and a "Showing what is available" banner — honest
  about the partial data, never a faked operational summary.
- **Setup needed** (a blocker exists, no failed read): the full checklist renders
  with per-item action buttons so the operator finishes setup.
- **Operational** (nothing blocks, nothing failed): a one-line, secret-free summary
  (`Brain: <label>. N agents, M tools ready. K open tasks, J running.`), any `warn`
  attention items shown quietly, and the full checks tucked behind a native
  `<details>` disclosure.

**The first action.** `deriveFirstAction(state)` always returns one clear next step
in priority order: review a pending approval → watch an active run → start/assign a
task → ask Prime to start the first task. Prime is always available, so even the
fresh state has a real action.

**Tests.** `apps/dashboard/test/readiness.test.ts` pins the four required states
(fresh/local-only, Claude available but disabled, metadata plugin needs config,
fully ready) plus the blocker and first-action priority, and the read-failure
honesty: a failed `state`/`tools`/`plugins`/`adapter` read produces an explicit
retryable `warn` row and a `degraded` (not-ready) report, while a still-loading
null read stays a neutral `info`/loading row with no Retry;
`apps/dashboard/test/readiness-guide-render.test.mjs` renders the component's three
modes (loading → "Checking readiness…", degraded → "… unavailable" + Retry +
`degraded` badge, operational → the summary);
`apps/dashboard/test/readiness-render.test.mjs` proves Home mounts under the
declarative router, the redundant "Run real work" prose card is gone, and the
committed bundle carries the copy; `apps/dashboard/test/health-render.test.mjs`
proves the Health page mounts the same guide and degrades to its honest loading
state. Reference grounding (openclaw `HealthStore`/onboarding, Hermes
`status`/`doctor`/`setup.status`) is recorded in
`docs/reference-driven-development.md`.

### 15.1 The Doctor panel (IMPLEMENTED)

> **STATUS: shipped.** The actionable, kernel-backed companion to the readiness
> guide on the Health page (`apps/dashboard/src/components/DoctorPanel.tsx`),
> grounded in a single read-only kernel endpoint.

**The goal:** the readiness guide derives pass/warn/fail *in the frontend* from
the reads Home already makes; the Doctor is the **kernel-side** diagnostic — the
kernel itself reports structured checks, so the operator gets a deeper, honest
"what's wrong and how to fix it" without leaving the dashboard.

**Backend — `GET /v1/relux/doctor` (read-only).** A new session-protected endpoint
(`relux_kernel::doctor::build_doctor_report`, handler `get_doctor` in
`crates/relux-kernel/src/server.rs`) reuses the SAME cheap reads as
`/v1/relux/health` (store open/load, dashboard bundle, AI status, adapter + tool
readiness, agent + approval counts) and returns structured rows. It does **no
heavy work** (no cargo build/test), no network beyond what health already does
(none), and no mutation. Each row carries an `id`, `label`, a `severity`
(`ok`/`info`/`warn`/`fail`), a secret-free `message`, and — where there is a
concrete fix — a `remediation` line and an in-app `action_link`
(`/health`, `/crew`, `/plugins`, `/approvals`). The report also carries an
`overall` severity (worst-of) and an `ok`/`info`/`warn`/`fail` `summary`.

The checks (and their severity rules, which match `readiness.ts` so the two
surfaces never disagree): **kernel.store** (fail if the store can't open/load —
the endpoint still returns an honest failing row rather than 500ing),
**dashboard.bundle** (warn when absent: the API works, only `/dashboard` shows a
build notice), **prime.brain** (a SELECTED-but-broken brain — OpenRouter without a
key, or a Claude/Codex CLI brain whose adapter is not runnable — is the failure; a
local deterministic brain is healthy `info`), **adapters.real_work** (optional, so
`info`/`ok`, never a blocker), **plugins.tools** (tools needing a loopback runtime
are `warn`; ready tools `ok`; approval-gated tools noted, never counted as ready),
**crew**, and **approvals.pending** (`warn` only when something waits).

**Redaction.** The Doctor takes NO filesystem paths as input (`DoctorInputs`
carries booleans/counts/states only), so a db path or a resolved binary path can
never reach a check message — structural redaction, mirroring openclaw's
admin-only `includeSensitive` path surfacing. The AI model **name** is shown (safe,
never the key).

**UI.** A compact, scan-friendly card on Health, directly below the readiness
guide (so it is reachable the moment the guide reports degraded). It shows the
`overall` badge, a one-line headline (`N fail, M warn, …`), and the rows sorted
worst-first, each with a severity badge, the message, the remediation, and a
**Fix →** link to the action route. A **Refresh** button re-runs the bounded read.
If the doctor read fails it shows an honest error (never a blank panel, never a
faked-green report). Presentation helpers are pure (`apps/dashboard/src/doctor.ts`:
`severityBadgeClass`/`severityLabel`/`sortChecksBySeverity`/`doctorHeadline`).

**Tests.** `crates/relux-kernel/src/doctor.rs` unit tests pin every severity rule
(local→info, OpenRouter-no-key→fail, disabled→warn, Claude CLI available→ok /
missing→fail, tools-need-runtime→warn, store-fail→fail, pending-approvals→warn,
missing-bundle→warn, overall aggregation) **and the redaction** (a path-shaped
adapter value never appears in the serialized report);
`server.rs::doctor_requires_a_session_and_returns_structured_checks` proves the
endpoint is session-gated, returns the expected rows over a bootstrapped store, and
never echoes the db path. Frontend: `apps/dashboard/test/doctor.test.ts` pins the
pure helpers; `apps/dashboard/test/doctor-render.test.mjs` renders the panel's
ok/warn/fail/error/loading states, proves Health mounts it, and asserts the
committed bundle carries it. Reference grounding (Hermes `hermes_cli/doctor.py`
`check_*`/`_fail_and_issue`; openclaw `gateway/server/health-state.ts`
`includeSensitive`) is recorded in `docs/reference-driven-development.md`.

---

*This is the dashboard-and-companion design. With the company model (`relix-company-model.md`), the execution spine (`relix-execution-and-issue-design.md`), and this, the three docs cover the product, the engine, and the surface — all grounded in the complete Paperclip read, all ideas-only. The next concrete step is to pick the first build slice (Phase 0/1) and design its exact data shape against Relix's coordinator schema.*
