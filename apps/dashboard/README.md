# Relix dashboard

The Relix operator console — a Vite + React + TypeScript single-page app
served by the web bridge at **`/dashboard`**. This is the **canonical and
only** dashboard.

The legacy single-file `crates/relix-web-bridge/src/dashboard.html` console
was **deleted** (Phase 2 Slice 3). There is no HTML fallback: if the React
bundle is missing, `/dashboard` returns an honest **HTTP 503 "dashboard not
built"** notice telling you to run the build — never an old console.

> **Canonical artifact:** the committed `crates/relix-web-bridge/dashboard-dist/`
> IS the dashboard the bridge ships. Whenever you change anything under
> `apps/dashboard/src`, you MUST re-run `npm run build` and commit the
> regenerated `dashboard-dist/` in the same change.
>
> **Two guards enforce this:**
> 1. A Rust test
>    (`dashboard::committed_react_dist_present_and_index_references_existing_assets`)
>    fails `cargo test` if `index.html` points at a bundle asset that isn't
>    committed.
> 2. A **dist-parity gate** rebuilds the dashboard and fails if the committed
>    `dashboard-dist/` drifts from a fresh build:
>    - locally: `pwsh -File scripts/check-dashboard-dist.ps1` (also part of
>      `scripts/ci-local.ps1`);
>    - in CI: the `dashboard dist parity` job in `.github/workflows/ci.yml`
>      (runs on every push to `main` and every PR).
>
> So a stale committed bundle is caught before it can serve old UI in
> production.

## How it is served

`npm run build` emits the production bundle straight into
`crates/relix-web-bridge/dashboard-dist/` (configured via `vite.config.ts`
`build.outDir`). At boot the bridge's `dashboard::resolve_spa_dir()`
discovers that directory and serves it as static assets with an SPA
history fallback to `index.html`. The built bundle is committed, so
`cargo build` + `relix` boot serve the real app with no extra step.

- Override the bundle location with `RELIX_DASHBOARD_DIST=/path/to/dist`.
- If no bundle is found, the bridge serves an honest HTTP 503 notice
  ("dashboard not built — run `npm run build`"), not a legacy page.

The app is built with Vite `base: "/dashboard/"` and
`modulePreload.polyfill = false`, so it has **no inline scripts** and
loads cleanly under the bridge's strict default CSP (`script-src 'self'`).

## Auth

The dashboard never handles a bearer token. It logs in with a
username/password (first-run setup creates the admin; Argon2id hash on
the bridge) and rides an HTTP-only `relix_session` cookie. Every API
call uses `credentials: "include"`, and the bridge auth middleware admits
a valid session cookie. Endpoints: `/v1/auth/{status,setup,login,logout,me}`.

## Develop

```sh
cd apps/dashboard
npm install
npm run dev      # Vite dev server on :5273, proxies /v1 -> 127.0.0.1:19791
```

Run a bridge locally (`relix` / the web bridge on its default port) so the
dev server's proxy reaches the real APIs.

## Build (the whole pipeline)

```sh
cd apps/dashboard
npm install      # first time only
npm run build    # -> crates/relix-web-bridge/dashboard-dist/
```

Then rebuild/boot the bridge as usual. Re-run `npm run build` and commit
the regenerated `dashboard-dist/` whenever the UI changes.

## Tests

```sh
cd apps/dashboard
npm test         # node:test — pure helpers + a render/DOM verification
```

Most tests are framework-free assertions over the pure derivations in
`src/*.ts` (run with `node --test --experimental-strip-types`). One harness,
`test/render-interrupted.test.mjs`, additionally proves the **interrupted
orchestration UX actually renders** (RELUX_MASTER_PLAN Sec 15) — the failure a
user hits after a server restart, which a pure-function test cannot catch. It
adds **no new dependencies** and needs **no browser**:

- **Render path.** It transpiles the real `OrchestrationRow` with the esbuild
  already vendored by Vite, then server-renders it through `react-dom/server`
  + react-router's `StaticRouter` (both already present). It asserts a
  reconstructed-`interrupted` job renders the "Run interrupted — no live worker"
  callout, the durable progress, and a **Continue** button — and that a planned
  plan or a live running job does **not** show that callout. So a regression
  that hides the callout, drops Continue, or shows it for the wrong state fails
  here.
- **Shipped-bundle path.** It reads the committed bundle the kernel actually
  serves (`crates/relix-web-bridge/dashboard-dist`) and asserts the `index.html`
  asset wiring is intact and the JS bundle still carries the callout copy — so a
  **blank/broken dashboard** (broken asset refs) or a **stale dist** (source
  changed, bundle not rebuilt) is caught in the artifact that ships.

The render path catches **source** regressions; the shipped-bundle path catches
a **stale committed bundle** — complementary to the dist-parity gate below
(which rebuilds and diffs the whole bundle).

**Why no live-browser click smoke.** The one link the render + bundle paths do
not exercise is the actual browser binding from the **Continue** button's
`onClick` to the resume request. Closing it honestly needs a real browser
(Playwright/Puppeteer) — a 100s-of-MB engine download — or a `jsdom`-class DOM
shim that still would not drive the real kernel over the network, only a
half-measure that adds a dependency. Neither is worth it here: the resume API
itself is already proven end-to-end against a live kernel by the unit test
`a_second_job_resumes_only_pending_briefs_and_preserves_completed_runs` and the
`scripts/smoke-orchestration-resume.ps1` / `-restart.ps1` smokes, and the button
that triggers it is proven to render (and ship) by this harness. The remaining
gap is a single one-line event binding, not worth a browser toolchain. If a
live-DOM smoke is ever wanted, it should reuse an already-present engine, not
commit a browser binary.

### Per-brief recorded duration

`OrchestrationRow` surfaces each brief's **recorded** run duration next to its
round (RELUX_MASTER_PLAN Sec 15: the view shows "real, already-recorded per-brief
start/finish/round"). `stepDurationLabel` in `src/orchestration.ts` derives it
purely from the kernel's `started_at`/`finished_at` stamps and reuses the run
view's single duration formatter, so timings read identically across the
dashboard. It is honest by construction: a brief that started but has not
finished shows **no** duration (no fabricated live timer), and an unparseable or
backwards pair shows nothing rather than a wrong number. Pinned by
`stepDurationLabel*` tests in `test/orchestration.test.ts`.

### Run-detail deep links (in-shell)

A run's detail belongs to the **Relux Work surface**, not the legacy `/runs`
console. So a deep link to a run is **`/work?run=<run_id>`** — the same
query-param style the Work page already uses for `?agentId` / `?status`. The
Work page reads that param (`runIdFromSearch` in `src/routing.ts`) and opens that
run's detail panel; the param is the **single source of truth**, so a deep link
and browser **back / forward / refresh** all restore the same view (and selecting
or closing a run just updates the URL). A run that no longer exists degrades
honestly — the panel shows a "could not load run `<id>`" notice with a Close path,
never a blank page or a fake link.

This is what an orchestration step's `run_id` links to: `OrchestrationRow` renders
a completed/failed brief's `run_id` as a link built by `workRunHref(run_id)`
(`/work?run=<id>`), keeping the operator inside the Relux shell instead of hopping
to the bridge-gated legacy console. A step that produced **no** run shows no link
(never a fabricated one). The link construction/parse are pinned by `workRunHref*`
/ `runIdFromSearch*` tests in `test/routing.test.ts`, and the shipped bundle is
asserted to carry the `/work?run=` literal in `test/render-interrupted.test.mjs`.

Every in-shell run reference now resolves to this one surface. **Retry lineage**
on the Work page is navigable: a run's `retried_from` parent (Recent Runs row and
the Run Detail "Retry of" field) is a `/work?run=<parent>` link — the parent is in
the **same** Relux ledger (`/v1/relux/runs`), so inspecting it stays in-shell. The
Run Detail header carries a **Copy link** button that copies the absolute,
copy-paste-able URL `workRunShareUrl(run_id, origin)` →
`<origin>/dashboard/work?run=<id>` (the SPA's `/dashboard` basename on top of the
in-shell href); if the clipboard API is unavailable the URL is surfaced inline to
copy by hand, never a silent failure. The URL builder is pinned by
`workRunShareUrl*` tests (round-tripping through `runIdFromSearch`) and the shipped
bundle is asserted to carry the **Copy link** affordance.

> The legacy `/runs` console is a **separate** run ledger (`/v1/runs`, the bridge's
> `brief_runs`) whose ids do **not** exist on relux-kernel. Its run links therefore
> stay on `/runs`; they are *not* rewritten to `/work?run=` (that would 404 on the
> Relux backend). Only relux-kernel runs route to the in-shell Work surface.

## Mesh policy

The dashboard's board / inbox / crew / runs pages call the coordinator's
product-spine capabilities (`brief.*`, `mandate.*`, `agent.roster_summary`,
`task.recent_events`, `task.stuck`). The mesh runs default-deny, so the
boot scripts (`scripts/relix-mesh-up.ps1` / `.sh`) grant the `web-bridge`
caller (`chat-users` group) allow-rules for those methods when they
generate the run policy. Per-agent Key gates (tenant / manage / assign)
still apply inside each capability — the policy rule only lifts the mesh
default-deny so the operator console can reach the spine. If you run a
custom policy, add the same `[[rules]]` (see the boot script) or the
spine pages will return 502 (`default_deny`).

## Agent execution (adapters / Rigs)

An Operative executes a Brief through a local **agent adapter** (a Rig):
a coding-agent CLI such as **Claude CLI** or **Codex CLI**, run on the
operator's own subscription (no API key injected). The dashboard surfaces
this end-to-end:

- **Settings → Agent adapters** and **Crew** show every registered
  adapter with a *live availability probe* (installed / missing + an
  install hint). Nothing assumes a CLI exists.
- **Briefs → Run** (`POST /v1/spine/briefs/:id/run`) runs the Brief now
  through its Operative's adapter and returns a structured RunReport.
  Refusals are explicit (`unassigned`, `no_adapter`, `adapter_unavailable`,
  `already_running`) — never a faked run.
- **Active Runs** lists the run lifecycle from the Brief Chronicle
  (`brief.run_started` → `brief.shift_done` / `dispatch_failed`), tagged
  with the adapter that handled it.

Backend: the adapter abstraction (`ProcessRig`) spawns the CLI with safe
argv construction (no shell), a hard timeout + child-kill (cancellation),
streamed output capture, a validated working directory, and secret
redaction on captured output. The optional `RELIX_DEFAULT_RIG` env sets a
Guild-default adapter so an Operative with no Rig of its own still runs;
`RELIX_HEARTBEAT_ENABLED` turns on the autonomous timer dispatch.

## Known backend gaps (UI degrades, not faked)

- **List-all mandates**: there is no list endpoint — only
  `mandate.search` (requires a non-empty query). The Company page shows a
  search box and only loads mandates on search.
- **Crew list when empty**: `/v1/spine/roster` is a count summary
  (`{active,total}`); the Operative *list* is `/v1/agents/access`. With no
  hired crew both are empty and the pages show empty states.

## Stack

- React 18 + react-router-dom 6
- Vite 5 + TypeScript 5 (strict)
- No UI framework — a small hand-written B&W design system in
  `src/styles.css`.
