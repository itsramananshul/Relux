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

**Still pending (not built here, deliberately):** drag-to-change-status, list↔board toggle, and per-subtree live **cost** rollup on the board. These remain §6/§7 targets. (Inline approve/reject/allow-always *on the strip* is now built — see the inline-decision bullet above; the dedicated Approvals surface remains the detailed audit/grants/permissions home. **Sub-issue nesting / workflow-checklist rendering and per-subtree progress strips are now built — see §6.2.**)

### 6.2 Work hierarchy / progress v1 (IMPLEMENTED)

The board now surfaces **sub-work and progress from real data** (design §6 "A progress strip on a parent: done/in-progress/blocked counts, a segmented bar … waiting on blockers" + §6.1's nesting/workflow-checklist target). A **Work groups** card sits on the Work page (`apps/dashboard/src/pages/Work.tsx`, the `WorkHierarchy` section) between the Oversight strip and the columns.

**The real hierarchy (no fake grouping).** The ONLY parent→child relationship the kernel records today is the multi-agent **orchestration**: each goal's `steps[]` carry the real child `task_id`, the specialist `role`, the durable `outcome`, and `depends_on` (indices into the step array — the genuine dependency / blocked-by / blocking edges). `relux_core::Task` has a `parent_task` field but the kernel **never populates it**, and there is no ad-hoc task→subtask link — so the orchestration **is** the parent, a task in no orchestration is genuinely standalone (a flat card in the columns below, never given a fabricated parent), and a planned orchestration with no committed steps is not shown as a parent. **Honest divergence:** the `parent_task` field is present but inert and ad-hoc (non-orchestration) task subtrees are unsupported until the kernel populates a real link — noted here as the authoritative record for this Relux-shell slice.

**What it composes (two existing reads, no new route).** `apps/dashboard/src/workhierarchy.ts` (pure, unit-tested) joins `reluxOrchestration.list()` (the structure + dependency edges) to the live `reluxWork.listTasks()` (the current per-child status), so a group's progress reflects **what the board columns actually show** — not a stale single summary field. A child is bucketed by its **live** task status (the same `oversight.ts::taskBucket` the columns use, so the strip and the columns can never disagree); a child absent from the current board view falls back to the durable step `outcome` and the card says so honestly ("Progress is from the orchestration record — these briefs are not on the current board view"). No backend was changed.

**What it shows.**
- **Per-group progress strip** — a compact **segmented bar** (`.seg-bar`, one slice per non-empty board bucket: done / running / blocked / open, painted with each bucket's semantic CSS var — color is meaning-only, §12), the **brief count**, and a `2/5 done · 1 running · 1 blocked` label. Done/in-progress/blocked counts and the segmented bar are exactly the §6 progress-strip spec.
- **Nested numbered workflow checklist** — expandable per group: each child is a numbered row (`1`, `2`, `3` …, the step position) with its title (→ Inspect), the specialist **role** badge, the **live status** badge, the assignee (resolved to a crew name), and the **blocked-by / blocking** dependency chips resolved from `depends_on` to sibling task ids. (Orchestration is a single level today, so the plan numbers `1..N`; deeper `1.1` nesting is not fabricated since the data is flat.)
- **Parent context in the task detail** — when a selected task is a brief inside a group, the Task Detail panel shows a "**part of** &lt;goal&gt;" header, the group's segmented progress, and the full numbered plan with the open task **highlighted** (the child-side of the same relationship).
- **Honest empty / degraded states** — no orchestration with committed steps → "**No sub-work yet** — no multi-agent goal has been decomposed into a grouped plan." A failed orchestration read degrades to an inline "Work groups unavailable — the board below still works" note (excluded from the page error gate, like the Oversight strip), never a blank.

**Tests.** `apps/dashboard/test/workhierarchy.test.ts` pins the join + progress semantics (live status wins over `step.outcome`, the durable-outcome fallback, the four-bucket tally, segment ordering, blocked-by/blocking resolution, `groupForTask`, empty inputs); `apps/dashboard/test/work-hierarchy-render.test.mjs` renders the real `WorkHierarchy` with a seeded parent + children in mixed states and asserts the progress strip, the numbered checklist, the role/live-status badges, the dependency chips, and the empty/error states; `work-render.test.mjs` asserts the section is part of the board's first paint. Reference grounding: Hermes' kanban dashboard computes the same parent progress rollup (`{done, total}` per parent from `task_links` joined to child status, `plugins/kanban/dashboard/plugin_api.py`) and the same blocked-by-status dependency model — recorded in `docs/reference-driven-development.md`.

**Still pending (deliberately):** ad-hoc (non-orchestration) task subtrees — needs the kernel to populate `parent_task` or a task-link table first; per-subtree **cost** rollup on the board (the Relux run-cost ledger exists but is not yet joined here); drag-to-reorder the plan. These remain §6/§7 targets.

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
