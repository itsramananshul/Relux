# Relix To Final Product Plan

Created: 2026-06-05

Owner: Anshul

Purpose: this is the working plan for taking Relix from the current real-but-rough system to a Paperclip-class product. It is written for Anshul, Claude, and Codex. It should be used before giving future implementation prompts so work does not drift into random features.

Location: `.relix-audit/RELIX_TO_FINAL_PRODUCT_PLAN.md`

This file is local-only. `.relix-audit/` is excluded through `.git/info/exclude`; do not commit it unless Anshul explicitly asks.

## Source Material Read

This plan is grounded in the completed strict audits:

- Relix strict audit:
  - `.relix-audit/RELIX_CODEBASE_AUDIT_LOG.md`
  - `.relix-audit/relix-file-line-coverage.jsonl`
  - `.relix-audit/relix-file-line-coverage-progress.md`
- Paperclip strict audit:
  - `references/paperclip/RELIX_PAPERCLIP_AUDIT_LOG.md`
  - `references/paperclip/.relix-audit/paperclip-file-line-coverage.jsonl`
  - `references/paperclip/.relix-audit/paperclip-file-line-coverage-progress.md`
- Relix idea/design docs:
  - `docs/relix-company-model.md`
  - `docs/relix-execution-and-issue-design.md`
  - `docs/relix-dashboard-design.md`
  - `docs/relix-hermes-integration.md`
  - `docs/relix-agent-adapters.md`
  - `docs/relix-lexicon.md`
- Relix implementation/status docs:
  - `docs/product-spine-implementation.md`
  - `docs/product-spine-roadmap.md`
  - `docs/current-limitations.md`

Verification immediately before this plan:

- Relix ledger rows: 805.
- Relix changed/missing rows since strict audit: 0.
- Paperclip ledger rows: 2639.
- Paperclip changed/missing rows since strict audit: 0.

## The Blunt Diagnosis

Relix is not behind because it has no backend. Relix is behind because the backend is not yet arranged into one obvious product loop.

Paperclip feels like a product because the user can see one connected loop:

```text
create company -> create agents -> assign issues -> run agents -> watch runs
-> review output -> manage approvals/secrets/workspaces -> understand status
```

Relix currently has many of the hard pieces:

```text
mesh, identity, policy, audit, coordinator ledger, Briefs, Mandates, Crew,
Rigs, bridge, dashboard, auth, Claude/Codex adapter path, memory, tools,
approvals, allowance/cost ideas, channels, docs, scripts
```

But the user-facing path still feels stitched together:

```text
boot -> login -> dashboard -> create work -> assign crew -> run -> see output
```

is not yet reliable, obvious, polished, and emotionally satisfying.

So the main problem is not "build more random backend." The main problem is:

1. Make the product loop real end to end.
2. Remove duplicated/confusing surfaces.
3. Make failures obvious and actionable.
4. Make the dashboard feel like the product, not a debug shell.
5. Harden execution until a Founder can trust it.

## Product Definition

The final Relix product is:

> A secure Guild of AI Operatives that the Founder can govern like a company, where work is organized into Mandates, Campaigns, Briefs, Shifts, Clearances, Allowances, Dossiers, and Chronicle events, and each Operative can run on a chosen Rig such as Claude, Codex, Hermes, or another adapter.

The product must feel like this on first use:

1. The Founder runs setup/boot.
2. Relix asks for or creates dashboard admin credentials.
3. Dashboard opens.
4. Founder sees a Command Center, not a raw endpoint console.
5. Founder creates or reviews the Guild.
6. Founder sees the Prime and Crew/Roster.
7. Founder creates a Mandate in plain language.
8. Prime/team planning can turn the Mandate into Briefs.
9. Founder assigns a Brief to an Operative.
10. Operative runs on its configured Rig.
11. Founder sees the Shift status live or near-live.
12. Founder sees the output, Chronicle, Dossiers, Clearances, Snags, and next action.
13. Founder can approve, reject, re-run, comment, reassign, or mark done.

If that loop does not work, the product is not real yet, even if the mesh is brilliant.

## Non-Negotiable Future Work Rules

These rules should be included in future Claude prompts.

1. Work only on `main`.
2. Never create a branch.
3. Do not delete user work or revert unrelated changes.
4. Commit and push frequently only when explicitly asked or when Anshul's current rule says to do so.
5. Do not add new product concepts before making the core loop work.
6. Do not build another dashboard surface.
7. The React dashboard in `apps/dashboard` should become the real dashboard.
8. Legacy dashboard surfaces should be retired or redirected after parity.
9. No silent empty states for failed API calls.
10. Every user-visible feature must have a clear empty/loading/error state.
11. Every backend feature that matters must have a dashboard workflow.
12. Every dashboard workflow that matters must have a live/manual smoke path.
13. Generated dashboard dist must not drift from source.
14. Do not fake Claude/Codex/Hermes availability; probe honestly.
15. Thin adapters must state their governance limits honestly.
16. Never let run workspaces execute in the live repo by default.
17. Any operation that can spend money, expose secrets, modify files, or create agents must have clear governance.
18. Keep docs/status honest. Do not leave stale limitations claiming old behavior after a feature lands.

## What Is Already Real

### 1. Runtime/Substrate

Relix already has serious infrastructure:

- controller runtime wiring;
- signed identity and bundles;
- policy/admission bridge;
- replay/audit paths;
- approvals and signed tokens;
- budget/metrics ideas;
- PII/security gates;
- AI/memory/tool/coordinator/channel/plugin/workflow/planning nodes;
- tenant-aware stores;
- Qdrant tenant isolation;
- tool security and SSRF controls;
- plugin/TLS surfaces;
- bridge-back token path for Rigs.

This is the strongest part of Relix.

### 2. Product Spine

Relix has shipped pieces of the lexicon/product spine:

- Guild;
- Mandate;
- Campaign;
- Brief;
- Shift/Run;
- Operative/Roster;
- Keys;
- Allowance;
- Chronicle;
- Clearance;
- Snags/Sub-briefs/Dossiers;
- Rig.

The issue is not "the objects do not exist." The issue is the whole loop is not yet smooth enough for a user.

### 3. Dashboard/Bridge

There is now a React dashboard in `apps/dashboard` with:

- login/first-run admin auth;
- layout;
- Overview/Command Center direction;
- Mandates;
- Briefs;
- Crew/Agents;
- Runs;
- Chat;
- Settings;
- maintenance/review/apply surfaces.

But there are also older surfaces:

- legacy `crates/relix-web-bridge/src/dashboard.html`;
- `crates/relix-web-bridge/src/spine_dashboard.html`;
- React dashboard bundle in `crates/relix-web-bridge/dashboard-dist`.

This duplication makes the product feel confused.

### 4. Adapter/Rig Path

Relix has a real Rig abstraction:

- `Rig` trait;
- `EchoRig`;
- `ProcessRig`;
- CLI Rig probes;
- Claude/Codex readiness;
- bridge-back support;
- per-run workspaces;
- generated `BRIEF.md`;
- workspace context modes;
- async dispatch direction;
- structured output parsing for Claude/Codex paths.

The gap is turning this into a first-class product workflow with reliable auth, run visibility, quota hints, and safe workspace review/apply.

## What Paperclip Has That Relix Still Lacks

Paperclip's advantage is not one magic feature. It is integration density.

Paperclip has:

1. A coherent app host.
2. One product dashboard.
3. A strong company/work object model.
4. Issue detail as the center of work.
5. Agent execution as a loop, not a one-off command.
6. Recovery and retry behavior that appears in the product.
7. Rich run transcripts and lifecycle surfaces.
8. Access/resource membership model.
9. Workspace runtime and output review.
10. Plugin ecosystem and SDK surfaces.
11. Cost/budget views.
12. Good storybook/product state coverage.
13. Operational docs and setup flows.

Relix has a stronger signed mesh and security substrate, but Paperclip has a more mature product loop.

## Core Strategy

Stop adding disconnected power.

Build one golden path:

```text
Boot -> Login -> Command Center -> Create Mandate -> Plan Team
-> Create Briefs -> Assign Crew -> Run via Rig -> Watch Shift
-> Review Output -> Apply/Reject -> Chronicle -> Next Action
```

Everything should be judged by whether it improves this path.

## Workstream A: Boot, Setup, Login, Dashboard Trust

### Goal

The user should be able to run Relix, open the dashboard, log in, and not hit 401/502/confusing unavailable panels.

### Current Problems

- User has already experienced dashboard API 401s.
- Admin password can be forgotten.
- Bridge token/admin auth history is confusing.
- Some endpoints may still require auth differently.
- First-run setup and dashboard login must feel deliberate.
- Dashboard health indicators must explain exact fixes.

### Code Areas

- `crates/relix-web-bridge/src/dashboard_auth.rs`
- `crates/relix-web-bridge/src/auth.rs`
- `crates/relix-web-bridge/src/main.rs`
- `crates/relix-web-bridge/src/config.rs`
- `crates/relix-web-bridge/src/config_api.rs`
- `apps/dashboard/src/auth.tsx`
- `apps/dashboard/src/api.ts`
- `apps/dashboard/src/pages/Login.tsx`
- `apps/dashboard/src/pages/Settings.tsx`
- `scripts/relix-dashboard-admin-reset.ps1`
- `scripts/relix-dashboard-admin-reset.sh`
- `scripts/relix-mesh-up.ps1`
- `scripts/relix-mesh-up.sh`
- `crates/relix-cli/src/setup.rs`
- `crates/relix-cli/src/doctor.rs`

### Required Work

1. Make first-run admin creation explicit.
2. Make password reset obvious through CLI/script and dashboard docs.
3. Ensure dashboard session auth is consistently used for all dashboard APIs.
4. Keep bridge bearer token for API/automation if needed, but do not make normal dashboard users paste it repeatedly.
5. Add a dashboard health card that shows:
   - bridge reachable;
   - spine reachable;
   - coordinator reachable;
   - adapters reachable;
   - auth status;
   - current Guild/tenant.
6. Every failed API call should show a human-readable error and exact next step.
7. `relix boot` output should print:
   - dashboard URL;
   - admin state;
   - reset command;
   - token file path only if relevant;
   - how to stop mesh safely.

### Acceptance Criteria

- Fresh checkout: `cargo build --workspace` succeeds.
- `relix boot` starts the mesh.
- Opening `/dashboard` shows login or first-run setup, not random API errors.
- After login, dashboard loads Overview, Briefs, Runs, Crew, Chat, Settings without 401.
- Forgetting password has a documented one-command reset.
- Dashboard health says exactly what is wrong when coordinator/AI/tool is offline.

### Tests/Verification

- Rust tests for auth/session endpoints.
- Bridge mini-mesh smoke for dashboard API auth.
- Playwright dashboard login smoke.
- Manual Windows PowerShell boot smoke.

## Workstream B: Collapse To One Dashboard

### Goal

Make the React dashboard the product. Stop spreading product behavior across three UI surfaces.

### Current Problems

- `apps/dashboard` is the intended dashboard.
- `dashboard.html` and `spine_dashboard.html` still exist.
- The user can open a surface that feels old.
- Generated dist can drift from source.

### Code Areas

- `apps/dashboard/**`
- `crates/relix-web-bridge/src/dashboard.rs`
- `crates/relix-web-bridge/src/dashboard.html`
- `crates/relix-web-bridge/src/spine_dashboard.html`
- `crates/relix-web-bridge/dashboard-dist/**`
- `crates/relix-web-bridge/src/main.rs`

### Required Work

1. Decide final route:
   - `/dashboard` is React.
   - `/spine` either redirects to React or becomes an internal fallback only.
   - legacy `dashboard.html` should not be the default product.
2. Move any useful legacy functionality into React.
3. Add build parity check:
   - React source build must match generated dist or CI fails.
4. Remove or quarantine legacy HTML only after parity.
5. Dashboard navigation should match Relix lexicon:
   - Command Center;
   - Desk;
   - Mandates;
   - Briefs;
   - Crew/Roster;
   - Lattice;
   - Runs/Shifts;
   - Clearances;
   - Allowance;
   - Settings.

### Acceptance Criteria

- User opening `/dashboard` always sees the real React product.
- No stale old dashboard by default.
- All important spine actions are available in React.
- Generated dist is rebuilt and checked.

## Workstream C: The Golden Product Loop

### Goal

The Founder can create a Mandate, create/assign Briefs, run an Operative, and inspect output without using CLI.

### Current Problems

- Many endpoints exist but are not woven into one guided flow.
- Some dashboard pages are lists, not workflow.
- Run errors/refusals may be understandable to engineers but not users.

### Code Areas

- `crates/relix-runtime/src/nodes/coordinator/spine/**`
- `crates/relix-runtime/src/nodes/coordinator/brief.rs`
- `crates/relix-runtime/src/nodes/coordinator/mod.rs`
- `crates/relix-runtime/src/nodes/coordinator/heartbeat.rs`
- `crates/relix-runtime/src/nodes/coordinator/agent/**`
- `crates/relix-web-bridge/src/spine.rs`
- `crates/relix-web-bridge/src/adapters.rs`
- `crates/relix-web-bridge/src/execution.rs`
- `apps/dashboard/src/pages/Mandates.tsx`
- `apps/dashboard/src/pages/Briefs.tsx`
- `apps/dashboard/src/pages/Agents.tsx`
- `apps/dashboard/src/pages/Runs.tsx`
- `apps/dashboard/src/pages/Overview.tsx`
- `apps/dashboard/src/api.ts`

### Required Work

1. Command Center should show:
   - current Guild;
   - Prime;
   - active Mandates;
   - active Briefs;
   - running Shifts;
   - blocked/stale/overdue Briefs;
   - pending Clearances;
   - adapter health.
2. Mandate page should support:
   - create Mandate;
   - view hierarchy;
   - propose/team-plan;
   - orchestrate into Briefs;
   - see generated/reused Briefs.
3. Brief page should support:
   - create Brief;
   - assign Operative;
   - set priority/status/due/reviewer;
   - comment;
   - add Dossier;
   - add Snag;
   - create Sub-brief;
   - run now;
   - show current Claim/Shift;
   - show Chronicle.
4. Runs page should support:
   - current running Shifts;
   - recent Shift history;
   - adapter/Rig;
   - workspace;
   - output summary;
   - errors/refusals;
   - retry/re-run.
5. Crew page should support:
   - create Operative;
   - pick Rig;
   - see readiness;
   - set Keys;
   - see Allowance;
   - pause/resume.

### Acceptance Criteria

- From empty local DB, user can:
  1. log in;
  2. create Mandate;
  3. create or generate Brief;
  4. assign to an Operative;
  5. run with echo Rig;
  6. see Shift result;
  7. comment;
  8. move Brief status.
- Same flow works with Codex/Claude when installed and logged in.
- If adapter unavailable, user sees exact reason and install/login hint.

## Workstream D: Execution Reliability And Recovery

### Goal

Make Shifts trustworthy. A run should not just start; it should have clear lifecycle, workspace, logs, retry, recovery, and final status.

### Current Problems

- Runtime audit says coordinator is durable ledger/product state, but not fully durable resumable execution.
- Comments mention mid-flow VM state and auto-relaunch not implemented.
- Relix has claim/run/heartbeat pieces but not full Paperclip-style liveness taxonomy.
- Workspace execution is safer now, but review/apply is not yet a complete product path.

### Code Areas

- `crates/relix-runtime/src/nodes/coordinator/heartbeat.rs`
- `crates/relix-runtime/src/nodes/coordinator/mod.rs`
- `crates/relix-runtime/src/nodes/coordinator/maintenance.rs`
- `crates/relix-runtime/src/rig/mod.rs`
- `crates/relix-runtime/src/rig/bridge.rs`
- `crates/relix-web-bridge/src/execution.rs`
- `crates/relix-web-bridge/src/task_recorder.rs`
- `apps/dashboard/src/pages/Runs.tsx`
- `apps/dashboard/src/components/MaintenancePanel.tsx`

### Required Work

1. Define Shift lifecycle states clearly:
   - queued;
   - preflight;
   - running;
   - succeeded;
   - failed;
   - refused;
   - interrupted;
   - timed_out;
   - cancelled;
   - waiting_for_clearance;
   - waiting_for_quota;
   - workspace_error.
2. Add clear refusal taxonomy:
   - unassigned;
   - no adapter;
   - adapter unavailable;
   - not authenticated;
   - already running;
   - workspace context failed;
   - permission denied.
3. Add recovery scan:
   - stale running Shift becomes interrupted;
   - claim released or marked;
   - dashboard shows recovery action.
4. Add retry rules:
   - transient adapter failure can retry;
   - quota reset can reschedule;
   - permission denied should not blindly retry;
   - workspace failure should not retry until config fixed.
5. Add visible run transcript/log view.
6. Add Review/Apply flow for workspace output:
   - show changed files in sandbox;
   - allow discard/apply;
   - never silently mutate live repo.

### Acceptance Criteria

- Killing a running adapter does not leave a Brief permanently stuck.
- Dashboard shows interrupted/retryable/non-retryable state.
- Workspace path is visible.
- User can inspect output before applying.
- Run never falls back to live repo unless explicit unsafe mode is configured.

## Workstream E: Rigs And Agent Adapters

### Goal

Make "plug in any agent" real and visible.

### Current Problems

- Rig contract exists.
- Claude/Codex probes and run parsing exist.
- Hermes is still a placeholder/stdout-style integration, not rich Tether.
- Thin adapter limits need product honesty.
- Subscription/quota/cost handling is incomplete.

### Code Areas

- `crates/relix-runtime/src/rig/**`
- `crates/relix-runtime/src/nodes/coordinator/heartbeat.rs`
- `crates/relix-web-bridge/src/adapters.rs`
- `apps/dashboard/src/pages/Settings.tsx`
- `apps/dashboard/src/pages/Agents.tsx`
- `apps/dashboard/src/pages/Runs.tsx`
- `docs/relix-agent-adapters.md`
- `docs/relix-hermes-integration.md`

### Required Work

1. Rig readiness should show:
   - installed/missing;
   - logged in/not logged in;
   - interactive-only;
   - unsupported version;
   - probe failed;
   - install/login hint.
2. Per-Operative Rig config:
   - Rig name;
   - model/profile;
   - subscription/API mode;
   - workspace context mode;
   - bridge-back support;
   - governance class: rich/box-level.
3. Claude:
   - noninteractive run;
   - stream-json parsing;
   - permission denial surfaced;
   - auth check.
4. Codex:
   - noninteractive run;
   - JSON parsing;
   - Windows sandbox caveat surfaced;
   - auth check.
5. Generic process/http adapters:
   - safe argv;
   - timeout/cancel;
   - redaction;
   - output cap;
   - clear failure states.
6. Hermes:
   - real Tether plugin path;
   - Relix bridge plugin;
   - per-tool governance where possible;
   - memory/skill loop mapping.

### Acceptance Criteria

- Dashboard says "Claude installed but not logged in" instead of "broken."
- Dashboard says "Codex available, logged in via ChatGPT" when true.
- A Brief can run on echo, Claude, or Codex.
- A thin adapter run clearly says governance is box-level.
- Hermes integration has a working end-to-end smoke or is labeled honestly as future.

## Workstream F: Prime, Team Planning, And Real Company Feel

### Goal

Make Relix feel like a company of agents, not a manually assigned task board.

### Current Problems

- Mandate team plan/orchestration exists in implementation map.
- Agent/Keys model exists.
- But the Prime flow is not yet the central user experience.

### Code Areas

- `crates/relix-runtime/src/nodes/coordinator/agent/handlers.rs`
- `crates/relix-runtime/src/nodes/coordinator/agent/store.rs`
- `crates/relix-runtime/src/nodes/coordinator/spine/**`
- `crates/relix-web-bridge/src/spine.rs`
- `apps/dashboard/src/pages/Mandates.tsx`
- `apps/dashboard/src/pages/Agents.tsx`
- `apps/dashboard/src/pages/Company.tsx`

### Required Work

1. Make Prime visible in dashboard.
2. Make the Lattice/org hierarchy visible.
3. Make team plan workflow:
   - Mandate -> Prime plan -> suggested hires/roles -> Clearances -> activated Crew.
4. Make orchestrate workflow:
   - Mandate -> generated Briefs -> assigned Crew -> dependencies/Snags.
5. Make Keys legible:
   - can spawn;
   - can assign;
   - can manage;
   - can configure;
   - secret allowlist;
   - instruction bundle.
6. Make pending hires/Clearances visible.
7. Make "this agent reports to this Lead" obvious.

### Acceptance Criteria

- Founder can create a Mandate and ask Prime to propose a team.
- Dashboard shows proposed hires and asks for Clearance.
- Approved hires appear in Crew/Roster.
- Prime can generate Briefs from Mandate without duplicate Brief spam.
- The user can see who owns each Brief.

## Workstream G: Chat Companion

### Goal

Make the dashboard feel conversational without losing the work-object spine.

### Current Problems

- `companion.rs` is rule-based.
- Chat exists, but product loop still depends on manual navigation.
- User wants Paperclip-style "start work by talking."

### Code Areas

- `crates/relix-web-bridge/src/companion.rs`
- `crates/relix-web-bridge/src/chat.rs`
- `crates/relix-web-bridge/src/openai.rs`
- `apps/dashboard/src/pages/Chat.tsx`
- `apps/dashboard/src/pages/Overview.tsx`
- `apps/dashboard/src/api.ts`

### Required Work

1. Chat companion should understand:
   - create Mandate;
   - create Brief;
   - assign Brief;
   - run Brief;
   - summarize status;
   - explain blockers;
   - show what needs Founder approval.
2. It should never silently mutate high-risk things.
3. It should use Clearances for agent creation, budget increases, risky runs.
4. It should link every action to created objects.
5. It should become a product guide, not a generic chat box.

### Acceptance Criteria

- User can type: "Build a landing page. Make a plan and assign it."
- Relix responds with Mandate/Brief plan and asks for approvals where needed.
- Every generated object is clickable.
- No hidden side effects.

## Workstream H: Review/Apply And Workspaces

### Goal

An agent can do real work safely, and the Founder can review it.

### Current Problems

- Run workspaces are now scoped and safer.
- `copy_repo` exists conceptually/currently documented.
- Product review/apply needs to become central.
- Prior disk-bloat incident proves this must be controlled.

### Code Areas

- `crates/relix-runtime/src/nodes/coordinator/heartbeat.rs`
- `crates/relix-runtime/src/nodes/tool/fs.rs`
- `crates/relix-web-bridge/src/workspaces.rs`
- `crates/relix-web-bridge/src/execution.rs`
- `apps/dashboard/src/pages/Runs.tsx`
- `apps/dashboard/src/components/MaintenancePanel.tsx`

### Required Work

1. Show workspace mode:
   - empty;
   - copy_repo;
   - future git_worktree.
2. Show workspace file count/bytes.
3. Show output files/changed files.
4. Let user inspect diffs.
5. Add apply/discard.
6. Add cleanup policy:
   - old run workspaces expire;
   - dashboard shows disk usage;
   - no hidden 150GB growth.
7. Add hard caps and warnings in UI.

### Acceptance Criteria

- Agent run cannot fill disk silently.
- Dashboard shows workspace storage.
- User can clean workspaces.
- User can review changed files before applying.

## Workstream I: CLI And SDK Operator Consistency

### Goal

The CLI should be a reliable operator tool, not a random set of endpoint mirrors.

### Current Problems

- CLI is broad but inconsistent.
- `ops.rs` is very large.
- Some bridge-backed commands do not use shared auth/token resolver.
- Raw 401/403 risk exists.

### Code Areas

- `crates/relix-cli/src/ops.rs`
- `crates/relix-cli/src/bridge_token.rs`
- `crates/relix-cli/src/doctor.rs`
- `crates/relix-cli/src/sessions.rs`
- `crates/relix-cli/src/export.rs`
- `sdks/python/**`
- `sdks/typescript/**`
- `crates/relix-sdk/**`

### Required Work

1. Centralize bridge HTTP client.
2. Centralize bearer/session resolution.
3. Convert raw 401/403 into friendly messages.
4. Split `ops.rs` gradually by command family.
5. Make CLI and dashboard names use lexicon consistently.
6. Ensure SDKs map auth/tenant/errors consistently.

### Acceptance Criteria

- CLI commands that hit dashboard/bridge work with the same auth model.
- `relix doctor` detects missing/invalid dashboard auth.
- No command fails with unexplained raw 401.

## Workstream J: Channels And Integrations

### Goal

Telegram/Slack/Discord should become real product channels, not just crates.

### Current Problems

- Telegram is strongest.
- Slack/Discord have useful pieces.
- Runnable channel-controller configs/examples are missing.
- No channel-specific end-to-end flow for inbound approvals/messages.

### Code Areas

- `crates/relix-telegram/**`
- `crates/relix-slack/**`
- `crates/relix-discord/**`
- `crates/relix-runtime/src/nodes/telegram/**`
- `crates/relix-runtime/src/nodes/slack/**`
- `crates/relix-runtime/src/nodes/discord/**`
- `configs/**`
- `docs/channels/**`
- `apps/dashboard/src/pages/Settings.tsx`

### Required Work

1. Add runnable config templates for each channel.
2. Add dashboard channel setup/status.
3. Add inbound message smoke tests.
4. Add approval button smoke tests.
5. Add replay/watermark safety where optional.
6. Add docs for how channels connect to Briefs/Clearances.

### Acceptance Criteria

- User can enable Telegram and see status in dashboard.
- Approval via Telegram works in a smoke path.
- Slack/Discord have same setup clarity or are marked experimental.

## Workstream K: Security And Production Hardening

### Goal

Move from alpha to credible beta.

### Current Problems

- Static peer discovery.
- No CRL/revocation gossip.
- manifest/signing limitations.
- FS jail TOCTOU caveat.
- tool policies may be broad.
- no fuzz coverage.
- log rotation/audit aggregation incomplete.
- bridge auth is loopback-grade unless proxied.

### Code Areas

- `crates/relix-runtime/src/manifest/**`
- `crates/relix-runtime/src/transport/**`
- `crates/relix-runtime/src/nodes/tool/**`
- `crates/relix-core/src/policy.rs`
- `configs/policies/**`
- `deny.toml`
- `.github/workflows/**`
- `docs/security.md`
- `docs/current-limitations.md`

### Required Work

1. Tighten default policies for terminal/file/browser/tool users.
2. Add policy impact tests.
3. Enforce `cargo deny` in CI or update docs honestly.
4. Add fuzz targets for:
   - wire codec;
   - SSRF/url parser;
   - SOL parser;
   - policy parser.
5. Implement or explicitly defer signed manifests.
6. Implement or explicitly defer revocation.
7. Add log rotation/cleanup.
8. Add production readiness checklist.

### Acceptance Criteria

- CI status matches docs.
- Current limitations are not stale.
- Tool policies are least-privilege by default.
- Security gaps have owners and gates.

## Workstream L: Dashboard Product Polish

### Goal

Make Relix look and feel like a premium product.

### Current Problems

- React dashboard is real but reads as internal console.
- Too many tables/cards/banners.
- Not enough workflow guidance.
- Error handling can degrade into empties.
- No obvious visual QA loop.

### Code Areas

- `apps/dashboard/src/App.tsx`
- `apps/dashboard/src/components/Layout.tsx`
- `apps/dashboard/src/components/common.tsx`
- `apps/dashboard/src/pages/**`
- `apps/dashboard/src/styles.css`

### Required Work

1. Build a strong shell:
   - left nav;
   - top status;
   - active Guild;
   - health;
   - current user;
   - command/action button.
2. Make Command Center the first screen.
3. Make Brief detail the center of work:
   - conversation;
   - properties;
   - runs;
   - dossiers;
   - clearances;
   - timeline.
4. Make lists dense but polished.
5. Use consistent icons and actions.
6. Avoid marketing hero UI.
7. Add responsive desktop/mobile checks.
8. Add Playwright screenshots for important states.

### Acceptance Criteria

- Dashboard feels like the product, not a debug UI.
- The user knows the next action on every page.
- No text overlap.
- No silent empty states.
- Desktop and mobile screenshots pass visual sanity.

## Phase Roadmap

### Phase 0: Audit And Alignment

Status: complete.

Done:

- Paperclip strict read complete.
- Relix strict read complete.
- Plan file created.

### Phase 1: Make Relix Usable Every Time

Priority: highest.

Goal:

```text
cargo build -> relix boot -> dashboard login -> health green -> create/run echo Brief
```

Tasks:

1. Fix/verify dashboard auth/session across every dashboard API.
2. Add first-run/reset clarity.
3. Add health diagnostics.
4. Add no-silent-empty API handling.
5. Add echo Rig golden-path smoke.
6. Add Playwright dashboard smoke.

Exit:

- Anshul can boot Relix and run one Brief without CLI hacking.

### Phase 2: Complete The Product Spine UI

Goal:

```text
Mandate -> Brief -> Crew -> Shift -> Chronicle
```

Tasks:

1. Make Mandates page workflow-based.
2. Make Brief detail page complete.
3. Make Crew/Roster useful.
4. Make Runs page useful.
5. Move legacy `/spine` functionality into React.
6. Redirect/quarantine legacy dashboard surfaces.

Exit:

- User can operate a small Guild from React dashboard only.

### Phase 3: Make Real Agent Execution Trustworthy

Goal:

```text
Brief assigned to Claude/Codex -> run in scoped workspace -> output visible -> retry/recover/apply
```

Tasks:

1. Polish Rig readiness.
2. Polish Claude/Codex run states.
3. Add run transcript/log detail.
4. Add recovery/liveness taxonomy.
5. Add workspace review/apply/discard.
6. Add workspace cleanup/disk usage.

Exit:

- Real Claude/Codex runs are understandable, safe, and recoverable.

### Phase 4: Make The Company Model Feel Alive

Goal:

```text
Founder -> Prime -> team plan -> hires -> assignments -> delegation
```

Tasks:

1. Prime visible.
2. Lattice/org chart visible.
3. Team plan workflow.
4. Spawn Clearances visible.
5. Keys/Allowances visible and editable.
6. Companion can create Mandates/Briefs and request Clearances.

Exit:

- Relix feels like a governed company of Operatives, not a task tool.

### Phase 5: Hermes And Rich Adapter Depth

Goal:

```text
Hermes as a rich Rig with Tether-level governance
```

Tasks:

1. Implement real Hermes Tether path.
2. Add per-tool governance where Hermes exposes it.
3. Map Hermes memory/skills to Relix Memory/Tradecraft.
4. Add Hermes dashboard status and run flow.

Exit:

- Hermes is not just "another process"; it is the deep integrated brain.

### Phase 6: Production Safety

Goal:

```text
credible beta for local/private deployments
```

Tasks:

1. Policy tightening.
2. CI/docs sync.
3. cargo-deny enforcement or honest deferral.
4. fuzz targets.
5. log rotation.
6. revocation/manifest decisions.
7. production checklist.

Exit:

- Known alpha risks are either closed or deliberately gated.

### Phase 7: Product Polish

Goal:

```text
Paperclip-class dashboard feel
```

Tasks:

1. Product-grade visual system.
2. Strong empty/error/loading states.
3. Workflow-first pages.
4. Mobile/desktop visual QA.
5. Reduce language drift.
6. Make the first hour delightful.

Exit:

- Relix finally feels like a product when opened.

## What To Tell Claude Next

Use this structure for future prompts:

```text
You are working in D:\DATA\WORK\OpenPrem\Apps\Relix.
Work only on main. Never create a branch.
Read `.relix-audit/RELIX_TO_FINAL_PRODUCT_PLAN.md`,
`.relix-audit/RELIX_CODEBASE_AUDIT_LOG.md`,
and the six design docs before editing.

Implement Phase <N>, slice <specific slice>.
Do not work outside the slice.
Do not invent new product concepts.
Do not touch legacy dashboard except to migrate/redirect/quarantine it.
Make changes directly, validate them, commit and push if instructed.
Final report must include changed files, tests run, what works now,
what still does not, and any user-visible workflow.
```

## The Immediate Next Slice

The best next implementation slice is Phase 1:

> Make boot/login/dashboard/API health reliable and make one echo Brief run work from the React dashboard.

Why this first:

- It addresses Anshul's actual pain.
- It creates trust.
- It forces auth, bridge, spine, dashboard, and execution to agree.
- It gives every later feature a reliable test harness.

Concrete Phase 1 task list:

1. Audit all `apps/dashboard/src/api.ts` calls and make auth/session consistent.
2. Ensure every dashboard API call shows explicit error state.
3. Add `/dashboard` health/diagnostic panel.
4. Verify first-run admin setup and reset scripts.
5. Add "Create sample Brief" or guided first Brief action.
6. Add "Run with echo Rig" action.
7. Show Shift result in Runs and Brief thread.
8. Add Playwright smoke:
   - login;
   - load dashboard;
   - create Brief;
   - run echo;
   - see Chronicle/result.
9. Build React dashboard and ensure bridge dist is updated.
10. Run focused Rust tests for bridge/spine/run path.

If Phase 1 is not done, do not start dashboard beautification. A beautiful broken dashboard will make the user even angrier.

## Simple Progress Model

Use this truth model when estimating:

- Backend/substrate: strong, roughly 70-80 percent for alpha/beta needs.
- Product spine backend: medium-strong, roughly 60-70 percent.
- Rig execution: medium, roughly 50-65 percent depending on adapter.
- Dashboard/product loop: weak-medium, roughly 30-40 percent.
- Production polish: medium-low, roughly 35-45 percent.
- Overall product: roughly 45-55 percent, because the visible product loop is the bottleneck.

This is not a failure. It means we built the hard engine first. Now we must build the cockpit.
