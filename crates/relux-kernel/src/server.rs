//! Local HTTP API for the dashboard Plugins tab (`relux-kernel serve`).
//!
//! This is the web seam in front of the durable plugin-install lifecycle
//! (`docs/RELUX_MASTER_PLAN.md` section 11.6 Plugins, section 7.4 Plugin Kernel
//! Layer). It exposes the exact operations the CLI already supports - list,
//! install-from-folder, install-from-GitHub, install-from-zip, remove - over a
//! small JSON API the dashboard can call so a plugin "stays installed until
//! removed" and the Plugins tab feels connected.
//!
//! Everything is local-only and conservative by construction:
//!
//! - It binds loopback (`127.0.0.1:19891` by default, `RELUX_HTTP_ADDR` to
//!   override) and shares the same persisted store path and plugins root as the
//!   CLI, so the API and the CLI see one durable control plane.
//! - All install/remove safety lives in [`crate::plugin_install`] (manifest
//!   validation, id sandboxing, zip-traversal rejection, bundled-plugin
//!   protection); this layer only routes to it.
//! - A single process-wide mutex serializes every load/modify/save so concurrent
//!   requests cannot interleave a snapshot. MVP-correct, not clever.
//! - Errors are mapped to honest HTTP status codes + a `{ "error": ... }` JSON
//!   body; a handler never panics on bad input. No secrets are returned.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use relux_core::{
    InstalledPlugin, Orchestration, OrchestrationBatchResult, OrchestrationId, OrchestrationPlan,
    OrchestrationStatus, PluginManifest, PluginSourceKind, PrimeAutonomyConfig,
    PrimeAutonomyTickResult, PrimeTurn, StepOutcome, ToolInvocationResult,
};
use relux_kernel::{
    install_from_dir, install_from_github, install_from_zip, remove_plugin, AiConfig, AiMode,
    AiOutcome, AiStatus, KernelError, KernelState, SqliteStore,
};

/// The default loopback bind address; override with `RELUX_HTTP_ADDR`.
const DEFAULT_ADDR: &str = "127.0.0.1:19891";

/// Cap an uploaded zip at 64 MiB so a stray large upload is refused cleanly
/// rather than buffered without bound. Plugin archives are tiny in practice.
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Shared, cloneable handler state. The mutex serializes every store
/// load/modify/save so concurrent requests can't interleave snapshots.
#[derive(Clone)]
struct AppState {
    db_path: PathBuf,
    plugins_root: PathBuf,
    uploads_root: PathBuf,
    /// The resolved dashboard bundle directory, or `None` when no bundle was
    /// built (a source-only checkout). `None` makes every dashboard route serve
    /// the honest missing-bundle notice instead of panicking.
    dashboard_dir: Option<PathBuf>,
    /// The dashboard-written AI provider secrets file (gitignored). Resolved live
    /// per request so a key set from the dashboard takes effect without a restart.
    ai_config_path: PathBuf,
    /// Local operator login: the durable Argon2id admin credential + in-memory
    /// session table. The auth middleware admits a request carrying a valid
    /// `relux_session` cookie; `/v1/auth/*` mint/clear it. See [`crate`] auth.
    dashboard_auth: relux_kernel::DashboardAuth,
    /// Per-agent access tokens: the first per-agent auth identity. The operator mints
    /// a bounded, hashed-at-rest, revocable token for a specific agent; a request
    /// carrying it (as `Authorization: Bearer <token>`) is authenticated AS that agent
    /// and admitted ONLY on the tiny agent-self route subset (`/v1/relux/agents/me*`)
    /// — never the operator console. See [`crate::agent_auth`] and
    /// `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §19.
    agent_tokens: relux_kernel::AgentTokenStore,
    /// Dev/test-only escape hatch: when `RELUX_AUTH_DISABLED` is truthy the auth
    /// middleware passes every request through (a loud warning is printed at
    /// startup). OFF by default — production/normal use always enforces login.
    /// NOTE: this bypass covers the OPERATOR session middleware only; the agent-token
    /// middleware never bypasses (an agent's identity must come from a real token).
    auth_disabled: bool,
    lock: Arc<Mutex<()>>,
    /// In-process registry of non-blocking orchestration jobs. Lives only for the
    /// life of the server process: a restart honestly loses in-flight job records
    /// (the durable orchestration record still carries the real per-brief progress
    /// recorded round-by-round). See [`JobRegistry`].
    jobs: JobRegistry,
    /// In-process registry of LIVE run-log buffers for in-flight adapter runs. The
    /// off-lock orchestration spawn streams stdout/stderr lines here as they are
    /// read, so `GET /v1/relux/runs/:id/logs` can show a tail BEFORE the run
    /// finalizes; once the canonical log is persisted the durable log wins and the
    /// live buffer is dropped. Independent of the kernel `lock`, so a live poll
    /// never blocks on a kernel operation. See [`relux_kernel::LiveRunLogs`] and
    /// `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10.
    live_run_logs: relux_kernel::LiveRunLogs,
    /// In-process registry of cancel tokens for in-flight, process-backed adapter
    /// runs. The off-lock orchestration spawn opens a token per brief; an operator
    /// `POST /v1/relux/runs/:id/cancel` sets the flag and the spawn kills its child
    /// mid-flight. Independent of the kernel `lock` (so a cancel is never blocked by
    /// a kernel operation), and bounded by its own backstop. Only an off-lock
    /// streaming run is cancellable — every other run honestly reports not-running.
    /// See [`relux_kernel::RunCancellations`] and `docs/HERMES_OPENCLAW_DEEP_AUDIT.md`
    /// §8/§26.
    run_cancellations: relux_kernel::RunCancellations,
}

/// Resolve the effective AI config from the local secrets file (when present)
/// with environment fallback. The key is never returned over the wire - only the
/// key-free [`AiStatus`].
fn resolve_ai(state: &AppState) -> AiConfig {
    AiConfig::resolve(Some(&state.ai_config_path))
}

/// Build the tokio runtime and run the API server until the process is killed.
///
/// This is the `relux-kernel serve` entry point. It bootstraps the durable store
/// once up front (so a fresh DB already lists the bundled example plugins) and
/// then serves the `/v1/relux` API on the configured loopback address.
pub fn run() -> Result<(), KernelError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| KernelError::Storage(format!("failed to start tokio runtime: {e}")))?;
    runtime.block_on(serve())
}

async fn serve() -> Result<(), KernelError> {
    let state = AppState {
        db_path: crate::db_path(),
        plugins_root: crate::plugins_root(),
        uploads_root: crate::uploads_root(),
        dashboard_dir: crate::dashboard::resolve_dist_dir(),
        ai_config_path: crate::ai_config_path(),
        dashboard_auth: relux_kernel::DashboardAuth::from_paths(
            &crate::admin_path(),
            &crate::session_path(),
        ),
        agent_tokens: relux_kernel::AgentTokenStore::from_path(&crate::agent_tokens_path()),
        auth_disabled: auth_disabled_from_env(),
        lock: Arc::new(Mutex::new(())),
        jobs: JobRegistry::default(),
        live_run_logs: relux_kernel::LiveRunLogs::new(),
        run_cancellations: relux_kernel::RunCancellations::new(),
    };

    // Bootstrap + persist once so a fresh store already lists the bundled
    // example plugins before the first request arrives.
    locked_save(&state, |_kernel| Ok(()))
        .map_err(|e| KernelError::Storage(format!("bootstrap failed: {}", e.message)))?;

    let addr = bind_addr()?;
    let dashboard_missing = state.dashboard_dir.is_none();
    let app = router(state.clone()); // Clone state for the background task

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => return Err(KernelError::ServeBind(bind_failure_message(addr, &e))),
    };
    let bound = listener
        .local_addr()
        .map_err(|e| KernelError::Storage(format!("failed to read bound address: {e}")))?;

    println!("relux-kernel serve: Relux local control plane is up.");
    println!();
    println!("   Relux dashboard: http://{bound}/dashboard");
    println!("   Relux API:       http://{bound}/v1/relux/state");
    println!();
    // Honest login status so the operator knows what the first dashboard load
    // will show: a first-run setup form, or the sign-in form.
    if state.auth_disabled {
        println!("   !! AUTH DISABLED (RELUX_AUTH_DISABLED set): the dashboard/API are OPEN.");
        println!("      This is a dev/test escape hatch ONLY. Unset it for normal use.");
    } else if state.dashboard_auth.admin_exists() {
        let who = state
            .dashboard_auth
            .admin_username()
            .unwrap_or_else(|| "admin".to_string());
        println!("   login:  sign in as '{who}' on the dashboard (session cookie, no token paste).");
    } else {
        println!("   login:  first run — open the dashboard to set your admin username + password.");
    }
    println!();
    if dashboard_missing {
        println!(
            "   note: the dashboard bundle is not built; /dashboard will return a build notice."
        );
        println!("         build it with `npm run build` in apps/dashboard, then reload.");
        println!();
    }
    println!("   db      {}", crate::db_path().display());
    println!("   plugins {}", crate::plugins_root().display());
    println!("   GET    /dashboard                          (standalone Relux shell)");
    println!("   GET    /v1/auth/status                     (public: needs_setup / authenticated)");
    println!("   POST   /v1/auth/setup                      {{ \"username\":\"...\", \"password\":\"...\" }} (first run only)");
    println!("   POST   /v1/auth/login                      {{ \"username\":\"...\", \"password\":\"...\" }}");
    println!("   POST   /v1/auth/logout");
    println!("   GET    /v1/auth/me                         (current session user)");
    println!("   POST   /v1/auth/change-password            {{ \"current_password\":\"...\", \"new_password\":\"...\" }} (requires session)");
    println!("   GET    /v1/relux/health                    (public liveness; no session required)");
    println!("   GET    /v1/relux/state");
    println!("   GET    /v1/relux/doctor                     (read-only structured diagnostics report)");
    println!("   GET    /v1/relux/ai/status");
    println!("   PUT    /v1/relux/ai/config                 {{ \"provider\":\"openrouter\", \"api_key\":\"...\", \"model\"?, \"disabled\"? }}");
    println!("   DELETE /v1/relux/ai/config                 (clear the stored AI key/config)");
    println!("   GET    /v1/relux/tasks");
    println!("   GET    /v1/relux/tasks/:id");
    println!("   GET    /v1/relux/runs");
    println!("   GET    /v1/relux/runs/:id");
    println!("   GET    /v1/relux/runs/:id/events            (optional ?since=<event_id> tail)");
    println!("   GET    /v1/relux/runs/:id/logs              (bounded redacted stdout/stderr/system tail; optional ?since=<seq>)");
    println!("   POST   /v1/relux/runs/:id/retry");
    println!("   POST   /v1/relux/runs/:id/resume            (continue the captured provider session; 422 if unsupported)");
    println!("   POST   /v1/relux/runs/:id/cancel            (cancel an in-flight process run; honest not-running otherwise)");
    println!("   POST   /v1/relux/runs/:id/proposed-changes/:index/review {{ \"decision\": \"approve|reject\" }}");
    println!("   POST   /v1/relux/runs/:id/proposed-changes/:index/apply");
    println!("   GET    /v1/relux/audit");
    println!("   GET    /v1/relux/health");
    println!("   POST   /v1/relux/prime                     {{ \"message\": \"...\" }}");
    println!("   POST   /v1/relux/prime/reset               (clear this conversation's bounded memory)");
    println!("   POST   /v1/relux/tasks                     {{ \"title\": \"...\" }}");
    println!("   POST   /v1/relux/tasks/:id/start");
    println!("   POST   /v1/relux/tasks/:id/execute-assigned");
    println!("   GET    /v1/relux/tools                      (installed tools + executable status)");
    println!("   POST   /v1/relux/tools/invoke              {{ \"plugin_id\":\"...\", \"tool_name\":\"...\", \"input\":{{}} }}");
    println!("   POST   /v1/relux/tools/request-approval    {{ \"plugin_id\":\"...\", \"tool_name\":\"...\", \"input\":{{}} }} (per-call approval for a gated tool)");
    println!("   POST   /v1/relux/approvals/:id/execute     (run an approved per-call tool invocation once)");
    println!("   POST   /v1/relux/approvals/:id/allow-always (approve + persist a standing allow-always grant)");
    println!("   GET    /v1/relux/grants                    (persistent allow-always grants)");
    println!("   POST   /v1/relux/grants                    {{ \"plugin_id\":\"...\", \"tool_name\":\"...\", \"agent_id\"? }}");
    println!("   DELETE /v1/relux/grants/:id                (revoke a persistent grant)");
    println!("   GET    /v1/relux/plugins");
    println!("   POST   /v1/relux/plugins/install-github   {{ \"url\": \"https://github.com/...\" }}");
    println!("   POST   /v1/relux/plugins/install-zip      (multipart field: file)");
    println!("   GET    /v1/relux/plugins/:id/runtime      (HTTP loopback runtime status)");
    println!("   PUT    /v1/relux/plugins/:id/runtime      {{ \"base_url\":\"http://127.0.0.1:<port>\", \"enabled\"?, \"timeout_ms\"? }}");
    println!("   DELETE /v1/relux/plugins/:id/runtime      (clear runtime config)");
    println!("   GET    /v1/relux/plugins/:id/manifest-template  (starter relux-plugin.json)");
    println!("   POST   /v1/relux/plugins/:id/tools         {{ \"name\":\"report.fetch\", \"description\"?, \"risk\"?, \"auto_approve\"?, \"timeout_secs\"? }}");
    println!("   DELETE /v1/relux/plugins/:id/tools/:tool   (remove a configured tool)");
    println!("   DELETE /v1/relux/plugins/:id");
    println!("   GET    /v1/relux/adapters                  (adapter plugins + CLI runtime status)");
    println!("   POST   /v1/relux/prime/orchestrations/:id/run-async  (start a background job; returns job + status_url)");
    println!("   GET    /v1/relux/prime/orchestrations/:id/job         (latest job for this orchestration)");
    println!("   GET    /v1/relux/orchestration-jobs/:job_id           (poll one job's status)");
    println!("   POST   /v1/relux/orchestration-jobs/:job_id/cancel    (request cancellation; stops before the next round)");
    println!("   GET    /v1/relux/adapters/:id/runtime");
    println!("   PUT    /v1/relux/adapters/:id/runtime     {{ \"enabled\":true, \"command\"?, \"timeout_seconds\"?, \"max_output_bytes\"? }}");
    println!("   DELETE /v1/relux/adapters/:id/runtime     (clear adapter runtime config)");
    println!("   GET    /v1/relux/mcp/servers               (registered MCP servers — loopback HTTP discovery)");
    println!("   POST   /v1/relux/mcp/servers              {{ \"id\":\"...\", \"endpoint\":\"http://127.0.0.1:<port>/mcp\", \"description\"?, \"enabled\"?, \"timeout_ms\"? }}");
    println!("   DELETE /v1/relux/mcp/servers/:id           (remove an MCP server)");
    println!("   GET    /v1/relux/mcp/servers/:id/tools     (live tools/list discovery → ToolDescriptor[])");
    println!("   GET    /v1/relux/mcp/servers/:id/resources (live resources/list → McpResource[]; read-only context)");
    println!("   GET    /v1/relux/mcp/servers/:id/resources/read?uri=…  (read one resource → shaped, redacted text)");
    println!("   PUT    /v1/relux/mcp/servers/:id/tools/:tool/classification  {{ \"risk\":\"low|medium|high|critical\", \"approval\":\"never|required\" }}");
    println!("   DELETE /v1/relux/mcp/servers/:id/tools/:tool/classification  (revert to the gated default)");
    println!("          (MCP tools invoke through /v1/relux/tools/invoke etc. with plugin_id \"mcp:<server>\")");

    // Start background autonomy loop
    let background_state = state.clone();
    tokio::spawn(async move {
        run_autonomy_loop(background_state).await;
    });

    axum::serve(listener, app)
        .await
        .map_err(|e| KernelError::Storage(format!("server error: {e}")))?;
    Ok(())
}

async fn run_autonomy_loop(state: AppState) {
    loop {
        let current_config = {
            let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
            let store = match SqliteStore::open(&state.db_path) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("ERROR: Autonomy loop failed to open store: {}", e);
                    None
                }
            };
            match store {
                Some(store) => match store.load() {
                    Ok(kernel) => Some(kernel.prime_autonomy_config.clone()),
                    Err(e) => {
                        eprintln!("ERROR: Autonomy loop failed to load kernel state: {}", e);
                        None
                    }
                },
                None => None,
            }
        };

        let sleep_seconds = current_config
            .as_ref()
            .map(|config| config.interval_seconds.clamp(5, 3600))
            .unwrap_or(60);

        if current_config.as_ref().is_some_and(|config| config.enabled) {
            println!("INFO: Running Prime autonomy tick...");
            match locked_save(&state, |kernel| Ok(kernel.one_autonomy_tick())) {
                Ok(result) => {
                    println!("INFO: Prime autonomy tick complete: {}", result.summary);
                    for reason in result.skipped_reasons {
                        println!("  - Skipped: {}", reason);
                    }
                }
                Err(e) => {
                    eprintln!("ERROR: Prime autonomy tick failed: {:?}", e);
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(sleep_seconds)).await;
    }
}

/// Build an actionable error message when `serve` cannot bind `addr`.
///
/// The common first-run failure is a port conflict: a second Relux, a leftover
/// process, or another app already on `19891`. Instead of a bare OS error, point
/// the operator at the documented override — `RELUX_HTTP_ADDR` for a source
/// checkout and `-Port` for the packaged `Start-Relux.ps1` bundle (see the
/// README "override the port if 19891 is taken" note and RELUX_MASTER_PLAN §22).
fn bind_failure_message(addr: SocketAddr, err: &std::io::Error) -> String {
    use std::io::ErrorKind;
    // Suggest a clearly-different port than the one that is busy.
    let alt = if addr.port() == 20000 { 20001 } else { 20000 };
    match err.kind() {
        ErrorKind::AddrInUse => format!(
            "cannot start Relux: {addr} is already in use.\n\
             \n\
             Most likely Relux is already running - open http://{addr}/dashboard to check.\n\
             To run on a different port, set RELUX_HTTP_ADDR before starting, e.g.\n    \
             PowerShell:  $env:RELUX_HTTP_ADDR='127.0.0.1:{alt}'; relux-kernel serve\n    \
             bash:        RELUX_HTTP_ADDR=127.0.0.1:{alt} relux-kernel serve\n\
             Packaged bundle:  .\\Start-Relux.ps1 -Port {alt}"
        ),
        ErrorKind::PermissionDenied => format!(
            "cannot start Relux: permission denied binding {addr}.\n\
             Choose a non-privileged port (>=1024) via RELUX_HTTP_ADDR, e.g. \
             RELUX_HTTP_ADDR=127.0.0.1:{alt} relux-kernel serve."
        ),
        ErrorKind::AddrNotAvailable => format!(
            "cannot start Relux: the address {addr} is not available on this machine.\n\
             Relux binds loopback by default; set RELUX_HTTP_ADDR to a valid local \
             address such as 127.0.0.1:{alt}."
        ),
        _ => format!("failed to bind {addr}: {err}"),
    }
}

/// Resolve the bind address from `RELUX_HTTP_ADDR`, falling back to loopback.
fn bind_addr() -> Result<SocketAddr, KernelError> {
    let raw = match std::env::var("RELUX_HTTP_ADDR") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_ADDR.to_string(),
    };
    raw.parse::<SocketAddr>()
        .map_err(|e| KernelError::Storage(format!("invalid RELUX_HTTP_ADDR {raw:?}: {e}")))
}

/// Assemble the full router with the shared state.
///
/// Two layers of routes:
///
/// - **Public** (no session required): the static dashboard shell (so the SPA can
///   always load and render the setup/login screen — never a blank page), the
///   `/v1/auth/*` login endpoints, and the `/v1/relux/health` liveness probe.
/// - **Protected** (`require_session`): every other `/v1/relux/*` control-plane
///   route. A request without a valid `relux_session` cookie gets an honest 401
///   (`needs_setup` is included so the dashboard can route to the right screen).
///
/// The dev/test escape hatch `RELUX_AUTH_DISABLED` short-circuits the middleware
/// (a loud startup warning is printed); it is OFF by default.
fn router(state: AppState) -> Router {
    let protected = protected_router().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        require_session,
    ));
    // The agent-token route subset rides a SEPARATE middleware (`require_agent_token`):
    // a request is authenticated AS an agent by a bearer token, not by the operator
    // session cookie. This is a deliberately tiny allowlist (`/v1/relux/agents/me*`) —
    // an agent token never reaches the operator console (those routes only ever check
    // the session cookie). The `me` static segment coexists with the protected `:id`
    // param routes (matchit gives static segments priority).
    let agent = agent_router().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        require_agent_token,
    ));
    public_router()
        .merge(protected)
        .merge(agent)
        // Bound the request body so a large zip upload is refused cleanly.
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(state)
}

/// Routes that never require a session: the static dashboard, the auth
/// endpoints, and the health probe.
fn public_router() -> Router<AppState> {
    Router::new()
        // Standalone Relux dashboard shell, served by the kernel itself.
        .route("/", get(root_redirect))
        .route("/dashboard", get(dashboard_index))
        .route("/dashboard/", get(dashboard_index))
        .route("/dashboard/*path", get(dashboard_path))
        // Local operator login (mints/clears the relux_session cookie). These are
        // intentionally public so an unauthenticated browser can reach the
        // setup/login forms; `me`/`logout` self-gate on the cookie.
        .route("/v1/auth/status", get(auth_status))
        .route("/v1/auth/setup", post(auth_setup))
        .route("/v1/auth/login", post(auth_login))
        .route("/v1/auth/logout", post(auth_logout))
        .route("/v1/auth/me", get(auth_me))
        // Liveness: no session required (a probe must work before login).
        .route("/v1/relux/health", get(get_health))
}

/// Every control-plane route that requires a valid session. The auth middleware
/// is attached by [`router`] via `route_layer`, so an unmatched path 404s
/// without running the guard.
fn protected_router() -> Router<AppState> {
    Router::new()
        // Authenticated password change (a valid session is required, hence it is
        // a protected route). Public /v1/auth/* covers setup/login/logout/me.
        .route("/v1/auth/change-password", post(auth_change_password))
        // The /v1/relux control-plane API the dashboard calls on the same origin.
        .route("/v1/relux/state", get(get_state))
        .route("/v1/relux/doctor", get(get_doctor))
        .route("/v1/relux/ai/status", get(get_ai_status))
        .route(
            "/v1/relux/ai/config",
            put(set_ai_config).patch(set_ai_config).delete(clear_ai_config),
        )
        .route("/v1/relux/agents", get(list_agents).post(create_agent))
        .route("/v1/relux/agent-presets", get(list_agent_presets))
        .route(
            "/v1/relux/agents/:id",
            put(update_agent).patch(update_agent),
        )
        .route("/v1/relux/prime", post(run_prime))
        .route("/v1/relux/prime/reset", post(reset_prime_conversation))
        .route("/v1/relux/prime/autonomy", get(get_autonomy_config).put(update_autonomy_config).patch(update_autonomy_config))
        .route("/v1/relux/prime/autonomy/tick", post(run_autonomy_tick))
        // Multi-agent orchestration (Prime as orchestrator).
        .route(
            "/v1/relux/prime/orchestrations",
            get(list_orchestrations).post(create_orchestration),
        )
        .route("/v1/relux/prime/orchestrate/preview", post(preview_orchestration))
        .route("/v1/relux/prime/orchestrations/:id", get(get_orchestration))
        .route(
            "/v1/relux/prime/orchestrations/:id/run",
            post(run_orchestration_batch),
        )
        // Non-blocking orchestration runs: start a background job and poll it.
        .route(
            "/v1/relux/prime/orchestrations/:id/run-async",
            post(start_orchestration_job),
        )
        .route(
            "/v1/relux/prime/orchestrations/:id/job",
            get(get_latest_orchestration_job),
        )
        .route(
            "/v1/relux/orchestration-jobs/:job_id",
            get(get_orchestration_job),
        )
        .route(
            "/v1/relux/orchestration-jobs/:job_id/cancel",
            post(cancel_orchestration_job),
        )
        .route("/v1/relux/tasks", get(list_tasks).post(create_task))
        .route("/v1/relux/tasks/:id", get(get_task))
        .route("/v1/relux/runs", get(list_runs))
        .route("/v1/relux/runs/:id", get(get_run))
        .route("/v1/relux/runs/:id/events", get(get_run_events))
        .route("/v1/relux/runs/:id/logs", get(get_run_logs))
        .route("/v1/relux/runs/:id/retry", post(retry_run))
        .route("/v1/relux/runs/:id/resume", post(resume_run))
        .route("/v1/relux/runs/:id/cancel", post(cancel_run))
        .route(
            "/v1/relux/runs/:id/proposed-changes/:index/review",
            post(review_proposed_change),
        )
        .route(
            "/v1/relux/runs/:id/proposed-changes/:index/apply",
            post(apply_proposed_change),
        )
        .route(
            "/v1/relux/runs/:id/proposed-changes/apply",
            post(apply_proposed_change_set),
        )
        .route("/v1/relux/audit", get(list_audit_events))
        .route("/v1/relux/tasks/:id/start", post(start_task))
        .route("/v1/relux/tasks/:id/execute-assigned", post(execute_assigned_task))
        .route("/v1/relux/tasks/:id/assign", post(assign_task_to_agent))
        .route("/v1/relux/tools", get(list_tools))
        .route("/v1/relux/tools/invoke", post(invoke_tool))
        .route(
            "/v1/relux/tools/request-approval",
            post(request_tool_invocation_approval),
        )
        .route("/v1/relux/plugins", get(list_plugins))
        .route("/v1/relux/plugins/install-dir", post(install_dir))
        .route("/v1/relux/plugins/install-github", post(install_github))
        .route("/v1/relux/plugins/install-zip", post(install_zip))
        .route(
            "/v1/relux/plugins/:id/runtime",
            get(get_plugin_runtime)
                .put(set_plugin_runtime)
                .patch(set_plugin_runtime)
                .delete(delete_plugin_runtime),
        )
        .route(
            "/v1/relux/plugins/:id/manifest-template",
            get(plugin_manifest_template),
        )
        // Operator-configured tool definitions for a user-installed ToolSet/wrapper.
        .route("/v1/relux/plugins/:id/tools", post(configure_plugin_tool))
        .route(
            "/v1/relux/plugins/:id/tools/:tool",
            delete(remove_plugin_tool),
        )
        .route("/v1/relux/plugins/:id", delete(remove))
        // Adapter runtime controls (local coding-agent CLIs).
        .route("/v1/relux/adapters", get(list_adapters))
        .route(
            "/v1/relux/adapters/:id/runtime",
            get(get_adapter_runtime)
                .put(set_adapter_runtime)
                .patch(set_adapter_runtime)
                .delete(delete_adapter_runtime),
        )
        // MCP servers (loopback HTTP discovery — MCP v1).
        .route(
            "/v1/relux/mcp/servers",
            get(list_mcp_servers).post(register_mcp_server),
        )
        .route("/v1/relux/mcp/servers/:id", delete(delete_mcp_server))
        .route("/v1/relux/mcp/servers/:id/tools", get(mcp_server_tools))
        // MCP resources — read-only context source (resources/list + resources/read).
        .route("/v1/relux/mcp/servers/:id/resources", get(mcp_server_resources))
        .route(
            "/v1/relux/mcp/servers/:id/resources/read",
            get(mcp_read_resource_route),
        )
        .route(
            "/v1/relux/mcp/servers/:id/tools/:tool/classification",
            put(set_mcp_tool_classification)
                .patch(set_mcp_tool_classification)
                .delete(clear_mcp_tool_classification),
        )
        // Relux Approvals and Permissions
        .route("/v1/relux/approvals", get(list_approvals))
        .route("/v1/relux/approvals/:id/decide", post(decide_approval))
        .route(
            "/v1/relux/approvals/:id/execute",
            post(execute_approved_tool_invocation),
        )
        .route(
            "/v1/relux/approvals/:id/allow-always",
            post(allow_always_from_approval),
        )
        // Persistent allow-always grants (list / create / revoke).
        .route(
            "/v1/relux/grants",
            get(list_persistent_grants).post(create_persistent_grant),
        )
        .route("/v1/relux/grants/:id", delete(revoke_persistent_grant))
        .route("/v1/relux/permissions", get(list_permissions))
        .route(
            "/v1/relux/agents/:id/permissions",
            post(grant_agent_permission).delete(revoke_agent_permission),
        )
        .route(
            "/v1/relux/agents/:id/manager-grant",
            post(manager_grant_to_subordinate),
        )
        // Per-agent access tokens (operator-only mint/list/revoke). The operator
        // console is the human authority that issues an agent its credential; the
        // raw token is returned exactly once at mint and never again.
        .route(
            "/v1/relux/agents/:id/tokens",
            get(list_agent_tokens).post(mint_agent_token),
        )
        .route(
            "/v1/relux/agents/:id/tokens/:token_id",
            delete(revoke_agent_token),
        )
}

/// The tiny route subset an agent may reach with its OWN access token (bearer auth
/// via [`require_agent_token`]). The acting agent is always the token's subject —
/// every handler reads its identity from the validated token, never from the path
/// or body, so a token can only ever act as itself. This is deliberately minimal:
/// agent self-info and the manager-grant-as-self path, nothing that touches the
/// operator console.
fn agent_router() -> Router<AppState> {
    Router::new()
        .route("/v1/relux/agents/me", get(agent_self_info))
        .route("/v1/relux/agents/me/manager-grant", post(agent_self_manager_grant))
        .route("/v1/relux/agents/me/assign-task", post(agent_self_assign_task))
        .route("/v1/relux/agents/me/manager-revoke", post(agent_self_manager_revoke))
}

/// Whether the dev/test auth bypass is requested via `RELUX_AUTH_DISABLED`.
/// Truthy values: `1`, `true`, `yes`, `on` (case-insensitive). Anything else —
/// including unset — keeps auth ENFORCED.
fn auth_disabled_from_env() -> bool {
    matches!(
        std::env::var("RELUX_AUTH_DISABLED")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Auth guard for the protected `/v1/relux/*` routes. Admits a request that
/// carries a valid `relux_session` cookie; otherwise returns an honest 401 with
/// `needs_setup` so the dashboard can route to the setup vs login screen. The
/// dev/test bypass (`auth_disabled`) passes everything through.
///
/// **Sliding session:** on a *successful* protected response the guard slides the
/// session's idle deadline forward and re-emits the `relux_session` cookie with a
/// fresh `Max-Age` (capped at the absolute lifetime — see
/// [`relux_kernel::DashboardAuth::refresh_session`]). So an actively-used console
/// stays signed in indefinitely up to the absolute cap, while an idle one still
/// times out. The refreshed cookie is attached **only** when the request was
/// authenticated AND the handler returned a success status — a 401 from this
/// guard, or a 4xx/5xx from the handler, never carries a session cookie.
///
/// **Out-of-band revocation:** the `validate_session` call here reconciles the
/// session table with its backing file first ([`relux_kernel`]'s
/// `SessionStore::reconcile_if_changed`), so if `reset-admin` cleared the session
/// file underneath a running `serve`, this guard rejects the old cookie on the next
/// request — no restart required.
async fn require_session(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if state.auth_disabled {
        return next.run(req).await;
    }
    // Decide admission with a NON-sliding validate (so a single request slides the
    // window at most once, on success, below — not here).
    let sid = relux_kernel::session_cookie_from_headers(req.headers());
    let authed = sid
        .as_deref()
        .and_then(|s| state.dashboard_auth.validate_session(s))
        .is_some();
    if !authed {
        let needs_setup = !state.dashboard_auth.admin_exists();
        let error = if needs_setup {
            "setup required — create the local admin account first"
        } else {
            "authentication required — sign in to the Relux dashboard"
        };
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": error, "needs_setup": needs_setup })),
        )
            .into_response();
    }
    let mut resp = next.run(req).await;
    // Only a successful protected response refreshes the rolling session. This
    // keeps `Set-Cookie` off failed-handler responses and slides the server-side
    // idle window in lock-step with the cookie the browser keeps.
    if resp.status().is_success() {
        if let Some(sid) = sid {
            if let Some(max_age) = state.dashboard_auth.refresh_session(&sid) {
                if let Ok(hv) = header::HeaderValue::from_str(
                    &relux_kernel::set_session_cookie_with_max_age(&sid, max_age),
                ) {
                    resp.headers_mut().append(header::SET_COOKIE, hv);
                }
            }
        }
    }
    resp
}

/// Auth middleware for the per-agent token route subset. A request must carry a
/// valid `Authorization: Bearer <token>` that authenticates AS a specific agent;
/// the resolved [`relux_kernel::AgentTokenIdentity`] is inserted into the request
/// extensions for the handler to read (so the acting agent comes from the token,
/// never the path/body). On failure the request is a clean 401.
///
/// Unlike [`require_session`], this middleware has **no `RELUX_AUTH_DISABLED`
/// bypass**: an agent's identity is meaningless without a real token, so even in
/// the dev/test operator bypass an agent route still requires a valid token. This
/// keeps the agent-actor surface honest — it can never act as an unspecified agent.
async fn require_agent_token(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let identity = relux_kernel::bearer_token_from_headers(req.headers())
        .and_then(|raw| state.agent_tokens.authenticate(&raw));
    match identity {
        Some(identity) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "agent token authentication required — present a valid Authorization: Bearer <agent token>"
            })),
        )
            .into_response(),
    }
}

// --- Local operator login handlers -----------------------------------------

/// A username/password pair for setup/login. The password is never logged or
/// echoed back.
#[derive(Debug, Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

/// Read the `relux_session` cookie from a header map and resolve it to a logged-
/// in username (or `None`).
fn session_user(state: &AppState, headers: &header::HeaderMap) -> Option<String> {
    relux_kernel::session_cookie_from_headers(headers)
        .and_then(|sid| state.dashboard_auth.validate_session(&sid))
}

/// Attach a `Set-Cookie` header to a JSON 200 response.
fn ok_with_cookie<T: Serialize>(body: T, cookie: String) -> Response {
    let mut resp = (StatusCode::OK, Json(body)).into_response();
    if let Ok(hv) = header::HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    resp
}

fn auth_err(status: StatusCode, error: &str) -> Response {
    (status, Json(serde_json::json!({ "error": error }))).into_response()
}

/// `GET /v1/auth/status` — public. Tells the dashboard whether to show the
/// first-run setup form, the login form, or the app. When auth is disabled
/// (dev/test) it reports `authenticated: true` so the SPA renders the app.
async fn auth_status(State(state): State<AppState>, headers: header::HeaderMap) -> Response {
    if state.auth_disabled {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "needs_setup": false,
                "authenticated": true,
                "username": "dev",
                "auth_disabled": true,
            })),
        )
            .into_response();
    }
    let username = session_user(&state, &headers);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "needs_setup": !state.dashboard_auth.admin_exists(),
            "authenticated": username.is_some(),
            "username": username,
        })),
    )
        .into_response()
}

/// `POST /v1/auth/setup` — first-run only. Creates the admin account and logs
/// in. Refuses (409) once an admin already exists (use login instead).
async fn auth_setup(State(state): State<AppState>, Json(creds): Json<Credentials>) -> Response {
    if state.dashboard_auth.admin_exists() {
        return auth_err(
            StatusCode::CONFLICT,
            "admin already configured — log in instead",
        );
    }
    match state
        .dashboard_auth
        .create_admin(creds.username.trim(), &creds.password)
    {
        Ok(()) => {
            let username = creds.username.trim().to_string();
            let sid = state.dashboard_auth.create_session(&username);
            ok_with_cookie(
                serde_json::json!({ "username": username }),
                relux_kernel::set_session_cookie(&sid),
            )
        }
        // create_admin validates username/password and returns a clear message.
        Err(e) => auth_err(StatusCode::BAD_REQUEST, &e),
    }
}

/// `POST /v1/auth/login` — verify the admin password and mint a session.
async fn auth_login(State(state): State<AppState>, Json(creds): Json<Credentials>) -> Response {
    if !state.dashboard_auth.admin_exists() {
        return auth_err(
            StatusCode::CONFLICT,
            "no admin configured — run setup first",
        );
    }
    match state
        .dashboard_auth
        .verify_login(creds.username.trim(), &creds.password)
    {
        Some(username) => {
            let sid = state.dashboard_auth.create_session(&username);
            ok_with_cookie(
                serde_json::json!({ "username": username }),
                relux_kernel::set_session_cookie(&sid),
            )
        }
        None => auth_err(StatusCode::UNAUTHORIZED, "invalid username or password"),
    }
}

/// `POST /v1/auth/logout` — drop the session and clear the cookie.
async fn auth_logout(State(state): State<AppState>, headers: header::HeaderMap) -> Response {
    if let Some(sid) = relux_kernel::session_cookie_from_headers(&headers) {
        state.dashboard_auth.remove_session(&sid);
    }
    ok_with_cookie(
        serde_json::json!({ "ok": true }),
        relux_kernel::clear_session_cookie(),
    )
}

/// An authenticated password change: the operator's CURRENT password plus the
/// new one. Both fields are write-only — never logged or echoed back.
#[derive(Debug, Deserialize)]
struct ChangePasswordReq {
    current_password: String,
    new_password: String,
}

/// `POST /v1/auth/change-password` — protected (a valid session is required, so
/// it lives behind `require_session`). Verifies the current password, stores a
/// fresh Argon2id hash atomically, and invalidates every OTHER live session
/// while preserving the caller's own. This is the normal in-product change path;
/// recovery when the current password is unknown stays the local `reset-admin`
/// CLI. Neither password is ever logged or returned.
async fn auth_change_password(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Json(req): Json<ChangePasswordReq>,
) -> Response {
    // The dev/test bypass admits requests without a real session, so there is no
    // caller session to anchor the change to — refuse clearly rather than rewrite
    // a credential the bypass ignores anyway.
    if state.auth_disabled {
        return auth_err(
            StatusCode::BAD_REQUEST,
            "password change is unavailable while RELUX_AUTH_DISABLED is set",
        );
    }
    // The middleware already admitted a valid session; resolve the raw cookie so
    // we know WHICH session to preserve when the others are invalidated.
    let Some(sid) = relux_kernel::session_cookie_from_headers(&headers) else {
        return auth_err(StatusCode::UNAUTHORIZED, "not logged in");
    };
    match state
        .dashboard_auth
        .change_password(&sid, &req.current_password, &req.new_password)
    {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => {
            // Map each refusal to an honest status; the Display text never carries
            // a secret (see ChangePasswordError).
            let status = match e {
                relux_kernel::ChangePasswordError::WrongCurrent => StatusCode::UNAUTHORIZED,
                relux_kernel::ChangePasswordError::TooShort => StatusCode::BAD_REQUEST,
                relux_kernel::ChangePasswordError::NoAdmin => StatusCode::CONFLICT,
                relux_kernel::ChangePasswordError::Storage(_) => {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            auth_err(status, &e.to_string())
        }
    }
}

/// `GET /v1/auth/me` — the logged-in username plus safe session-expiry metadata,
/// or 401.
///
/// The body carries ONLY non-secret fields the Account control needs to render an
/// idle/re-auth readout: the username, the two absolute deadlines (unix seconds),
/// the seconds remaining on each, the configured policy windows, and the server's
/// own clock. It NEVER carries the session id, the cookie value, or the admin
/// hash.
///
/// **Pre- vs post-refresh:** this route is PUBLIC (it sits outside the
/// `require_session` sliding middleware) and reads via the non-mutating
/// [`relux_kernel::DashboardAuth::session_meta`], so polling it does NOT slide the
/// idle window. The deadlines returned are therefore the **current, pre-refresh**
/// values — exactly what the operator's cookie reflects right now, not a window
/// bumped by the act of asking. (A real protected `/v1/relux/*` request still
/// slides the window as before; this read deliberately does not.) The Account
/// modal can poll this safely without keeping an otherwise-idle console alive.
async fn auth_me(State(state): State<AppState>, headers: header::HeaderMap) -> Response {
    if state.auth_disabled {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "username": "dev", "auth_disabled": true })),
        )
            .into_response();
    }
    let Some(sid) = relux_kernel::session_cookie_from_headers(&headers) else {
        return auth_err(StatusCode::UNAUTHORIZED, "not logged in");
    };
    match state.dashboard_auth.session_meta(&sid) {
        Some(meta) => {
            let now = now_secs();
            // Clamp remaining at 0 — a session at/just past a deadline reads as
            // "0 left", never a negative countdown.
            let idle_remaining = (meta.idle_expires_at - now).max(0);
            let absolute_remaining = (meta.absolute_expires_at - now).max(0);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "username": meta.username,
                    // Absolute instants (unix seconds) so a client can anchor its
                    // own countdown to the server clock if it prefers.
                    "idle_expires_at": meta.idle_expires_at,
                    "absolute_expires_at": meta.absolute_expires_at,
                    // Pre-computed remaining seconds (skew-free for a simple local
                    // countdown), clamped at 0.
                    "idle_expires_in_secs": idle_remaining,
                    "absolute_expires_in_secs": absolute_remaining,
                    // The configured policy windows, so the UI can state the rule
                    // ("signs out after Nh idle / re-auth after Nd") plainly.
                    "idle_timeout_secs": relux_kernel::SESSION_TTL_SECS,
                    "absolute_max_secs": relux_kernel::SESSION_ABSOLUTE_MAX_SECS,
                    // The server's own clock, so the client can compute an offset
                    // rather than trusting the browser's wall time.
                    "server_now": now,
                })),
            )
                .into_response()
        }
        None => auth_err(StatusCode::UNAUTHORIZED, "not logged in"),
    }
}

// --- Static dashboard serving ----------------------------------------------

/// `/` redirects to the dashboard so a bare visit lands on the product, not a
/// blank 404. Temporary (307) so it is never cached as permanent.
async fn root_redirect() -> Redirect {
    Redirect::temporary("/dashboard")
}

/// Serve the SPA `index.html` for `/dashboard` and `/dashboard/`.
async fn dashboard_index(State(state): State<AppState>) -> Response {
    serve_index(&state).await
}

/// Serve one path under `/dashboard/*`: a real bundle file when it exists,
/// otherwise the SPA `index.html` (history fallback) for client routes like
/// `/dashboard/prime`. A missing path under `assets/` is an honest 404 rather
/// than the shell, so a stale asset reference surfaces instead of silently
/// returning HTML.
async fn dashboard_path(State(state): State<AppState>, AxumPath(path): AxumPath<String>) -> Response {
    let Some(dir) = state.dashboard_dir.as_ref() else {
        return missing_bundle_notice();
    };
    if let Some(file) = crate::dashboard::resolve_asset(dir, &path) {
        return serve_file(&file).await;
    }
    if path.starts_with("assets/") {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    }
    serve_index(&state).await
}

/// Read + return the SPA index, or the honest missing-bundle notice when the
/// bundle is absent or unreadable. `index.html` is never cached so a rebuilt
/// bundle (new hashed asset names) is picked up on the next load.
async fn serve_index(state: &AppState) -> Response {
    let Some(dir) = state.dashboard_dir.as_ref() else {
        return missing_bundle_notice();
    };
    match tokio::fs::read(dir.join("index.html")).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => missing_bundle_notice(),
    }
}

/// Read + return one bundle file with an honest content type. Hashed assets are
/// immutable, so they carry a long-lived cache header; `index.html` never does
/// (whether reached via the SPA routes or a direct `/dashboard/index.html`),
/// because a rebuilt bundle changes the hashed asset names it references and a
/// stale immutable copy would point at files that no longer exist.
async fn serve_file(file: &std::path::Path) -> Response {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ctype = crate::dashboard::content_type_for(name);
    let cache = if name.eq_ignore_ascii_case("index.html") {
        "no-store"
    } else {
        "public, max-age=31536000, immutable"
    };
    match tokio::fs::read(file).await {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, cache)],
            bytes,
        )
            .into_response(),
        Err(e) => ApiError::internal(format!("failed to read asset: {e}")).into_response(),
    }
}

/// Honest 503 served when no dashboard bundle is present. It is deliberately NOT
/// a dashboard (no app shell, no asset bundle) so a missing build reads as a
/// build/setup step, not a broken product. The `/v1/relux` API is unaffected.
fn missing_bundle_notice() -> Response {
    const BODY: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Relux - dashboard not built</title></head><body>\
<h1>Relux dashboard bundle not found</h1>\
<p>The Relux dashboard is a React app that must be built before relux-kernel can \
serve it. This is a build/setup step, not a product error.</p>\
<p>Build it, then reload:</p>\
<pre>cd apps/dashboard\nnpm install\nnpm run build</pre>\
<p>That emits <code>crates/relix-web-bridge/dashboard-dist/</code>, which the \
<code>/dashboard</code> route serves. Set <code>RELUX_DASHBOARD_DIST</code> to \
point at a bundle elsewhere.</p>\
<p>The Relux API at <code>/v1/relux/*</code> is unaffected.</p>\
</body></html>";
    match Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(BODY))
    {
        Ok(r) => r,
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "dashboard bundle not built - run `npm run build` in apps/dashboard",
        )
            .into_response(),
    }
}

// --- Handlers --------------------------------------------------------------

async fn get_state(State(state): State<AppState>) -> Result<Json<StateResponse>, ApiError> {
    let resp = locked_read(&state, |kernel| Ok(state_response(kernel, &state.db_path)))?;
    Ok(Json(resp))
}

async fn get_ai_status(State(state): State<AppState>) -> Json<AiStatus> {
    Json(resolve_ai(&state).status())
}

/// Set or update Prime's AI provider configuration from the dashboard.
///
/// First-release product path (`docs/RELUX_MASTER_PLAN.md` "Optional LLM-backed
/// Prime"): an operator configures the OpenRouter key WITHOUT environment
/// variables. The key is stored in a local gitignored secrets file under the data
/// root and is NEVER returned - the response is the key-free [`AiStatus`]. Claude
/// and Codex adapters do not use a key here; they authenticate via their own
/// local CLI login.
async fn set_ai_config(
    State(state): State<AppState>,
    Json(req): Json<SetAiConfigReq>,
) -> Result<Json<AiStatus>, ApiError> {
    if let Some(p) = req.provider.as_ref() {
        let p = p.trim().to_ascii_lowercase();
        if !p.is_empty() && p != "openrouter" {
            return Err(ApiError::bad_request(format!(
                "unsupported provider '{p}'. Only 'openrouter' takes an API key today; \
                 Claude and Codex adapters use their own local CLI login (no key here)."
            )));
        }
    }
    // Validate the brain selection up front so a typo is a clear 400, not a
    // silently-ignored field. An empty string clears the selection.
    if let Some(b) = req.brain.as_ref() {
        let b = b.trim();
        if !b.is_empty() && relux_kernel::PrimeBrain::parse(b).is_none() {
            return Err(ApiError::bad_request(format!(
                "unsupported brain '{b}'. Use one of: local, openrouter, claude_cli, codex_cli."
            )));
        }
    }
    relux_kernel::write_stored_config(
        &state.ai_config_path,
        req.provider,
        req.api_key,
        req.model,
        req.disabled,
        req.brain,
    )
    .map_err(|e| ApiError::internal(format!("failed to write AI config: {e}")))?;
    Ok(Json(resolve_ai(&state).status()))
}

/// Clear the dashboard-written AI config entirely (Prime falls back to env, then
/// to deterministic mode). Returns the resulting key-free [`AiStatus`].
async fn clear_ai_config(State(state): State<AppState>) -> Result<Json<AiStatus>, ApiError> {
    relux_kernel::clear_stored_config(&state.ai_config_path)
        .map_err(|e| ApiError::internal(format!("failed to clear AI config: {e}")))?;
    Ok(Json(resolve_ai(&state).status()))
}

/// The dashboard's AI-config write payload. Only OpenRouter is honored; the key
/// is accepted here but never echoed back. An empty `api_key` clears the stored
/// key without disturbing the model/disabled flags.
#[derive(Debug, Deserialize)]
struct SetAiConfigReq {
    provider: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    disabled: Option<bool>,
    /// The selected Prime brain (`local` | `openrouter` | `claude_cli` |
    /// `codex_cli`). An empty string clears the selection (legacy auto choice).
    brain: Option<String>,
}

async fn list_agents(State(state): State<AppState>) -> Result<Json<Vec<AgentRecord>>, ApiError> {
    let records = locked_read(&state, |kernel| {
        let agents = kernel.agents();
        // Index the roster once so each card can show its Lead's display name and its
        // direct reports compactly (no per-agent roster scan, no big org-chart payload).
        let name_by_id: std::collections::BTreeMap<&str, &str> =
            agents.iter().map(|a| (a.id.as_str(), a.name.as_str())).collect();
        let mut direct_reports: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for a in &agents {
            if let Some(lead) = &a.reports_to {
                direct_reports
                    .entry(lead.as_str().to_string())
                    .or_default()
                    .push(a.id.as_str().to_string());
            }
        }
        Ok(agents
            .iter()
            .map(|a| {
                let mut rec = agent_record(a);
                rec.reports_to_name = a
                    .reports_to
                    .as_ref()
                    .and_then(|lead| name_by_id.get(lead.as_str()).map(|n| n.to_string()));
                rec.reports = direct_reports.get(a.id.as_str()).cloned().unwrap_or_default();
                rec
            })
            .collect())
    })?;
    Ok(Json(records))
}

/// Optional `?include_internal=true` reveals internal dev/test fixtures (e.g. the
/// echo ToolSet) that are hidden from normal product surfaces by default.
#[derive(Debug, Deserialize, Default)]
struct IncludeInternalQuery {
    include_internal: Option<bool>,
}

async fn list_plugins(
    State(state): State<AppState>,
    query: axum::extract::Query<IncludeInternalQuery>,
) -> Result<Json<Vec<PluginRecord>>, ApiError> {
    let include_internal = query.include_internal.unwrap_or(false);
    let records = locked_read(&state, |kernel| Ok(plugin_records(kernel)))?;
    // Hide internal dev/test fixtures (echo) from the normal Plugins surface so an
    // operator never mistakes them for a real capability.
    let records = records
        .into_iter()
        .filter(|p| include_internal || !relux_kernel::is_internal_plugin(&p.id))
        .collect();
    Ok(Json(records))
}

/// Optional `?agent=<id>` scoping for tool discovery: when supplied, each tool's
/// executable status reflects whether THAT agent holds the permission
/// (`ready`/`missing_permission`); when absent, discovery is permission-agnostic.
/// `?include_internal=true` reveals hidden dev/test tools (echo).
#[derive(Debug, Deserialize)]
struct ToolsQuery {
    agent: Option<String>,
    include_internal: Option<bool>,
}

/// GET `/v1/relux/tools` - list installed plugin tools with their honest
/// executable status (`docs/RELUX_MASTER_PLAN.md` section 7.4; `docs/Relux spec.md`
/// section 20.2 Tools view). Returns only manifest-declared tool metadata; never
/// plugin config or secrets.
async fn list_tools(
    State(state): State<AppState>,
    query: axum::extract::Query<ToolsQuery>,
) -> Result<Json<Vec<relux_core::ToolDescriptor>>, ApiError> {
    let agent_id = query
        .agent
        .as_ref()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(relux_core::AgentId::new);
    let include_internal = query.include_internal.unwrap_or(false);
    let tools = locked_read(&state, |kernel| {
        Ok(kernel.discover_tools(agent_id.as_ref()))
    })?;
    // Hide internal dev/test tools (echo) from the normal Tools surface unless a
    // dev explicitly opts in.
    let tools = tools
        .into_iter()
        .filter(|t| include_internal || !relux_kernel::is_internal_plugin(&t.plugin_id))
        .collect();
    Ok(Json(tools))
}

#[derive(Debug, Deserialize)]
struct InvokeToolReq {
    plugin_id: String,
    tool_name: String,
    /// JSON input passed to the tool; defaults to `{}` when omitted.
    input: Option<serde_json::Value>,
    /// Actor to attribute the call to; defaults to Prime when omitted.
    agent_id: Option<String>,
}

/// POST `/v1/relux/tools/invoke` - invoke a supported built-in tool, permission-
/// checked and audited (`docs/RELUX_MASTER_PLAN.md` section 13.6, section 10.2).
///
/// The actor defaults to Prime (when it exists and holds the permission);
/// otherwise an explicit `agent_id` is required. An installed-but-unimplemented
/// tool returns HTTP 501 with a clear error and never fabricates output; a
/// permission denial returns HTTP 403.
async fn invoke_tool(
    State(state): State<AppState>,
    Json(req): Json<InvokeToolReq>,
) -> Result<Json<ToolInvocationResult>, ApiError> {
    let plugin_id = req.plugin_id.trim().to_string();
    if plugin_id.is_empty() {
        return Err(ApiError::bad_request("plugin_id is required"));
    }
    let tool_name = req.tool_name.trim().to_string();
    if tool_name.is_empty() {
        return Err(ApiError::bad_request("tool_name is required"));
    }
    let input = req.input.unwrap_or_else(|| serde_json::json!({}));
    let requested_agent = req
        .agent_id
        .as_ref()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(|a| a.to_string());

    let result = locked_save(&state, |kernel| {
        let agent_id = match requested_agent {
            Some(a) => relux_core::AgentId::new(a),
            None => kernel.prime_agent_id().ok_or_else(|| {
                KernelError::UnknownAgent(
                    "no agent_id supplied and Prime is not available".to_string(),
                )
            })?,
        };
        kernel.invoke_tool(
            &agent_id,
            &relux_core::PluginId::new(plugin_id.clone()),
            &tool_name,
            input,
        )
    })?;
    Ok(Json(result))
}

/// POST `/v1/relux/tools/request-approval` — create a pending per-call approval to
/// invoke ONE non-low-risk configured tool with specific arguments
/// (`docs/RELUX_MASTER_PLAN.md` §7.4 per-call approval). Validates the tool exists,
/// the subject agent holds its permission, the tool actually requires approval, and
/// the args are bounded; binds the approval to the exact `(tool, args snapshot)`.
/// Nothing runs here — the operator decides on the Approvals page, then executes.
///
/// A directly-runnable (low-risk) tool is refused with 400 — invoke it instead.
async fn request_tool_invocation_approval(
    State(state): State<AppState>,
    Json(req): Json<InvokeToolReq>,
) -> Result<Json<ReluxApprovalRecord>, ApiError> {
    let plugin_id = req.plugin_id.trim().to_string();
    if plugin_id.is_empty() {
        return Err(ApiError::bad_request("plugin_id is required"));
    }
    let tool_name = req.tool_name.trim().to_string();
    if tool_name.is_empty() {
        return Err(ApiError::bad_request("tool_name is required"));
    }
    let input = req.input.unwrap_or_else(|| serde_json::json!({}));
    let requested_agent = req
        .agent_id
        .as_ref()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(|a| a.to_string());

    let record = locked_save(&state, |kernel| {
        let agent_id = match requested_agent {
            Some(a) => relux_core::AgentId::new(a),
            None => kernel.prime_agent_id().ok_or_else(|| {
                KernelError::UnknownAgent(
                    "no agent_id supplied and Prime is not available".to_string(),
                )
            })?,
        };
        let id = kernel.request_tool_invocation_approval(
            "dashboard_user",
            &agent_id,
            &relux_core::PluginId::new(plugin_id.clone()),
            &tool_name,
            input,
        )?;
        let approval = kernel.approval(&id).cloned().ok_or_else(|| {
            KernelError::UnknownApproval(id.to_string())
        })?;
        Ok(approval_record(kernel, approval))
    })?;
    Ok(Json(record))
}

/// POST `/v1/relux/approvals/:id/execute` — execute the single invocation bound to
/// an APPROVED per-call approval, exactly once (`docs/RELUX_MASTER_PLAN.md` §7.4).
/// Returns the structured [`ToolInvocationResult`] on success; an undecided/consumed
/// approval is a 409, a missing binding a 404, a permission/runtime failure honest.
async fn execute_approved_tool_invocation(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ToolInvocationResult>, ApiError> {
    let approval_id = relux_core::ApprovalId::new(id);
    let result = locked_save(&state, |kernel| {
        kernel.execute_approved_tool_invocation(&approval_id, "dashboard_user")
    })?;
    Ok(Json(result))
}

async fn list_tasks(State(state): State<AppState>) -> Result<Json<Vec<TaskRecord>>, ApiError> {
    let records = locked_read(&state, |kernel| {
        let tasks = kernel.tasks();
        let agents_by_id: std::collections::HashMap<_, _> =
            kernel.agents().into_iter().map(|a| (a.id.clone(), a)).collect();
        let task_records: Vec<TaskRecord> = tasks
            .into_iter()
            .map(|t| {
                let agent = t.assigned_agent.as_ref().and_then(|id| agents_by_id.get(id));
                task_record(t, agent.map(|v| &**v))
            })
            .collect();
        Ok(task_records)
    })?;
    Ok(Json(records))
}

async fn list_runs(State(state): State<AppState>) -> Result<Json<Vec<relux_core::Run>>, ApiError> {
    let runs = locked_read(&state, |kernel| {
        Ok(kernel.runs().into_iter().cloned().collect())
    })?;
    Ok(Json(runs))
}

async fn get_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<TaskRecord>, ApiError> {
    let task_id = relux_core::TaskId::new(id);
    let record = locked_read(&state, |kernel| {
        let task = kernel
            .task(&task_id)
            .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
        let agent = task
            .assigned_agent
            .as_ref()
            .and_then(|id| kernel.agent(id));
        Ok(task_record(task, agent))
    })?;
    Ok(Json(record))
}

async fn get_run(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RunRecord>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    let record = locked_read(&state, |kernel| {
        let run = kernel
            .run(&run_id)
            .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?
            .clone();
        Ok(build_run_record(kernel, run))
    })?;
    Ok(Json(record))
}

/// Query params for `GET /v1/relux/runs/:id/events`.
#[derive(Debug, Deserialize)]
struct RunEventsQuery {
    /// Optional exclusive event-id cursor: return ONLY events strictly after
    /// this id (the incremental live-tail). Absent/empty returns the full
    /// transcript, so a first load (or a client that lost its place) still gets
    /// everything. Mirrors the legacy bridge `/v1/runs/:id/events?since=`
    /// (relix-dashboard-design §8) for the Relux run model.
    #[serde(default)]
    since: Option<String>,
}

async fn get_run_events(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    query: axum::extract::Query<RunEventsQuery>,
) -> Result<Json<Vec<relux_kernel::RunEvent>>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    // An empty `since` degrades to a full transcript (treated as "no cursor").
    let since = query.since.as_deref().filter(|s| !s.is_empty());
    let events = locked_read(&state, |kernel| {
        // Check if the run exists to return 404 if not.
        kernel
            .run(&run_id)
            .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
        Ok(kernel
            .run_events_since(&run_id, since)
            .into_iter()
            .cloned()
            .collect())
    })?;
    Ok(Json(events))
}

/// Query params for `GET /v1/relux/runs/:id/logs`.
#[derive(Debug, Deserialize)]
struct RunLogsQuery {
    /// Optional exclusive 1-based sequence cursor: return ONLY log lines strictly
    /// after this `seq` (the pollable incremental tail — the analogue of
    /// Paperclip's byte `offset`). Absent/empty/unparseable returns the full
    /// bounded tail, so a first load (or a client that lost its place) still gets
    /// everything (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10).
    #[serde(default)]
    since: Option<u32>,
}

/// The bounded, redacted run-log tail for one run. A run with no captured log
/// (the deterministic local-echo path, or a not-yet-executed run) returns an
/// empty `lines` array with `dropped_lines: 0` and no truncation — never an
/// error for a real run — so the UI renders an honest "No logs" state instead of
/// blanking. An unknown run id is the kernel's existing `UnknownRun` 400 (the
/// same mapping every other run route uses).
async fn get_run_logs(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    query: axum::extract::Query<RunLogsQuery>,
) -> Result<Json<relux_core::RunLog>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    // A `since=0` cursor degrades to a full tail (lines are 1-based, so nothing
    // is ever filtered by 0 anyway, but treat it as "no cursor" explicitly).
    let since = query.since.filter(|s| *s > 0);
    // Read the durable log under the kernel lock (also the 404 validation). A run
    // that has FINALIZED carries the canonical persisted log; an in-flight run does
    // not yet, so we fall to the live registry below.
    let (has_persisted, persisted) = locked_read(&state, |kernel| {
        // 404 only for an unknown run; a real run with no captured log returns
        // the empty tail.
        kernel
            .run(&run_id)
            .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
        Ok((kernel.has_run_log(&run_id), kernel.run_log(&run_id, since)))
    })?;
    // Precedence: the finalized durable log wins once it exists. Until then, serve
    // the LIVE tail streamed by an in-flight off-lock run (read WITHOUT the kernel
    // lock, so it is unblocked while the run streams). With neither, the durable
    // empty tail is the honest "No logs" state.
    let log = if has_persisted {
        persisted
    } else {
        state
            .live_run_logs
            .snapshot(&run_id, since)
            .unwrap_or(persisted)
    };
    Ok(Json(log))
}

#[derive(Debug, Deserialize)]
struct AuditQueryParams {
    #[serde(default = "default_audit_limit")]
    limit: usize,
}

fn default_audit_limit() -> usize {
    100
}

async fn list_audit_events(
    State(state): State<AppState>,
    query: axum::extract::Query<AuditQueryParams>,
) -> Result<Json<Vec<relux_core::AuditEvent>>, ApiError> {
    let limit = query.limit.min(500); // Cap at 500 as per requirement
    let events = locked_read(&state, |kernel| {
        let audit_log = kernel.audit_log();
        let num_events = audit_log.len();
        let start_index = num_events.saturating_sub(limit);

        let recent_events: Vec<relux_core::AuditEvent> = audit_log
            .iter()
            .skip(start_index)
            .rev() // Reverse to get newest first
            .cloned()
            .collect();
        Ok(recent_events)
    })?;
    Ok(Json(events))
}

/// The per-tool-call binding surfaced alongside a tool-invocation approval, so the
/// Approvals page can render enough detail to decide and an Execute affordance.
/// Never leaks the raw args — only the bounded, secret-redacted preview + hash.
#[derive(Debug, Serialize)]
struct ReluxToolInvocationView {
    plugin_id: String,
    tool_name: String,
    agent_id: String,
    permission: String,
    risk: String,
    args_preview: String,
    args_sha256: String,
    /// One-shot: true once the bound invocation has been executed.
    consumed: bool,
    /// Convenience for the UI: the bound call can be executed now (approved + not
    /// yet consumed).
    executable: bool,
}

/// A richer approval record: the generic [`relux_core::Approval`] fields plus, for a
/// per-tool-call approval, its bound invocation detail. Replaces the bare
/// `Approval` the Approvals page previously consumed so it can show the action /
/// reason / risk and (for tool invocations) the args + Execute button.
#[derive(Debug, Serialize)]
struct ReluxApprovalRecord {
    id: String,
    requested_by: String,
    action: String,
    reason: String,
    /// `snake_case` wire form (`low`/`medium`/`high`/`critical`).
    risk: String,
    /// `snake_case` wire form (`pending`/`approved`/`rejected`).
    status: String,
    approved_by: Option<String>,
    created_at: String,
    resolved_at: Option<String>,
    note: Option<String>,
    tool_invocation: Option<ReluxToolInvocationView>,
}

fn wire_label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build a [`ReluxApprovalRecord`] for one approval, attaching its tool-invocation
/// binding when present.
fn approval_record(kernel: &KernelState, approval: relux_core::Approval) -> ReluxApprovalRecord {
    let status = wire_label(&approval.status);
    let tool_invocation = kernel.pending_tool_invocation(&approval.id).map(|b| {
        let executable =
            approval.status == relux_core::ApprovalStatus::Approved && !b.consumed;
        ReluxToolInvocationView {
            plugin_id: b.plugin_id.to_string(),
            tool_name: b.tool_name.clone(),
            agent_id: b.agent_id.to_string(),
            permission: b.permission.clone(),
            risk: wire_label(&b.risk),
            args_preview: b.args_preview.clone(),
            args_sha256: b.args_sha256.clone(),
            consumed: b.consumed,
            executable,
        }
    });
    ReluxApprovalRecord {
        id: approval.id.to_string(),
        requested_by: approval.requested_by,
        action: approval.action,
        reason: approval.reason,
        risk: wire_label(&approval.risk),
        status,
        approved_by: approval.approved_by,
        created_at: approval.created_at,
        resolved_at: approval.resolved_at,
        note: approval.note,
        tool_invocation,
    }
}

async fn list_approvals(
    State(state): State<AppState>,
) -> Result<Json<Vec<ReluxApprovalRecord>>, ApiError> {
    let approvals = locked_read(&state, |kernel| {
        let mut all_approvals: Vec<relux_core::Approval> =
            kernel.approvals.values().cloned().collect();
        // Sort approvals: pending first, then by created_at descending
        all_approvals.sort_by(|a, b| {
            let order_a = match a.status {
                relux_core::ApprovalStatus::Pending => 0,
                relux_core::ApprovalStatus::Approved => 1,
                relux_core::ApprovalStatus::Rejected => 2,
            };
            let order_b = match b.status {
                relux_core::ApprovalStatus::Pending => 0,
                relux_core::ApprovalStatus::Approved => 1,
                relux_core::ApprovalStatus::Rejected => 2,
            };

            order_a
                .cmp(&order_b)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        Ok(all_approvals
            .into_iter()
            .map(|a| approval_record(kernel, a))
            .collect::<Vec<_>>())
    })?;
    Ok(Json(approvals))
}

#[derive(Debug, Deserialize)]
struct DecideApprovalReq {
    decision: String,
    note: Option<String>,
}

async fn decide_approval(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<DecideApprovalReq>,
) -> Result<Json<ReluxApprovalRecord>, ApiError> {
    let approval_id = relux_core::ApprovalId::new(id);
    let approve = match req.decision.as_str() {
        "approved" => true,
        "rejected" => false,
        _ => return Err(ApiError::bad_request("decision must be 'approved' or 'rejected'")),
    };

    let record = locked_save(&state, |kernel| {
        // TODO: Pass actual user or Prime agent id for approver
        kernel.resolve_approval(&approval_id, approve, "dashboard_user", req.note)?;
        let approval = kernel
            .approval(&approval_id)
            .cloned()
            .ok_or_else(|| KernelError::UnknownApproval(approval_id.to_string()))?;
        Ok(approval_record(kernel, approval))
    })?;
    Ok(Json(record))
}

/// The wire form of a persistent allow-always grant, for the Approvals/Governance UI.
/// Never the bare core type so the risk renders as a `snake_case` wire label.
#[derive(Debug, Serialize)]
struct ReluxPersistentGrantRecord {
    id: String,
    created_by: String,
    agent_id: String,
    plugin_id: String,
    tool_name: String,
    permission: String,
    /// `snake_case` wire form (`low`/`medium`/`high`/`critical`).
    risk: String,
    created_at: String,
    last_used_at: Option<String>,
}

fn grant_record(g: &relux_core::PersistentGrant) -> ReluxPersistentGrantRecord {
    ReluxPersistentGrantRecord {
        id: g.id.clone(),
        created_by: g.created_by.clone(),
        agent_id: g.subject_agent.as_str().to_string(),
        plugin_id: g.plugin_id.clone(),
        tool_name: g.tool_name.clone(),
        permission: g.permission.clone(),
        risk: wire_label(&g.risk),
        created_at: g.created_at.clone(),
        last_used_at: g.last_used_at.clone(),
    }
}

/// POST `/v1/relux/approvals/:id/allow-always` — "Allow always" on a pending
/// per-tool-call approval: create a standing grant from its bound invocation AND
/// approve the pending approval (so the bound one-shot can still run once). Future
/// matching direct invocations then bypass the per-call prompt. A generic approval
/// (no tool-invocation binding) is a 404.
async fn allow_always_from_approval(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ReluxApprovalRecord>, ApiError> {
    let approval_id = relux_core::ApprovalId::new(id);
    let record = locked_save(&state, |kernel| {
        kernel.allow_always_from_approval(&approval_id, "dashboard_user")?;
        let approval = kernel
            .approval(&approval_id)
            .cloned()
            .ok_or_else(|| KernelError::UnknownApproval(approval_id.to_string()))?;
        Ok(approval_record(kernel, approval))
    })?;
    Ok(Json(record))
}

async fn list_persistent_grants(
    State(state): State<AppState>,
) -> Result<Json<Vec<ReluxPersistentGrantRecord>>, ApiError> {
    let grants = locked_read(&state, |kernel| {
        Ok(kernel
            .persistent_grants()
            .into_iter()
            .map(grant_record)
            .collect::<Vec<_>>())
    })?;
    Ok(Json(grants))
}

#[derive(Debug, Deserialize)]
struct CreateGrantReq {
    /// The permission subject the grant applies to. Optional — defaults to Prime.
    agent_id: Option<String>,
    plugin_id: String,
    tool_name: String,
}

/// POST `/v1/relux/grants` — create a persistent allow-always grant directly (the
/// Governance affordance). Validates the tool exists, the subject holds its
/// permission, and the tool actually gates; a directly-runnable tool is refused.
async fn create_persistent_grant(
    State(state): State<AppState>,
    Json(req): Json<CreateGrantReq>,
) -> Result<Json<ReluxPersistentGrantRecord>, ApiError> {
    let plugin_id = req.plugin_id.trim().to_string();
    if plugin_id.is_empty() {
        return Err(ApiError::bad_request("plugin_id is required"));
    }
    let tool_name = req.tool_name.trim().to_string();
    if tool_name.is_empty() {
        return Err(ApiError::bad_request("tool_name is required"));
    }
    let requested_agent = req
        .agent_id
        .as_ref()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(|a| a.to_string());

    let record = locked_save(&state, |kernel| {
        let subject = match requested_agent {
            Some(a) => relux_core::AgentId::new(a),
            None => kernel.prime_agent_id().ok_or_else(|| {
                KernelError::UnknownAgent(
                    "no agent_id supplied and Prime is not available".to_string(),
                )
            })?,
        };
        let grant = kernel.grant_persistent_tool_invocation(
            "dashboard_user",
            &subject,
            &relux_core::PluginId::new(plugin_id.clone()),
            &tool_name,
        )?;
        Ok(grant_record(&grant))
    })?;
    Ok(Json(record))
}

/// DELETE `/v1/relux/grants/:id` — revoke a persistent allow-always grant. After
/// this the covered invocation requires per-call approval again. Unknown id is 404.
async fn revoke_persistent_grant(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    locked_save(&state, |kernel| {
        kernel.revoke_persistent_grant(&id, "dashboard_user")
    })?;
    Ok(Json(serde_json::json!({ "revoked": true })))
}

#[derive(Debug, Serialize)]
struct AgentPermissionsRecord {
    agent_id: String,
    permissions: Vec<String>,
}

async fn list_permissions(
    State(state): State<AppState>,
) -> Result<Json<Vec<AgentPermissionsRecord>>, ApiError> {
    let records = locked_read(&state, |kernel| {
        let agent_permissions: Vec<AgentPermissionsRecord> = kernel
            .agents()
            .into_iter()
            .map(|agent| AgentPermissionsRecord {
                agent_id: agent.id.to_string(),
                permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
            })
            .collect();
        Ok(agent_permissions)
    })?;
    Ok(Json(records))
}

#[derive(Debug, Deserialize)]
struct GrantPermissionReq {
    permission: String,
}

async fn grant_agent_permission(
    State(state): State<AppState>,
    AxumPath(agent_id_str): AxumPath<String>,
    Json(req): Json<GrantPermissionReq>,
) -> Result<Json<AgentPermissionsRecord>, ApiError> {
    let agent_id = relux_core::AgentId::new(agent_id_str.clone());
    let permission = relux_core::Permission::new(&req.permission)
        .map_err(|e| ApiError::bad_request(format!("invalid permission string: {e}")))?;

    let updated_agent_permissions = locked_save(&state, |kernel| {
        kernel.grant_permission_to_agent(&agent_id, permission)?;
        let agent = kernel
            .agent(&agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?; // Should not happen after successful grant
        Ok(AgentPermissionsRecord {
            agent_id: agent.id.to_string(),
            permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
        })
    })?;
    Ok(Json(updated_agent_permissions))
}

/// Revoke an EXPLICIT permission from an agent. The operator console is the human
/// approval (the same gate as if they'd clicked the button), so this is a direct,
/// audited operator action — the inverse of [`grant_agent_permission`]. Revoking a
/// permission the agent does not hold is an honest 404 (`PermissionNotGranted`), not a
/// silent no-op. Returns the agent's remaining explicit permissions.
async fn revoke_agent_permission(
    State(state): State<AppState>,
    AxumPath(agent_id_str): AxumPath<String>,
    Json(req): Json<GrantPermissionReq>,
) -> Result<Json<AgentPermissionsRecord>, ApiError> {
    let agent_id = relux_core::AgentId::new(agent_id_str.clone());
    let permission = relux_core::Permission::new(&req.permission)
        .map_err(|e| ApiError::bad_request(format!("invalid permission string: {e}")))?;

    let updated = locked_save(&state, |kernel| {
        kernel.revoke_permission_from_agent(&agent_id, &permission)?;
        let agent = kernel
            .agent(&agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?;
        Ok(AgentPermissionsRecord {
            agent_id: agent.id.to_string(),
            permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
        })
    })?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
struct ManagerGrantReq {
    target_id: String,
    permission: String,
}

/// `POST /v1/relux/agents/:id/manager-grant` — the operator-assisted manager-subtree
/// grant. `:id` is the **acting manager**; the body names a `target_id` subordinate and
/// the `permission` to grant. The manager grants the capability to one of its own-Branch
/// operatives, gated by the real `agent:<manager>:subtree:grant_permission` scope it
/// holds (own-Branch + Active + scope — see
/// [`relux_kernel::KernelState::manager_grant_permission_to_subordinate`]).
///
/// HONEST trust boundary (documented in `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §19): Relux
/// has **no per-agent auth identity** yet — a manager agent cannot present its own
/// credential. So the authenticated dashboard **operator** explicitly authorizes "grant
/// as this manager"; the operator is named in the audit. The operator can NOT bypass the
/// manager-subtree rule — an unauthorized manager (no scope / not Active / target outside
/// its Branch) is a `403` and grants nothing, exactly as for a real agent actor. The only
/// thing the operator supplies is the *request*; authority is still the manager's.
///
/// A malformed permission string is an honest `400`. An unauthorized manager (or unknown
/// manager/target, since existence is folded into the fail-closed authority check) is a
/// `403`. On success it returns the **target's** updated explicit permission list.
async fn manager_grant_to_subordinate(
    State(state): State<AppState>,
    AxumPath(manager_id_str): AxumPath<String>,
    headers: header::HeaderMap,
    Json(req): Json<ManagerGrantReq>,
) -> Result<Json<AgentPermissionsRecord>, ApiError> {
    let manager_id = relux_core::AgentId::new(manager_id_str);
    let target_id = relux_core::AgentId::new(req.target_id);
    let permission = relux_core::Permission::new(&req.permission)
        .map_err(|e| ApiError::bad_request(format!("invalid permission string: {e}")))?;
    // The authenticated operator who stands in for the manager (the dev/test bypass has
    // no session, so it is attributed to the generic "operator").
    let operator = session_user(&state, &headers).unwrap_or_else(|| "operator".to_string());

    let updated = locked_save(&state, |kernel| {
        kernel.manager_grant_permission_to_subordinate_as_operator(
            &operator,
            &manager_id,
            &target_id,
            permission,
        )?;
        let agent = kernel
            .agent(&target_id)
            .ok_or_else(|| KernelError::UnknownAgent(target_id.to_string()))?;
        Ok(AgentPermissionsRecord {
            agent_id: agent.id.to_string(),
            permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
        })
    })?;
    Ok(Json(updated))
}

// --- Per-agent access tokens (operator mint/list/revoke) -------------------

#[derive(Debug, Deserialize)]
struct MintAgentTokenReq {
    /// Operator label for the token (e.g. "ci-runner"); optional, bounded/sanitized.
    #[serde(default)]
    label: String,
    /// Optional lifetime override (seconds); clamped to the bounded window. Omitted
    /// → the default TTL.
    #[serde(default)]
    ttl_secs: Option<i64>,
}

/// The mint response. The `token` is the raw secret, returned EXACTLY ONCE — it is
/// stored only as a hash and never shown again. The `warning` makes the copy-once
/// contract explicit for any API caller.
#[derive(Debug, Serialize)]
struct MintedAgentTokenRecord {
    token_id: String,
    token: String,
    agent_id: String,
    label: String,
    created_at: i64,
    expires_at: i64,
    warning: &'static str,
}

/// Non-secret token metadata for the operator list (never the hash or raw secret).
#[derive(Debug, Serialize)]
struct AgentTokenMetaRecord {
    token_id: String,
    agent_id: String,
    label: String,
    created_at: i64,
    expires_at: i64,
}

/// `POST /v1/relux/agents/:id/tokens` — operator mints a bounded, hashed-at-rest,
/// revocable access token for agent `:id`. The agent must exist. The raw token is
/// returned ONCE in the response and never persisted in plaintext; the mint is
/// audited (`agent:mint_token`, recording only the public `token_id`).
async fn mint_agent_token(
    State(state): State<AppState>,
    AxumPath(agent_id_str): AxumPath<String>,
    headers: header::HeaderMap,
    Json(req): Json<MintAgentTokenReq>,
) -> Result<Json<MintedAgentTokenRecord>, ApiError> {
    let agent_id = relux_core::AgentId::new(agent_id_str.clone());
    let operator = session_user(&state, &headers).unwrap_or_else(|| "operator".to_string());

    // Verify the agent exists before minting a credential for it.
    locked_read(&state, |kernel| {
        if kernel.agent(&agent_id).is_none() {
            return Err(KernelError::UnknownAgent(agent_id_str.clone()));
        }
        Ok(())
    })?;

    let minted = state
        .agent_tokens
        .mint(agent_id.as_str(), &req.label, req.ttl_secs);

    // Audit the mint in the durable kernel log (public token_id only).
    let token_id_for_audit = minted.token_id.clone();
    locked_save(&state, |kernel| {
        kernel.audit_agent_token_minted(&operator, &agent_id, &token_id_for_audit);
        Ok(())
    })?;

    Ok(Json(MintedAgentTokenRecord {
        token_id: minted.token_id,
        token: minted.secret,
        agent_id: minted.agent_id,
        label: minted.label,
        created_at: minted.created_at,
        expires_at: minted.expires_at,
        warning: "copy this token now — it is stored only as a hash and will never be shown again",
    }))
}

/// `GET /v1/relux/agents/:id/tokens` — list the live tokens' non-secret metadata for
/// agent `:id`. Never returns a hash or a raw token.
async fn list_agent_tokens(
    State(state): State<AppState>,
    AxumPath(agent_id_str): AxumPath<String>,
) -> Result<Json<Vec<AgentTokenMetaRecord>>, ApiError> {
    let agent_id = relux_core::AgentId::new(agent_id_str.clone());
    // Honest 404 for an unknown agent (rather than a silent empty list).
    locked_read(&state, |kernel| {
        if kernel.agent(&agent_id).is_none() {
            return Err(KernelError::UnknownAgent(agent_id_str.clone()));
        }
        Ok(())
    })?;
    let list = state
        .agent_tokens
        .list_for_agent(agent_id.as_str())
        .into_iter()
        .map(|m| AgentTokenMetaRecord {
            token_id: m.token_id,
            agent_id: m.agent_id,
            label: m.label,
            created_at: m.created_at,
            expires_at: m.expires_at,
        })
        .collect();
    Ok(Json(list))
}

/// `DELETE /v1/relux/agents/:id/tokens/:token_id` — revoke one of agent `:id`'s
/// tokens by its public id. Revoking an unknown token is an honest 404. The revoke
/// is audited (`agent:revoke_token`).
async fn revoke_agent_token(
    State(state): State<AppState>,
    AxumPath((agent_id_str, token_id)): AxumPath<(String, String)>,
    headers: header::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent_id = relux_core::AgentId::new(agent_id_str.clone());
    let operator = session_user(&state, &headers).unwrap_or_else(|| "operator".to_string());
    let found = state.agent_tokens.revoke(agent_id.as_str(), &token_id);

    let token_id_for_audit = token_id.clone();
    locked_save(&state, |kernel| {
        kernel.audit_agent_token_revoked(&operator, &agent_id, &token_id_for_audit, found);
        Ok(())
    })?;

    if found {
        Ok(Json(serde_json::json!({ "ok": true, "revoked": token_id })))
    } else {
        Err(ApiError::not_found(format!(
            "no token '{token_id}' for agent '{agent_id_str}'"
        )))
    }
}

// --- Agent-authenticated self routes (bearer agent token) ------------------

/// `GET /v1/relux/agents/me` — the agent identified by the bearer token reads its
/// OWN record (id/name/status/permissions + its Branch direct reports). Proves
/// agent-token auth and lets a manager see who is inside its Branch. The acting
/// agent is the token subject (from the validated [`relux_kernel::AgentTokenIdentity`]
/// in the request extensions), never a path/body value.
async fn agent_self_info(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<relux_kernel::AgentTokenIdentity>,
) -> Result<Json<AgentRecord>, ApiError> {
    let agent_id = relux_core::AgentId::new(identity.agent_id.clone());
    let record = locked_read(&state, |kernel| {
        let agent = kernel
            .agent(&agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(identity.agent_id.clone()))?;
        let mut rec = agent_record(agent);
        // Enrich the single record with its Lead name + direct reports so a manager
        // can see its Branch from self-info (the same shape the list endpoint builds).
        let agents = kernel.agents();
        if let Some(lead) = &agent.reports_to {
            rec.reports_to_name = agents
                .iter()
                .find(|a| &a.id == lead)
                .map(|a| a.name.clone());
        }
        rec.reports = agents
            .iter()
            .filter(|a| a.reports_to.as_ref() == Some(&agent_id))
            .map(|a| a.id.as_str().to_string())
            .collect();
        Ok(rec)
    })?;
    Ok(Json(record))
}

/// `POST /v1/relux/agents/me/manager-grant` — the **per-agent-authenticated**
/// manager-subtree grant (§19 follow-up): the manager that authenticated the request
/// with its own token grants a permission to one of its own-Branch subordinates, with
/// NO operator in the loop. The acting manager is the token subject — inferred from the
/// validated token, NOT from the request body — so a token can only ever grant as
/// itself. Authority is unchanged: the kernel still enforces own-Branch + Active +
/// `agent:<id>:subtree:grant_permission` scope. Malformed permission → 400; an
/// unauthorized manager (no scope / not Active / target outside its Branch / unknown
/// target) → 403, granting nothing.
async fn agent_self_manager_grant(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<relux_kernel::AgentTokenIdentity>,
    Json(req): Json<ManagerGrantReq>,
) -> Result<Json<AgentPermissionsRecord>, ApiError> {
    let manager_id = relux_core::AgentId::new(identity.agent_id.clone());
    let target_id = relux_core::AgentId::new(req.target_id);
    let permission = relux_core::Permission::new(&req.permission)
        .map_err(|e| ApiError::bad_request(format!("invalid permission string: {e}")))?;
    let token_ref = identity.token_id.clone();

    let updated = locked_save(&state, |kernel| {
        kernel.manager_grant_permission_to_subordinate_as_agent(
            &token_ref,
            &manager_id,
            &target_id,
            permission,
        )?;
        let agent = kernel
            .agent(&target_id)
            .ok_or_else(|| KernelError::UnknownAgent(target_id.to_string()))?;
        Ok(AgentPermissionsRecord {
            agent_id: agent.id.to_string(),
            permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
        })
    })?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
struct AgentAssignTaskReq {
    /// The existing task to assign.
    task_id: String,
    /// The subordinate to assign it to — must be a proper descendant of the acting
    /// manager's Branch.
    target_agent_id: String,
}

/// `POST /v1/relux/agents/me/assign-task` — the **per-agent-authenticated**
/// manager-subtree task assignment (the §20 follow-up that adds a second subtree action
/// beyond `grant_permission`). The manager that authenticated the request with its own
/// token assigns an existing, non-terminal task to one of its own-Branch subordinates,
/// with NO operator in the loop. The acting manager is the token subject — inferred from
/// the validated token, NOT from the request body — so a token can only ever assign as
/// itself.
///
/// Authority is the same gate as the manager-grant path: own-Branch + Active +
/// `agent:<id>:subtree:assign_task` scope. An unauthorized manager (no scope / not Active
/// / target outside its Branch / unknown target) → 403, assigning nothing. A missing task
/// → 400 (the kernel's existing `UnknownTask` mapping for every task route); a terminal
/// task → 409. On success it returns the updated task record.
async fn agent_self_assign_task(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<relux_kernel::AgentTokenIdentity>,
    Json(req): Json<AgentAssignTaskReq>,
) -> Result<Json<TaskRecord>, ApiError> {
    let manager_id = relux_core::AgentId::new(identity.agent_id.clone());
    let target_id = relux_core::AgentId::new(req.target_agent_id);
    let task_id = relux_core::TaskId::new(req.task_id);
    let token_ref = identity.token_id.clone();

    let updated = locked_save(&state, |kernel| {
        kernel.manager_assign_task_to_subordinate_as_agent(
            &token_ref,
            &manager_id,
            &target_id,
            &task_id,
        )?;
        let task = kernel
            .task(&task_id)
            .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
        let agent = task.assigned_agent.as_ref().and_then(|id| kernel.agent(id));
        Ok(task_record(task, agent))
    })?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
struct ManagerRevokeReq {
    target_id: String,
    permission: String,
}

/// `POST /v1/relux/agents/me/manager-revoke` — the **per-agent-authenticated**
/// manager-subtree permission revoke (the §22 follow-up that adds a third subtree action
/// beyond `grant_permission` and `assign_task`). The manager that authenticated the
/// request with its own token revokes an explicit permission from one of its own-Branch
/// subordinates, with NO operator in the loop. The acting manager is the token subject —
/// inferred from the validated token, NOT from the request body — so a token can only ever
/// revoke as itself.
///
/// Authority is the same gate as the manager-grant/assign paths: own-Branch + Active +
/// `agent:<id>:subtree:revoke_permission` scope. An unauthorized manager (no scope / not
/// Active / target outside its Branch / unknown target) → 403, revoking nothing. A
/// malformed permission → 400. The revoke removes EXACTLY the stored grant (no pattern
/// expansion); if the target does not hold it, that is the honest `PermissionNotGranted`
/// → 404 the operator revoke already returns. On success it returns the **target's**
/// updated explicit permission list.
async fn agent_self_manager_revoke(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<relux_kernel::AgentTokenIdentity>,
    Json(req): Json<ManagerRevokeReq>,
) -> Result<Json<AgentPermissionsRecord>, ApiError> {
    let manager_id = relux_core::AgentId::new(identity.agent_id.clone());
    let target_id = relux_core::AgentId::new(req.target_id);
    let permission = relux_core::Permission::new(&req.permission)
        .map_err(|e| ApiError::bad_request(format!("invalid permission string: {e}")))?;
    let token_ref = identity.token_id.clone();

    let updated = locked_save(&state, |kernel| {
        kernel.manager_revoke_permission_from_subordinate_as_agent(
            &token_ref,
            &manager_id,
            &target_id,
            &permission,
        )?;
        let agent = kernel
            .agent(&target_id)
            .ok_or_else(|| KernelError::UnknownAgent(target_id.to_string()))?;
        Ok(AgentPermissionsRecord {
            agent_id: agent.id.to_string(),
            permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
        })
    })?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
struct CreateTaskReq {
    title: String,
    /// Optional explicit tool-call directive. When present the task's input becomes
    /// the canonical `{ "tool_call": { plugin, tool, args } }` shape, so the local
    /// run executes that ONE operator-named tool through the gated `call_tool` path
    /// (an MCP `mcp:<server>` server or a real plugin) instead of the default echo.
    /// The directive is fixed here at creation time — never chosen by the brain.
    /// (`docs/mcp.md` "Run-driven MCP tool call".)
    #[serde(default)]
    tool_call: Option<relux_core::TaskToolCall>,
}

/// One read-only role preset (an operator-convenience suggestion bundle). Carries only
/// advisory fields — never a permission or adapter — so it can never widen an agent's
/// power. `docs/relix-dashboard-design.md` §9.1.
#[derive(Debug, Serialize)]
struct AgentPresetRecord {
    id: String,
    label: String,
    summary: String,
    role: String,
    persona: String,
    skills: Vec<String>,
}

/// `GET /v1/relux/agent-presets` — the curated role presets the Crew create form
/// offers. Read-only and non-sensitive: it only describes default role/persona/skills
/// text; applying one fills the (still editable) form and is created through the normal
/// validated path, which grants no extra permission.
async fn list_agent_presets() -> Json<Vec<AgentPresetRecord>> {
    let presets = relux_kernel::AGENT_PRESETS
        .iter()
        .map(|p| AgentPresetRecord {
            id: p.id.to_string(),
            label: p.label.to_string(),
            summary: p.summary.to_string(),
            role: p.role.to_string(),
            persona: p.persona.to_string(),
            skills: p.skills.iter().map(|s| s.to_string()).collect(),
        })
        .collect();
    Json(presets)
}

#[derive(Debug, Deserialize)]
struct CreateAgentReq {
    id: Option<String>,
    name: String,
    role: Option<String>,
    persona: Option<String>,
    adapter_plugin: Option<String>,
    /// Optional specialty tags/skills (each a short word/slug); validated and bounded
    /// server-side. Absent => no skills.
    skills: Option<Vec<String>>,
    /// Optional Lead (`reports_to`) — an existing crew member's id this operative reports
    /// to. Absent/blank => top-level. Validated against the live roster (must exist,
    /// cannot be self) server-side.
    reports_to: Option<String>,
    /// Optional role-preset id (`researcher`/`builder`/…). When present and recognised,
    /// it fills any role/persona/skills the request itself did NOT supply (the request's
    /// own value always wins), then the MERGED input flows through the same
    /// `validate_new_agent` validators. A preset never grants a permission or picks an
    /// adapter — the create below still grants only the minimal echo tool. An unknown
    /// preset id is an honest 400.
    preset: Option<String>,
}

async fn create_agent(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentReq>,
) -> Result<Json<AgentRecord>, ApiError> {
    // Expand a role preset (if any) BEFORE the lock: resolve it against the fixed
    // allowlist and fill only the advisory fields the request omitted. The request's
    // own non-blank value always wins; an unknown preset fails closed with a 400.
    let preset = match req.preset.as_deref() {
        Some(p) if !p.trim().is_empty() => Some(
            relux_kernel::find_agent_preset(p).ok_or_else(|| {
                ApiError::bad_request(format!(
                    "unknown preset '{}'; choose one of the listed presets",
                    p.trim()
                ))
            })?,
        ),
        _ => None,
    };
    // request field (non-blank) wins; else the preset default; else absent.
    let merged_role: Option<String> = match req.role.as_deref() {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        _ => preset.map(|p| p.role.to_string()),
    };
    let merged_persona: Option<String> = match req.persona.as_deref() {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        _ => preset.map(|p| p.persona.to_string()),
    };
    let merged_skills: Option<Vec<String>> = match req.skills.as_deref() {
        Some(s) if !s.is_empty() => Some(s.to_vec()),
        _ => preset.map(|p| p.skills.iter().map(|s| s.to_string()).collect()),
    };

    let agent = locked_save(&state, |kernel| {
        let ctx = crate::ensure_bootstrapped(kernel)?;

        // Gather the live rosters the validator needs: installed adapters (the
        // allowlist a chosen adapter must resolve to) and the existing ids/names
        // (uniqueness). Validation is pure; the kernel just hands it the state.
        let known_adapters: Vec<String> = kernel
            .adapter_runtime_status()
            .into_iter()
            .map(|a| a.plugin_id)
            .collect();
        let existing_ids: Vec<String> =
            kernel.agents().into_iter().map(|a| a.id.as_str().to_string()).collect();
        let existing_names: Vec<String> =
            kernel.agents().into_iter().map(|a| a.name.clone()).collect();

        let resolved = relux_kernel::validate_new_agent(
            relux_kernel::CreateAgentInput {
                id: req.id.as_deref(),
                name: &req.name,
                role: merged_role.as_deref(),
                persona: merged_persona.as_deref(),
                adapter_plugin: req.adapter_plugin.as_deref(),
                skills: merged_skills.as_deref(),
                reports_to: req.reports_to.as_deref(),
            },
            &known_adapters,
            &existing_ids,
            &existing_names,
        )
        .map_err(|e| KernelError::InvalidAgentConfig(e.message()))?;

        let adapter_plugin_id = relux_core::PluginId::new(resolved.adapter_plugin);
        // Grant minimal safe permissions for MVP (the echo tool); richer grants flow
        // through the explicit, approval-gated permission path.
        let permissions = vec![relux_core::Permission::new("tool:relux-tools-echo:say").unwrap()];

        let id = kernel.create_agent_with_skills(
            &resolved.id,
            &resolved.name,
            &resolved.description,
            &adapter_plugin_id,
            &ctx.namespace,
            resolved.persona,
            permissions,
            resolved.skills,
            resolved.reports_to.map(relux_core::AgentId::new),
        )?;
        Ok(agent_record(kernel.agent(&id).unwrap()))
    })?;
    Ok(Json(agent))
}

#[derive(Debug, Deserialize)]
struct UpdateAgentReq {
    name: Option<String>,
    role: Option<String>,
    persona: Option<String>,
    adapter_plugin: Option<String>,
    status: Option<String>,
    /// Present => REPLACE the whole skill list (an empty list clears it); absent =>
    /// leave skills unchanged.
    skills: Option<Vec<String>>,
    /// Present => set the Lead (`reports_to`); a blank string CLEARS it (top-level);
    /// absent => leave the Lead unchanged. Validated against the live roster (exists, not
    /// self, no cycle) server-side.
    reports_to: Option<String>,
}

/// Edit an existing agent's configurable fields (name, role, persona, adapter,
/// status). Absent fields are left unchanged; an empty `persona` clears it. All
/// values are sanitized/validated against the live rosters before they are applied.
async fn update_agent(
    State(state): State<AppState>,
    AxumPath(agent_id_str): AxumPath<String>,
    Json(req): Json<UpdateAgentReq>,
) -> Result<Json<AgentRecord>, ApiError> {
    let agent_id = relux_core::AgentId::new(agent_id_str.clone());
    let record = locked_save(&state, |kernel| {
        if kernel.agent(&agent_id).is_none() {
            return Err(KernelError::UnknownAgent(agent_id_str.clone()));
        }

        let known_adapters: Vec<String> = kernel
            .adapter_runtime_status()
            .into_iter()
            .map(|a| a.plugin_id)
            .collect();
        // Names held by every OTHER agent (renaming to one's own name is allowed).
        let names_except_self: Vec<String> = kernel
            .agents()
            .into_iter()
            .filter(|a| a.id != agent_id)
            .map(|a| a.name.clone())
            .collect();
        // The full roster of ids (for resolving a requested Lead against existing crew).
        let existing_ids: Vec<String> =
            kernel.agents().into_iter().map(|a| a.id.as_str().to_string()).collect();

        let resolved = relux_kernel::validate_agent_update(
            relux_kernel::UpdateAgentInput {
                name: req.name.as_deref(),
                role: req.role.as_deref(),
                persona: req.persona.as_deref(),
                adapter_plugin: req.adapter_plugin.as_deref(),
                status: req.status.as_deref(),
                skills: req.skills.as_deref(),
                reports_to: req.reports_to.as_deref(),
            },
            &known_adapters,
            &names_except_self,
            &existing_ids,
            agent_id.as_str(),
        )
        .map_err(|e| KernelError::InvalidAgentConfig(e.message()))?;

        kernel.update_agent_with_skills(
            &agent_id,
            resolved.name,
            resolved.description,
            resolved.persona,
            resolved.adapter_plugin.map(relux_core::PluginId::new),
            resolved.status,
            resolved.skills,
            resolved
                .reports_to
                .map(|opt| opt.map(relux_core::AgentId::new)),
        )?;
        Ok(agent_record(kernel.agent(&agent_id).unwrap()))
    })?;
    Ok(Json(record))
}

async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskReq>,
) -> Result<Json<relux_core::Task>, ApiError> {
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::bad_request("title is required"));
    }
    // Build the task input. An explicit tool-call directive is validated (non-empty
    // plugin + tool) and serialized into the canonical `{ "tool_call": … }` shape the
    // local run reads; otherwise the input is empty (a plain echo task).
    let input = match &req.tool_call {
        Some(directive) => {
            let built = directive.to_input();
            if relux_core::parse_task_tool_call(&built).is_none() {
                return Err(ApiError::bad_request(
                    "tool_call requires a non-empty plugin and tool",
                ));
            }
            built
        }
        None => serde_json::json!({}),
    };
    let task = locked_save(&state, |kernel| {
        let ctx = crate::ensure_bootstrapped(kernel)?;
        let id = kernel.create_task(
            &title,
            input,
            &ctx.actor,
            &ctx.namespace,
            vec![],
        );
        // Automatically assign to Prime so it is ready to run.
        kernel.assign_task(&id, &ctx.agent)?;
        Ok(kernel.task(&id).cloned().unwrap())
    })?;
    Ok(Json(task))
}

#[derive(Debug, Deserialize)]
struct AssignTaskReq {
    agent_id: String,
}

async fn assign_task_to_agent(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<AssignTaskReq>,
) -> Result<Json<relux_core::Task>, ApiError> {
    let task_id = relux_core::TaskId::new(id);
    let agent_id = relux_core::AgentId::new(req.agent_id);

    let task = locked_save(&state, |kernel| {
        kernel.assign_task(&task_id, &agent_id)?;
        Ok(kernel.task(&task_id).cloned().unwrap())
    })?;
    Ok(Json(task))
}

#[derive(Debug, Serialize)]
struct StartTaskResponse {
    task: relux_core::Task,
    run: relux_core::Run,
}

async fn start_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StartTaskResponse>, ApiError> {
    let task_id = relux_core::TaskId::new(id);
    let (task, run) = locked_save(&state, |kernel| {
        let run_id = kernel.start_run(&task_id)?;
        let task = kernel.task(&task_id).cloned().unwrap();
        let run = kernel.run(&run_id).cloned().unwrap();
        Ok((task, run))
    })?;
    Ok(Json(StartTaskResponse { task, run }))
}

#[derive(Debug, Serialize)]
struct ExecuteAssignedTaskResponse {
    run_id: relux_core::RunId,
}

async fn execute_assigned_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ExecuteAssignedTaskResponse>, ApiError> {
    let task_id = relux_core::TaskId::new(id);
    // Dispatch on the assigned agent's adapter: local Prime echoes; an enabled
    // CLI adapter (Claude/Codex/generic) spawns its local binary; anything else
    // fails honestly. The run/transcript is persisted either way.
    // Persist even on failure so a failed CLI run + its transcript survive for the
    // dashboard (and stay retryable), matching the CLI path.
    let run_id = locked_save_persisting(&state, |kernel| kernel.execute_assigned_run(&task_id))?;
    Ok(Json(ExecuteAssignedTaskResponse { run_id }))
}

#[derive(Debug, Serialize)]
struct RetryRunResponse {
    /// The id of the fresh run created by the retry (linked to the same task via
    /// the new run's `retried_from`).
    run_id: relux_core::RunId,
}

/// Retry a failed run as a fresh run on the same task (master plan section 10.2
/// `prime.retry_run`). This is a re-attempt, not a resume of a partial CLI run.
async fn retry_run(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RetryRunResponse>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    // Persist even if the retry's fresh run fails, so the new attempt is durable.
    let new_run_id = locked_save_persisting(&state, |kernel| kernel.retry_run(&run_id))?;
    Ok(Json(RetryRunResponse { run_id: new_run_id }))
}

/// Resume a prior run's provider session (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md`
/// §3). Distinct from retry: it continues the recorded adapter session (threading
/// its captured session id through the governed `--resume` gate) instead of
/// starting cold. Refuses honestly with 422 when the run carries no resumable
/// session (no captured session id, an adapter without safe non-interactive
/// resume, or a run still in flight) — never a faked continuation. The new run is
/// persisted even if the resume attempt itself fails honestly.
async fn resume_run(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RetryRunResponse>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    let new_run_id = locked_save_persisting(&state, |kernel| kernel.resume_run(&run_id))?;
    Ok(Json(RetryRunResponse { run_id: new_run_id }))
}

/// The honest result of a cancel request, returned by `POST .../cancel`.
#[derive(Debug, Serialize)]
struct CancelRunResponse {
    run_id: String,
    /// `requested` | `already_requested` | `not_running` (the wire form of
    /// [`relux_kernel::CancelOutcome`]).
    status: String,
    /// True when the run is (or already was) being cancelled — false only for a run
    /// that is not a cancellable in-flight process run.
    cancelling: bool,
    /// A short, honest human message for the UI.
    message: String,
}

/// POST `/v1/relux/runs/:id/cancel` — request mid-run cancellation of an in-flight,
/// process-backed adapter run (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26).
///
/// Session-gated (the operator console is the human authority). Validates the run
/// exists under the kernel lock (404 for an unknown run), then sets the cancel flag
/// in the lock-INDEPENDENT [`relux_kernel::RunCancellations`] registry so the
/// off-lock spawn kills its child within a poll tick. The run is then finalized as
/// [`relux_core::RunStatus::Cancelled`] by the orchestration driver's finalize phase.
///
/// HONEST: only a run that is actually streaming off-lock has a live cancel token.
/// A request for any other run — already finished, never started off-lock, or a
/// synchronous lock-holding run — returns `not_running` with `cancelling: false`
/// and a clear reason; the kernel never claims to cancel something it cannot reach.
/// A repeat request for an already-cancelling run is idempotent (`already_requested`).
async fn cancel_run(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<CancelRunResponse>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    // 404 only for an unknown run id (read-only validation under the lock).
    locked_read(&state, |kernel| {
        kernel
            .run(&run_id)
            .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
        Ok(())
    })?;
    // Set the cancel flag WITHOUT the kernel lock — the off-lock spawn polls it.
    let outcome = state.run_cancellations.request(&run_id);
    let message = match outcome {
        relux_kernel::CancelOutcome::Requested => {
            "Cancellation requested — the run is being stopped.".to_string()
        }
        relux_kernel::CancelOutcome::AlreadyRequested => {
            "This run is already being cancelled.".to_string()
        }
        relux_kernel::CancelOutcome::NotRunning => {
            "This run is not a cancellable in-flight process run (it already \
             finished, never started, or runs on the synchronous path)."
                .to_string()
        }
    };
    Ok(Json(CancelRunResponse {
        run_id: run_id.to_string(),
        status: outcome.as_str().to_string(),
        cancelling: outcome.is_cancelling(),
        message,
    }))
}

#[derive(Debug, Deserialize)]
struct ReviewProposedChangeReq {
    /// "approve" or "reject" — the operator's verdict on the proposed change.
    decision: String,
    /// An optional, bounded operator note recorded with the verdict.
    #[serde(default)]
    note: Option<String>,
}

/// POST `/v1/relux/runs/:id/proposed-changes/:index/review` — record an operator
/// accept/reject of one proposed change (master plan section 15). Returns the
/// updated run detail so the dashboard can refresh in one round trip. Never
/// applies anything; apply is a separate, explicit action.
async fn review_proposed_change(
    State(state): State<AppState>,
    AxumPath((id, index)): AxumPath<(String, usize)>,
    body: Option<Json<ReviewProposedChangeReq>>,
) -> Result<Json<RunRecord>, ApiError> {
    let req = body.map(|b| b.0).ok_or_else(|| {
        ApiError::bad_request("a JSON body with { \"decision\": \"approve|reject\" } is required")
    })?;
    let approve = match req.decision.trim().to_ascii_lowercase().as_str() {
        "approve" | "approved" | "accept" | "accepted" => true,
        "reject" | "rejected" => false,
        other => {
            return Err(ApiError::bad_request(format!(
                "decision must be 'approve' or 'reject', got '{other}'"
            )))
        }
    };
    let run_id = relux_core::RunId::new(id);
    let record = locked_save(&state, |kernel| {
        kernel.review_proposed_change(&run_id, index, approve, req.note.as_deref())?;
        let run = kernel
            .run(&run_id)
            .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?
            .clone();
        Ok(build_run_record(kernel, run))
    })?;
    Ok(Json(record))
}

/// POST `/v1/relux/runs/:id/proposed-changes/:index/apply` — apply one APPROVED
/// proposed change into the run's controlled workspace root (master plan section
/// 15 apply, safety bar section 17.5). Refuses honestly (no fabricated success):
/// 409 when the change is not approved or the baseline conflicts with the
/// workspace; 422 when it cannot be applied (no baseline hash, no workspace root,
/// unsafe/irregular target). Persists the recorded refusal reason even on a
/// refusal so the dashboard can show why.
async fn apply_proposed_change(
    State(state): State<AppState>,
    AxumPath((id, index)): AxumPath<(String, usize)>,
) -> Result<Json<relux_kernel::AppliedProposedChange>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    let applied =
        locked_save_persisting(&state, |kernel| kernel.apply_proposed_change(&run_id, index))?;
    Ok(Json(applied))
}

#[derive(Debug, Deserialize)]
struct ApplyProposedChangeSetReq {
    /// The explicit indices of the (already approved) proposed changes to apply as
    /// one transaction. Must be non-empty.
    indices: Vec<usize>,
}

/// POST `/v1/relux/runs/:id/proposed-changes/apply` — apply a SET of approved
/// proposed changes for one run as a single all-or-nothing transaction (master
/// plan section 15 apply, safety bar section 17.5). Every selected change is
/// validated together first (approved, baseline present + still matching, safe
/// distinct path, existing target); only if ALL pass are the files written (each
/// via a temp file then a rename, with rollback on a mid-apply write fault).
/// Refuses honestly with no
/// file modified — a baseline conflict with the workspace maps to a `409`, any
/// other inapplicable set (a change not approved, a missing baseline, no workspace
/// root, an unsafe or duplicate target, an unknown index) maps to a `422`, and a
/// request with no indices maps to a `400`.
async fn apply_proposed_change_set(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<ApplyProposedChangeSetReq>>,
) -> Result<Json<relux_kernel::AppliedProposedChangeSet>, ApiError> {
    let req = body.map(|b| b.0).ok_or_else(|| {
        ApiError::bad_request("a JSON body with { \"indices\": [..] } is required")
    })?;
    if req.indices.is_empty() {
        return Err(ApiError::bad_request(
            "at least one proposed-change index is required",
        ));
    }
    let run_id = relux_core::RunId::new(id);
    let applied = locked_save_persisting(&state, |kernel| {
        kernel.apply_proposed_change_set(&run_id, &req.indices)
    })?;
    Ok(Json(applied))
}

#[derive(Debug, Deserialize)]
struct PrimeReq {
    message: String,
}

/// The result of one Prime turn plus a fresh state summary, so the chat UI can
/// show what Prime did AND the updated control-plane counts in one round trip.
#[derive(Debug, Serialize)]
struct PrimeResponse {
    /// Flattened so the JSON carries `intent`, `reply`, `disposition`, `action`,
    /// `created_task`, `started_run`, `approval` at the top level.
    #[serde(flatten)]
    turn: PrimeTurn,
    state: StateResponse,
    /// Which path produced the reply (deterministic or LLM).
    ai_mode: AiMode,
    /// The model used, if LLM-backed.
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_model: Option<String>,
    /// A safe, non-secret note (e.g. why LLM was skipped or fell back).
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_note: Option<String>,
    /// Present only when a configured brain genuinely shaped this turn's intent
    /// (the value is `brain`). Absent for deterministic turns — including a brain
    /// proposal that the safety gate vetoed — so the UI attributes the brain only
    /// when it actually decided. Provenance only; never affects execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    intent_source: Option<String>,
    /// Present only when a configured brain re-worded a clarify / brainstorm turn's
    /// reply (the validated wording path). Advisory provenance for the small chip; the
    /// turn stays action-free and the wording was schema-validated. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_polish: Option<ReplyPolishProvenance>,
    /// Present only when, AFTER this turn, Prime is still waiting on the user to answer a
    /// clarifying question for an actionable request (`docs/prime-processing-audit.md`
    /// "Multi-turn clarify memory"). The dashboard renders a small "waiting for: …" chip
    /// with a cancel action; the next message will be read as the answer. Bounded,
    /// non-secret user text only. Absent when no clarification is pending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_clarification: Option<relux_core::PendingClarification>,
    /// Present ONLY when a single UNIFIED brain decision carried more than one proposal this
    /// turn (intent + slots + wording answered in one provider call). The value is the model
    /// id / CLI brain label. The chat renders one concise "one brain decision · <source>" chip;
    /// the per-section chips still attribute each piece. Provenance only; never affects state.
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_source: Option<String>,
    /// Present ONLY when the brain requested a governed WRITE-capable tool that genuinely drove
    /// this turn (the turn is actionful and its intent matches the tool). The value is the tool
    /// name (e.g. `task.update`). The chat renders a small "requested tool: <name>" provenance
    /// chip. The mutation still flowed through the unchanged fail-closed `decide` → `prime_execute`
    /// (safe `Act`) / human-approval (`Propose`) path; the brain wrote nothing directly. Absent on
    /// every turn with no honored write tool (including a vetoed one), so existing clients are
    /// unaffected.
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_tool: Option<String>,
    /// Present ONLY when a configured brain shaped this turn's POST-EXECUTION (after-action)
    /// reply — the action had ALREADY run (or been proposed) through the unchanged `decide` →
    /// `prime_execute` / approval path, and the brain re-worded the confirmation, grounded ONLY
    /// in a sanitized result envelope and validated against it (no claim of unexecuted work, no
    /// invented id, no "installed"/"granted" on a still-pending proposal). The value is the model
    /// id / CLI brain label. The chat renders a small "after-action wording · <source>" chip.
    /// Wording/provenance only; the brain changed no state. Absent on every turn where the reply
    /// stayed the grounded deterministic one (including Local / any failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    after_action_source: Option<String>,
}

/// Provenance for a brain-polished clarify / brainstorm reply: which KIND of wording
/// was re-shaped and which brain produced it. Presentation/provenance only.
#[derive(Debug, Serialize)]
struct ReplyPolishProvenance {
    /// "clarification" | "brainstorm".
    kind: String,
    /// The OpenRouter model id / CLI brain label that produced the wording.
    source: String,
}

/// Run exactly one durable Prime turn (`docs/RELUX_MASTER_PLAN.md` section 10) over
/// HTTP: the same grounded `prime_turn` the CLI uses, so a greeting stays a
/// greeting and "create a task to X" creates that task. Persisted under the lock
/// so the next turn (and the dashboard) sees the result.
///
/// If OpenRouter is configured, the conversational parts of the reply are
/// shaped by the LLM (while actions stay grounded and deterministic).
async fn run_prime(
    State(state): State<AppState>,
    Json(req): Json<PrimeReq>,
) -> Result<Json<PrimeResponse>, ApiError> {
    let message = req.message.trim().to_string();
    if message.is_empty() {
        return Err(ApiError::bad_request("message is required"));
    }

    // Resolve the AI config (and therefore the selected brain) live, so a brain
    // chosen from the dashboard takes effect without a restart.
    let ai_config = resolve_ai(&state);
    let brain = ai_config.effective_brain();
    let cli_adapter_id = match brain {
        relux_kernel::PrimeBrain::ClaudeCli => Some(relux_core::CLAUDE_CLI_ADAPTER_ID),
        relux_kernel::PrimeBrain::CodexCli => Some(relux_core::CODEX_CLI_ADAPTER_ID),
        _ => None,
    };

    // 0. Snapshot the brain's CLI adapter status (if any) under a short read-only
    // lock. A CLI brain needs this BEFORE the turn so it can classify intent (and
    // it is reused below for the reply/polish spawns), all OUTSIDE the main lock.
    let cli_status = if cli_adapter_id.is_some() {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        let store = SqliteStore::open(&state.db_path)?;
        let kernel = store.load()?;
        cli_adapter_id.and_then(|id| {
            kernel
                .adapter_runtime_status()
                .into_iter()
                .find(|a| a.plugin_id == id)
        })
    } else {
        None
    };

    // 0b. Continuation pre-flight + board snapshot (a short read under the lock). If this
    // message CONTINUES a pending clarification, the brain must reason about the COMBINED
    // message + the recorded intent — exactly what the kernel will reclassify under the lock —
    // not the bare answer. The board summary grounds an assignment/update against real ids. The
    // kernel re-decides the continuation authoritatively under its own lock; this preview only
    // steers which message the (slow, off-lock) brain is asked about.
    let (continuation, board_summary, context_snapshot, recent_history) = {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut kernel = {
            let store = SqliteStore::open(&state.db_path)?;
            store.load()?
        };
        let ctx = crate::ensure_bootstrapped(&mut kernel)?;
        let preview = kernel.continuation_preview(&ctx, &message);
        let summary = kernel.inspect_state();
        // Snapshot the read-only context the governed tool loop reads from (the whole board,
        // bounded), taken under THIS lock so the loop's brain rounds run lock-free below.
        let snapshot = kernel.context_snapshot(&ctx);
        // The bounded, secret-redacted recent-conversation context, so the (slow, off-lock) brain
        // can interpret a follow-up in context. Advisory BACKGROUND only — it is injected into the
        // decision prompt and never reaches the deterministic classifier or any gate.
        let history = kernel.recent_conversation_context(&ctx);
        (preview, summary, snapshot, history)
    };
    let is_continuation = continuation.is_some();
    // The message the brain reasons about: the COMBINED message on a continuation, else raw.
    let decision_message = match continuation.as_ref() {
        Some((combined, _)) => combined.clone(),
        None => message.clone(),
    };

    // 0c. UNIFIED brain decision via the bounded OBSERVE-THEN-ACT loop (OUTSIDE the lock). When a
    // brain is configured it answers the WHOLE turn at once: the proposed intent + whichever slots
    // apply + optional clarifying wording + at most one governed write tool, in ONE validated
    // envelope — the Hermes/Codex "one response carries the answer and the structured actions"
    // shape. The loop lets the brain inspect a LITTLE live state through the governed READ-ONLY
    // tools before it commits: each round it may either request read-only tools (the kernel runs
    // them deterministically against the pre-taken snapshot and re-asks, grounded in the results)
    // or commit its decision, bounded by `MAX_DECISION_ROUNDS`. So a single actionful turn can
    // observe live state, choose its one action grounded in what it saw, and the action then flows
    // through the UNCHANGED fail-closed gate + `decide` → `prime_execute` / approval — the loop adds
    // no new authority and has no mutation path. The reads it gathered (`observed_reads`) are
    // surfaced as provenance and ground the reply. ANY failure → `None`, and the specialized
    // per-section stack below runs as the fallback (§10.1, §10.2, §17.1;
    // `docs/prime-processing-audit.md` "observe-then-act decision loop").
    let (decision, observed_reads) = decide_prime_with_observation(
        brain,
        &ai_config,
        cli_status.clone(),
        &context_snapshot,
        &decision_message,
        &board_summary,
        &recent_history,
    )
    .await;

    // 0d. Derive the intent proposal + the slot bundle. PREFERRED: the one unified decision
    // (no further brain calls this turn). FALLBACK: when the unified call produced nothing
    // usable (no brain, disabled, malformed/empty envelope), the prior specialized stack runs —
    // a dedicated intent call, then a dedicated slot call for the resolved intent — so behavior
    // is byte-for-byte the old path whenever the unified shape is unavailable. Either way the
    // kernel validates every section; it uses only the sections that match the turn it produces
    // (a `task` proposal on an assign turn is simply ignored), and on a continuation it drops
    // the intent proposal and keeps the slot bundle only because `continuation` matches below.
    let intent_proposal: Option<relux_kernel::BrainIntentProposal>;
    let mut task_slots: Option<relux_kernel::BrainTaskSlots> = None;
    let mut agent_slots: Option<relux_kernel::BrainAgentSlots> = None;
    let mut plugin_ref: Option<relux_kernel::BrainPluginRef> = None;
    let mut permission_slots: Option<relux_kernel::BrainPermissionSlots> = None;
    let mut assign_slots: Option<relux_kernel::BrainAssignSlots> = None;
    let mut update_slots: Option<relux_kernel::BrainUpdateSlots> = None;
    let mut run_slots: Option<relux_kernel::BrainRunStart> = None;
    let mut orchestration_slots: Option<relux_kernel::BrainOrchestrationSlots> = None;
    let mut run_orchestration_slots: Option<relux_kernel::BrainRunOrchestration> = None;
    if let Some(d) = decision.as_ref() {
        if let Some(wt) = d.action_request.as_ref() {
            // A WRITE tool request is the authority for this turn's intent + its one slot. Its
            // synthesized intent flows through the UNCHANGED fail-closed gate (so guarded chat
            // still vetoes a mutating tool), and only the matching validated slot is fed to the
            // kernel chokepoint — the loose classification / slot sections are ignored in favor
            // of the explicitly named tool. The kernel still validates every id and gates a
            // risky action behind a human approval (§10.1, §10.2, §17.1).
            intent_proposal = Some(wt.intent_proposal());
            match &wt.slot {
                relux_kernel::WriteToolSlot::Task(s) => task_slots = Some(s.clone()),
                relux_kernel::WriteToolSlot::Update(s) => update_slots = Some(s.clone()),
                relux_kernel::WriteToolSlot::Assign(s) => assign_slots = Some(s.clone()),
                relux_kernel::WriteToolSlot::Agent(s) => agent_slots = Some(s.clone()),
                relux_kernel::WriteToolSlot::Plugin(s) => plugin_ref = Some(s.clone()),
                relux_kernel::WriteToolSlot::Permission(s) => permission_slots = Some(s.clone()),
                relux_kernel::WriteToolSlot::RunStart(s) => run_slots = Some(s.clone()),
                relux_kernel::WriteToolSlot::Orchestration(s) => {
                    orchestration_slots = Some(s.clone())
                }
                relux_kernel::WriteToolSlot::RunOrchestration(s) => {
                    run_orchestration_slots = Some(s.clone())
                }
            }
        } else {
            intent_proposal = d.classification.clone();
            task_slots = d.task.clone();
            agent_slots = d.agent.clone();
            plugin_ref = d.plugin.clone();
            permission_slots = d.permission.clone();
            assign_slots = d.assign.clone();
            update_slots = d.update.clone();
        }
    } else {
        // Specialized fallback (the prior multi-call stack), reached only when the unified
        // decision was unavailable. A dedicated intent proposal, then a dedicated slot call for
        // the message + intent the kernel will act on.
        intent_proposal = match brain {
            relux_kernel::PrimeBrain::Openrouter => {
                relux_kernel::classify_intent_via_openrouter(&ai_config, &message).await
            }
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                classify_intent_via_cli(brain, cli_status.clone(), &message).await
            }
            relux_kernel::PrimeBrain::Local => None,
        };
        let (slot_message, slot_intent) = match continuation.as_ref() {
            Some((combined, intent)) => (combined.clone(), intent.clone()),
            None => {
                let deterministic_intent = relux_kernel::classify_intent(&message);
                let resolved_intent = match intent_proposal.as_ref() {
                    Some(p) => {
                        relux_kernel::reconcile_intent(deterministic_intent.clone(), p, &message).0
                    }
                    None => deterministic_intent,
                };
                (message.clone(), resolved_intent)
            }
        };
        use relux_core::PrimeIntent as I;
        match slot_intent {
            I::TaskCreation | I::CreateAndRunTask => {
                task_slots = match brain {
                    relux_kernel::PrimeBrain::Openrouter => {
                        relux_kernel::extract_task_slots_via_openrouter(&ai_config, &slot_message)
                            .await
                    }
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        extract_task_slots_via_cli(brain, cli_status.clone(), &slot_message).await
                    }
                    relux_kernel::PrimeBrain::Local => None,
                };
            }
            I::AgentCreation => {
                agent_slots = match brain {
                    relux_kernel::PrimeBrain::Openrouter => {
                        relux_kernel::extract_agent_slots_via_openrouter(&ai_config, &slot_message)
                            .await
                    }
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        extract_agent_slots_via_cli(brain, cli_status.clone(), &slot_message).await
                    }
                    relux_kernel::PrimeBrain::Local => None,
                };
            }
            I::PluginInstallation => {
                plugin_ref = match brain {
                    relux_kernel::PrimeBrain::Openrouter => {
                        relux_kernel::extract_plugin_ref_via_openrouter(&ai_config, &slot_message)
                            .await
                    }
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        extract_plugin_ref_via_cli(brain, cli_status.clone(), &slot_message).await
                    }
                    relux_kernel::PrimeBrain::Local => None,
                };
            }
            I::PermissionChange => {
                permission_slots = match brain {
                    relux_kernel::PrimeBrain::Openrouter => {
                        relux_kernel::extract_permission_slots_via_openrouter(
                            &ai_config,
                            &slot_message,
                        )
                        .await
                    }
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        extract_permission_slots_via_cli(brain, cli_status.clone(), &slot_message)
                            .await
                    }
                    relux_kernel::PrimeBrain::Local => None,
                };
            }
            I::AssignTask => {
                assign_slots = match brain {
                    relux_kernel::PrimeBrain::Openrouter => {
                        relux_kernel::extract_assign_slots_via_openrouter(
                            &ai_config,
                            &slot_message,
                            &board_summary,
                        )
                        .await
                    }
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        extract_assign_slots_via_cli(
                            brain,
                            cli_status.clone(),
                            &slot_message,
                            &board_summary,
                        )
                        .await
                    }
                    relux_kernel::PrimeBrain::Local => None,
                };
            }
            I::TaskUpdate => {
                update_slots = match brain {
                    relux_kernel::PrimeBrain::Openrouter => {
                        relux_kernel::extract_update_slots_via_openrouter(
                            &ai_config,
                            &slot_message,
                            &board_summary,
                        )
                        .await
                    }
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        extract_update_slots_via_cli(
                            brain,
                            cli_status.clone(),
                            &slot_message,
                            &board_summary,
                        )
                        .await
                    }
                    relux_kernel::PrimeBrain::Local => None,
                };
            }
            _ => {}
        }
    }

    // 1. Run the deterministic kernel turn (must happen under the lock), passing
    // the optional brain intent proposal AND the slot bundle so the kernel reconciles
    // + audits the final intent and validates every slot at its single chokepoint.
    // `intent_source` records who decided.
    let (turn, summary, intent_source, pending_clarification) = {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = SqliteStore::open(&state.db_path)?;
        let mut kernel = store.load()?;
        let ctx = crate::ensure_bootstrapped(&mut kernel)?;
        let (turn, intent_source) = kernel.prime_turn_with_brain(
            &ctx,
            &message,
            intent_proposal.as_ref(),
            relux_kernel::BrainSlotProposals {
                task: task_slots.as_ref(),
                agent: agent_slots.as_ref(),
                plugin: plugin_ref.as_ref(),
                permission: permission_slots.as_ref(),
                assign: assign_slots.as_ref(),
                update: update_slots.as_ref(),
                run: run_slots.as_ref(),
                orchestration: orchestration_slots.as_ref(),
                run_orchestration: run_orchestration_slots.as_ref(),
                continuation: is_continuation,
            },
        )?;
        // Read back any pending clarification this turn LEFT active, so the chat can
        // show the "waiting for: …" chip. Read under the same lock, after the turn.
        let pending_clarification = kernel.pending_clarification_for(&ctx);
        let summary = state_response(&kernel, &state.db_path);
        store.save(&kernel)?;
        (turn, summary, intent_source, pending_clarification)
    };

    // 2. Produce the conversational reply through the selected brain. Actions are
    // never delegated: an actionful turn (a real state change / approval / tool
    // result) always keeps the grounded deterministic reply. Conversational turns
    // route to the chosen brain. This happens OUTSIDE the lock because it can
    // involve a slow network/process call.
    //
    // A clarify / brainstorm turn is the special case: the brain may re-WORD it, but
    // through the VALIDATED wording path (one schema-checked, length-bounded question
    // or short summary — never free-form prose that could lecture or claim an action).
    // So those turns skip the free-form shaper and go through `run_clarify_polish`,
    // which falls back to the grounded deterministic wording on any failure (§10.5,
    // §17.1). The wall is intact: this only ever runs on a non-actionful turn.
    // The READ-ONLY context tools the governed observe-then-act loop gathered this turn (empty
    // unless the brain requested read-only tools before committing), surfaced as provenance on the
    // response and used to ground a conversational reply. Seeded from the decision loop above so an
    // ACTIONFUL turn that observed first also shows its reads.
    let mut gathered_reads: Vec<relux_kernel::ContextRead> = observed_reads;
    // Set when the brain shaped this turn's POST-EXECUTION (after-action) reply — provenance for
    // the small chip; the action already ran through the unchanged path, so this is wording only.
    let mut after_action_source: Option<String> = None;
    let clarify_kind = relux_kernel::clarify_polish_kind(&turn);
    let (outcome, reply_polish) = if let Some(kind) = clarify_kind {
        // Prefer the wording the UNIFIED decision already carried (no extra brain call); it is
        // validated by the SAME `parse_clarify`/`reconcile_clarify` chokepoint via
        // `validated_wording`. When the unified envelope omitted wording or it failed
        // validation, fall back to a dedicated clarify-polish call. A failure on either path
        // leaves the grounded deterministic wording in place.
        let precomputed = decision
            .as_ref()
            .and_then(|d| d.validated_wording(kind, &turn.reply));
        run_clarify_polish(
            brain,
            &ai_config,
            cli_status.clone(),
            &message,
            &turn,
            kind,
            precomputed,
        )
        .await
    } else if !relux_kernel::is_actionful(&turn)
        && relux_kernel::turn_wants_context(&turn)
        && !matches!(brain, relux_kernel::PrimeBrain::Local)
    {
        // An inspection / explanation / conversational-QUESTION turn: the brain may inspect live
        // state before answering. The OBSERVE-THEN-ACT decision loop above already ran the brain's
        // read-only `tool_requests` between its decision rounds and accumulated the observations
        // (`gathered_reads`). Prefer those — no second gather, no duplicate execution. FALLBACK:
        // only when the loop gathered nothing (the unified decision requested no tools, or there was
        // no usable decision) run the sidecar `ContextLoop` exactly as before, so an inspection turn
        // still inspects live state before answering — byte-for-byte the prior behavior. Either way
        // the reply is shaped grounded in what was actually observed; the reads change nothing and
        // never reach an action (`docs/prime-processing-audit.md` "observe-then-act decision loop").
        if gathered_reads.is_empty() {
            gathered_reads = gather_read_only_context(
                brain,
                &ai_config,
                cli_status.clone(),
                &context_snapshot,
                &message,
            )
            .await;
        }
        let observations = relux_kernel::render_observations(&gathered_reads);
        let outcome = match brain {
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                run_cli_brain(brain, cli_status.clone(), &message, &turn, &observations).await
            }
            _ => relux_kernel::shape_reply(&ai_config, &message, &turn, &observations).await,
        };
        (outcome, None)
    } else if !relux_kernel::is_actionful(&turn) {
        // A non-actionful, non-clarify conversational turn that does not benefit from a state
        // lookup (a greeting / plan prose / a turn under the Local brain). PREFER the free-form
        // reply the UNIFIED decision already carried — no extra brain call — validated through the
        // SAME block-sanitize + action-claim chokepoint a brainstorm reply uses (`validated_reply`).
        // On any miss fall back to the dedicated free-form shaper, so behavior is byte-for-byte the
        // prior path. The action-free wall is intact: this only runs on a non-actionful turn.
        let precomputed_reply = decision.as_ref().and_then(|d| d.validated_reply(&turn.reply));
        match precomputed_reply {
            Some(text) => (unified_reply_outcome(brain, &ai_config, text), None),
            None if matches!(
                brain,
                relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli
            ) =>
            {
                (run_cli_brain(brain, cli_status.clone(), &message, &turn, "").await, None)
            }
            None => {
                // Local / OpenRouter go through shape_reply, which only augments via OpenRouter
                // when that brain is selected and a key is configured.
                (relux_kernel::shape_reply(&ai_config, &message, &turn, "").await, None)
            }
        }
    } else if let Some(kind) = relux_kernel::after_action_kind(&turn)
        .filter(|_| !matches!(brain, relux_kernel::PrimeBrain::Local))
    {
        // An ACTIONFUL turn under a configured brain. The brain composed its decision BEFORE the
        // kernel executed, so it could not honestly narrate the result — that is why an actionful
        // reply has always stayed deterministic. This is the deferred "after-action narration"
        // pass: the action has ALREADY run (or been proposed) through the unchanged `decide` →
        // `prime_execute` / approval path, and the brain now re-words the FINAL confirmation,
        // grounded ONLY in a sanitized result envelope and validated against it — it may not claim
        // unexecuted work, invent an id, say installed/granted on a still-pending proposal, or
        // narrate a failure as success (`docs/prime-processing-audit.md` "after-action narration").
        // On ANY failure (malformed reply, contradiction, invented id, low confidence, echo, or an
        // unavailable adapter) we fall back to the grounded deterministic reply via `shape_reply`,
        // which keeps the turn deterministic — byte-for-byte the prior behavior. The action-free
        // wall holds: nothing here changes state; only the confirmation wording can change.
        let envelope = relux_kernel::build_action_envelope(&turn, kind);
        let shaped = match brain {
            relux_kernel::PrimeBrain::Openrouter => {
                relux_kernel::polish_after_action_via_openrouter(&ai_config, &message, &envelope)
                    .await
            }
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                polish_after_action_via_cli(brain, cli_status.clone(), &message, &envelope).await
            }
            relux_kernel::PrimeBrain::Local => None,
        };
        match shaped {
            Some(text) => {
                after_action_source = Some(slot_source_label(brain, &ai_config));
                (unified_reply_outcome(brain, &ai_config, text), None)
            }
            None => (relux_kernel::shape_reply(&ai_config, &message, &turn, "").await, None),
        }
    } else {
        // A non-shapeable actionful turn (a tool result, or the Local brain): keep the grounded
        // deterministic reply so the brain never narrates (and possibly overclaims) a state change.
        (relux_kernel::shape_reply(&ai_config, &message, &turn, "").await, None)
    };

    // 3. Merge the outcome into the response.
    let mut final_turn = turn;
    final_turn.reply = outcome.reply;
    // Surface the read-only context reads the governed loop gathered (if any) as provenance. The
    // full result bodies stayed server-side grounding; only the bounded summaries ship.
    final_turn.context_reads = relux_kernel::reads_to_wire(&gathered_reads);

    // 4. OPTIONAL advisory polish of a plan-preview card. Whichever brain is live
    // may refine only the WORDING of the proposal (summary, step titles, clarifying
    // questions, risk notes) — OpenRouter over HTTP, the Claude/Codex CLI brains via
    // a bounded adapter spawn. Both feed the SAME `validate_polish` chokepoint, so
    // neither can change step count, order, or agent ids. This is presentation-only
    // and happens OUTSIDE the lock. It is gated on a NON-actionful turn: only a
    // PlanRequest carries a proposal (the commit path is a separate Orchestration
    // turn with no proposal), so the authoritative steps/agents/goal — and the
    // "Create these tasks" commit — are never touched, and a skip/error/invalid
    // reply simply leaves the deterministic preview in place (§10 planning layer,
    // §11.1, §17.1).
    if !relux_kernel::is_actionful(&final_turn) {
        if let Some(proposal) = final_turn.proposal.clone() {
            // PREFER the polish the UNIFIED decision already carried — no extra brain call —
            // validated against the AUTHORITATIVE proposal through the SAME `validate_polish`
            // chokepoint (step count / order / agent ids immutable; a step title applies only on
            // an exact index match). Fall back to a dedicated polish call when the unified
            // envelope omitted it or it yielded nothing usable, so behavior is byte-for-byte the
            // prior path. Single-step proposals carry nothing to refine and skip for every brain.
            let precomputed = if relux_kernel::proposal_wants_polish(&proposal) {
                decision.as_ref().and_then(|d| {
                    d.validated_polish(&proposal, &slot_source_label(brain, &ai_config))
                })
            } else {
                None
            };
            let polish = match precomputed {
                Some(p) => Some(p),
                None => match brain {
                    relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                        polish_proposal_via_cli(brain, cli_status.clone(), &proposal).await
                    }
                    _ => relux_kernel::polish_proposal(&ai_config, &proposal).await,
                },
            };
            if let Some(polish) = polish {
                if let Some(p) = final_turn.proposal.as_mut() {
                    p.polish = Some(polish);
                }
            }
        }
    }

    // Stamp the slot provenance label (the OpenRouter model id / CLI brain label
    // that produced the validated slots) so the card can attribute them, exactly as
    // the intent and polish provenance do. The kernel left `source` unset (it knows
    // only that the brain assisted); the server, which knows the brain + model,
    // fills it in. Present only when the kernel attached brain-assisted slots.
    if let Some(slots) = final_turn.slots.as_mut() {
        slots.source = Some(slot_source_label(brain, &ai_config));
    }
    // The same provenance stamping for the brain-assisted agent slots (a sharpened
    // `AgentCreation`) and the advisory admin slots (a sharpened plugin/permission
    // `Propose`). Present only when the kernel attached them.
    if let Some(slots) = final_turn.agent_slots.as_mut() {
        slots.source = Some(slot_source_label(brain, &ai_config));
    }
    if let Some(slots) = final_turn.admin_slots.as_mut() {
        slots.source = Some(slot_source_label(brain, &ai_config));
    }
    // And the brain-resolved assignment slots (a promoted `AssignTask`).
    if let Some(slots) = final_turn.assign_slots.as_mut() {
        slots.source = Some(slot_source_label(brain, &ai_config));
    }
    // The by-id update card carries a brain `source` ONLY when the kernel marked it
    // brain-resolved (a deterministically-parsed update leaves `source` None and shows
    // no chip). Replace the kernel's marker with the real model / CLI label.
    if let Some(update) = final_turn.update.as_mut() {
        if update.source.is_some() {
            update.source = Some(slot_source_label(brain, &ai_config));
        }
    }

    // Surface intent provenance only when the brain genuinely drove the intent (a
    // vetoed or low-confidence proposal stays deterministic and shows nothing).
    let intent_source_label = match intent_source {
        relux_kernel::IntentSource::Brain => Some("brain".to_string()),
        relux_kernel::IntentSource::Deterministic => None,
    };

    // The unified-decision provenance: shown ONLY when ONE brain call produced more than one
    // proposal (the thing that distinguishes the unified path from the prior serial calls). The
    // per-section chips already attribute each piece; this names the single decision behind
    // them, so the chat shows one concise "from one brain decision" label instead of a panel.
    let decision_source = decision
        .as_ref()
        .filter(|d| d.section_count() >= 2)
        .map(|_| slot_source_label(brain, &ai_config));

    // The governed WRITE-tool provenance: present ONLY when the brain requested a write tool AND
    // that request genuinely drove this turn — the turn is actionful (a real `Act` / approval) and
    // its intent matches the tool's mapped intent. So a write tool the fail-closed gate vetoed (a
    // mutating request on guarded chat) attributes NOTHING, keeping the chip honest. The chat
    // renders a small "requested tool: task.update" chip; the action still flowed through the
    // unchanged validation/approval path.
    let requested_tool = decision
        .as_ref()
        .and_then(|d| d.action_request.as_ref())
        .filter(|wt| relux_kernel::is_actionful(&final_turn) && wt.intent == final_turn.intent)
        .map(|wt| wt.tool.clone());

    // 5. Record a bounded, secret-redacted slice of THIS turn into the per-conversation memory so
    // the NEXT turn's brain can interpret a follow-up in context. Done AFTER the reply is shaped
    // (`final_turn.reply` is now the FINAL user-visible reply — a validated brain-shaped /
    // after-action wording when one ran, never the earlier deterministic draft) and the read-only
    // context gathered, so the stored reply + reads match what the user actually saw. A short lock
    // of its own (after the turn's own save); it stores only advisory grounding (the final reply,
    // the ids the turn created, and the read-only tools consulted as name + their bounded one-line
    // summary — never a raw provider envelope or full tool JSON), and grants no authority
    // (`docs/prime-processing-audit.md` "Bounded conversation memory").
    {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = SqliteStore::open(&state.db_path)?;
        let mut kernel = store.load()?;
        let ctx = crate::ensure_bootstrapped(&mut kernel)?;
        // The combined message on a continuation (what the turn actually answered), else the raw
        // user message.
        let recorded_message = match continuation.as_ref() {
            Some((combined, _)) => combined.clone(),
            None => message.clone(),
        };
        // The bounded context reads (name + already-redacted/clamped summary) the turn shipped as
        // provenance; `build_turn` re-redacts + clamps each entry. The full result bodies stayed
        // server-side grounding and are never persisted.
        kernel.record_conversation_turn(
            &ctx,
            &recorded_message,
            &final_turn,
            &final_turn.context_reads,
        );
        store.save(&kernel)?;
    }

    Ok(Json(PrimeResponse {
        turn: final_turn,
        state: summary,
        ai_mode: outcome.mode,
        ai_model: outcome.model,
        ai_note: outcome.note,
        intent_source: intent_source_label,
        reply_polish,
        pending_clarification,
        decision_source,
        requested_tool,
        after_action_source,
    }))
}

/// The JSON body returned by the conversation-reset endpoint.
#[derive(Debug, Serialize)]
struct ResetPrimeResponse {
    /// True when there was advisory memory (history and/or a pending clarification) to clear.
    cleared: bool,
}

/// Clear the caller's Prime conversation memory — the bounded recent-turn history and any
/// pending clarification (`docs/prime-processing-audit.md` "Bounded conversation memory"). This
/// drops ONLY advisory context so a fresh conversation starts clean; no durable entity
/// (task / run / agent / approval) is touched, so a reset can never lose real work.
async fn reset_prime_conversation(
    State(state): State<AppState>,
) -> Result<Json<ResetPrimeResponse>, ApiError> {
    let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = SqliteStore::open(&state.db_path)?;
    let mut kernel = store.load()?;
    let ctx = crate::ensure_bootstrapped(&mut kernel)?;
    let cleared = kernel.clear_conversation(&ctx);
    if cleared {
        store.save(&kernel)?;
    }
    Ok(Json(ResetPrimeResponse { cleared }))
}

/// Cap on a CLI brain's reply, mirroring the OpenRouter reply cap.
const CLI_REPLY_MAX_CHARS: usize = 4_000;

/// Shape a CLI brain's captured `stdout` into the human answer to show in chat,
/// or `Err(note)` when the envelope reported an error / produced no usable text.
///
/// This is the seam that guarantees the chat bubble shows only the human reply,
/// never the raw `--output-format json` result envelope. The Claude CLI emits a
/// single JSON envelope (`{ "type":"result", "result":"...", "is_error":false,
/// "usage":{...}, "duration_ms":.., "session_id":".." }`); Codex `exec` and text
/// mode emit plain prose. [`parse_adapter_result`] lifts the human text out of a
/// recognized envelope and degrades to the raw text otherwise, exactly as the
/// assigned-run path does (master plan section 9.6). Kept pure so the
/// no-raw-JSON contract is pinned by unit tests without spawning a process.
fn shape_cli_brain_reply(
    stdout: &str,
    stdout_truncated: bool,
    kind: relux_core::AdapterKind,
    label: &str,
) -> Result<String, String> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    // An envelope can report an error even on a clean process exit. Surface it
    // honestly as a fallback note instead of presenting the error text (or, worse,
    // the raw JSON) as a confident answer.
    if parsed.is_error == Some(true) {
        let detail = parsed
            .text
            .lines()
            .next()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .unwrap_or("the CLI reported an error");
        return Err(format!(
            "{label} reported an error ({detail}); showing the grounded reply. \
             Check that the CLI is logged in and try again."
        ));
    }
    let answer = parsed.text.trim();
    if answer.is_empty() {
        return Err(format!(
            "{label} returned an empty answer; showing the grounded reply. Please try again."
        ));
    }
    let mut reply: String = answer.chars().take(CLI_REPLY_MAX_CHARS).collect();
    // A structured envelope that parsed cleanly carries the complete `result`
    // text, so the byte-cap on raw stdout did not cut the answer. Only flag
    // truncation for unstructured (plain-text) output.
    if stdout_truncated && !parsed.structured {
        reply.push_str("\n\n[output truncated]");
    }
    Ok(reply)
}

/// Build an honest advisory note when a *conversational* brain envelope declared
/// structured **proposed file changes**, or `None` when it declared none.
///
/// Design decision (master plan §15 + the AI "Conversational Shaping / Actionful
/// Safety" section): the Prime chat/brain path is **action-free by design** — it
/// only runs on non-actionful turns ([`relux_kernel::is_actionful`]), the chat
/// prompt ([`relux_kernel::compose_chat_prompt`]) forbids claiming any state
/// change, and [`run_cli_brain`] "only ever shapes a conversational reply; it
/// never performs a durable action". So, unlike the assigned-run path
/// ([`relux_core::Run::proposed_changes`], master plan §15), a chat turn does NOT
/// capture proposed changes into a run: there is no documented chat-turn run to
/// hang a review/apply flow on, and synthesizing one would manufacture hidden,
/// mutable work from a casual chat message (explicitly out of scope). Read-only
/// `artifacts` references are likewise not captured on this path.
///
/// Dropping them *silently* would be dishonest, though — so when an envelope from
/// a chat turn does declare proposed changes, we surface a bounded, secret-free
/// note telling the operator what was proposed and how to get a real, reviewable/
/// applyable run (the documented assigned-run path on Work). Pure: parses the
/// already-redacted stdout, spawns nothing, and persists nothing.
fn brain_envelope_advisory(
    stdout: &str,
    kind: relux_core::AdapterKind,
    label: &str,
) -> Option<String> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    let changes = parsed.proposed_changes.len();
    if changes == 0 {
        return None;
    }
    let noun = if changes == 1 {
        "file change"
    } else {
        "file changes"
    };
    Some(format!(
        "{label} proposed {changes} {noun} during this chat turn. Chat turns are \
         action-free, so nothing was captured for review or applied. To get \
         reviewable, applyable changes, create a task assigned to this adapter and \
         run it on Work — that path captures proposed changes with the safe \
         review/apply flow."
    ))
}

/// Delegate one conversational Prime turn to a local CLI brain (Claude / Codex).
///
/// Safety + honesty contract (`docs/RELUX_MASTER_PLAN.md` section 8.1, section
/// 17.5): the CLI is spawned in the same bounded, non-bypass mode the assigned-run
/// path uses (argv-only, prompt on stdin, wall-clock timeout, output cap, secret
/// redaction). It only ever *shapes a conversational reply*; it never performs a
/// durable action. If the adapter is missing / disabled / off-PATH, this returns
/// the grounded deterministic reply with a clear, actionable note instead of a
/// blank or fabricated answer.
async fn run_cli_brain(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    turn: &PrimeTurn,
    observations: &str,
) -> AiOutcome {
    let (label, bin, kind, mode) = match brain {
        relux_kernel::PrimeBrain::ClaudeCli => (
            "Claude CLI",
            "claude",
            relux_core::AdapterKind::ClaudeCli,
            AiMode::ClaudeCli,
        ),
        relux_kernel::PrimeBrain::CodexCli => (
            "Codex CLI",
            "codex",
            relux_core::AdapterKind::CodexCli,
            AiMode::CodexCli,
        ),
        // Not a CLI brain — caller never routes these here.
        _ => return AiOutcome::deterministic_fallback(turn.reply.clone(), None),
    };

    // A clear, actionable fallback that keeps the grounded reply.
    let fallback = |note: String| AiOutcome::deterministic_fallback(turn.reply.clone(), Some(note));

    let Some(st) = status else {
        return fallback(format!(
            "{label} is selected as Prime's brain, but its adapter is not installed. \
             Install the `{bin}` CLI and enable its adapter on Crew → Adapters."
        ));
    };

    // The adapter must be enabled with its binary resolved on PATH.
    if st.state != relux_core::AdapterRuntimeState::Available {
        let next = match st.state {
            relux_core::AdapterRuntimeState::MissingBinary => format!(
                "install the `{bin}` CLI and make sure it is on PATH, then refresh on Crew → Adapters"
            ),
            relux_core::AdapterRuntimeState::Disabled
            | relux_core::AdapterRuntimeState::NeedsConfiguration => {
                "enable it on Crew → Adapters (it is disabled by default)".to_string()
            }
            _ => "configure it on Crew → Adapters".to_string(),
        };
        return fallback(format!(
            "{label} is selected as Prime's brain, but its adapter is not ready ({}). To use it, {next}.",
            st.state.as_str()
        ));
    }

    let Some(program) = st.resolved_path.clone() else {
        return fallback(format!(
            "{label} is selected, but the `{bin}` binary could not be resolved on PATH. \
             Reinstall it or set an explicit command on Crew → Adapters."
        ));
    };

    // Fold the read-only observations the governed tool loop gathered this turn (if any) into the
    // grounded facts the CLI brain answers from; the fallback reply stays `turn.reply`.
    let grounded = relux_kernel::grounded_facts_with_observations(&turn.reply, observations);
    let prompt = relux_kernel::compose_chat_prompt(message, &grounded);
    let spec = relux_kernel::AdapterCommandSpec {
        program,
        args: relux_kernel::build_adapter_args(&kind),
        stdin: prompt,
        working_dir: st.working_dir.clone(),
        timeout: std::time::Duration::from_secs(
            st.timeout_seconds
                .unwrap_or(relux_core::DEFAULT_ADAPTER_TIMEOUT_SECONDS),
        ),
        max_output_bytes: st
            .max_output_bytes
            .unwrap_or(relux_core::DEFAULT_ADAPTER_MAX_OUTPUT_BYTES) as usize,
    };

    // The spawn is blocking (poll loop); keep it off the async reactor.
    let run = tokio::task::spawn_blocking(move || relux_kernel::run_adapter_command(&spec)).await;

    match run {
        Ok(Ok(outcome)) if outcome.success && !outcome.stdout.trim().is_empty() => {
            match shape_cli_brain_reply(&outcome.stdout, outcome.stdout_truncated, kind.clone(), label) {
                Ok(reply) => AiOutcome {
                    mode,
                    reply,
                    model: Some(label.to_string()),
                    // The chat path never captures proposed changes into a run
                    // (action-free by design); if the envelope declared any, say
                    // so honestly and point at the documented assigned-run path.
                    note: brain_envelope_advisory(&outcome.stdout, kind, label),
                },
                Err(note) => fallback(note),
            }
        }
        Ok(Ok(outcome)) if outcome.timed_out => fallback(format!(
            "{label} timed out after {}s; showing the grounded reply. Raise the timeout on Crew → Adapters or try again.",
            st.timeout_seconds.unwrap_or(relux_core::DEFAULT_ADAPTER_TIMEOUT_SECONDS)
        )),
        Ok(Ok(outcome)) => {
            // Ran but produced no usable answer (non-zero exit or empty stdout).
            let detail = outcome
                .stderr
                .lines()
                .next()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| format!("exit code {:?}", outcome.exit_code));
            fallback(format!(
                "{label} did not return an answer ({detail}); showing the grounded reply. \
                 Check that the CLI is logged in and try again."
            ))
        }
        Ok(Err(e)) => fallback(format!(
            "{label} could not be started ({e}); showing the grounded reply. \
             Verify the `{bin}` CLI is installed and enabled on Crew → Adapters."
        )),
        Err(_) => fallback(format!(
            "{label} run was interrupted; showing the grounded reply. Please try again."
        )),
    }
}

/// Classify one user message into a structured intent via a local CLI brain
/// (Claude / Codex). Returns a validated [`relux_kernel::BrainIntentProposal`] or
/// `None` on ANY failure (adapter missing/disabled/off-PATH, spawn error,
/// timeout, error envelope, or an unparseable reply) — every failure path lands
/// on the deterministic classifier, so the brain is strictly additive (§10.1,
/// §17.1).
///
/// No-leak seam: the spawn uses the same bounded, non-bypass mode the assigned-run
/// path uses (argv-only, prompt on stdin, wall-clock timeout, output cap, secret
/// redaction), and the captured stdout is lifted by [`parse_adapter_result`] FIRST
/// — so the raw `--output-format json` envelope never reaches the classifier or
/// the UI — and only the lifted human text is handed to
/// [`relux_kernel::parse_intent_proposal`], which validates it against the intent
/// allowlist. It performs NO durable action: it only proposes an intent the kernel
/// then reconciles behind the fail-closed gate.
async fn classify_intent_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
) -> Option<relux_kernel::BrainIntentProposal> {
    let kind = match brain {
        relux_kernel::PrimeBrain::ClaudeCli => relux_core::AdapterKind::ClaudeCli,
        relux_kernel::PrimeBrain::CodexCli => relux_core::AdapterKind::CodexCli,
        // Not a CLI brain — caller never routes these here.
        _ => return None,
    };
    // Missing / disabled / off-PATH adapter: no classification, just fall back to
    // the deterministic classifier (the conversational reply path carries any
    // actionable note about the adapter on this same turn).
    let st = status?;
    if st.state != relux_core::AdapterRuntimeState::Available {
        return None;
    }
    let program = st.resolved_path.clone()?;

    let prompt = relux_kernel::build_intent_prompt(message);
    let spec = relux_kernel::AdapterCommandSpec {
        program,
        args: relux_kernel::build_adapter_args(&kind),
        stdin: prompt,
        working_dir: st.working_dir.clone(),
        timeout: std::time::Duration::from_secs(
            st.timeout_seconds
                .unwrap_or(relux_core::DEFAULT_ADAPTER_TIMEOUT_SECONDS),
        ),
        max_output_bytes: st
            .max_output_bytes
            .unwrap_or(relux_core::DEFAULT_ADAPTER_MAX_OUTPUT_BYTES) as usize,
    };

    // The spawn is blocking (poll loop); keep it off the async reactor.
    let run = tokio::task::spawn_blocking(move || relux_kernel::run_adapter_command(&spec)).await;
    let outcome = match run {
        Ok(Ok(outcome)) if outcome.success && !outcome.stdout.trim().is_empty() => outcome,
        // Timeout / non-zero exit / spawn error / interruption: fall back silently.
        _ => return None,
    };

    parse_cli_intent(&outcome.stdout, kind)
}

/// Lift a validated intent proposal out of a CLI brain's captured `stdout`, or
/// `None`. This is the no-leak parse boundary, kept pure so it is pinned by unit
/// tests WITHOUT spawning a process: [`parse_adapter_result`] lifts the human text
/// out of the `--output-format json` envelope (degrading to raw prose otherwise,
/// exactly as the conversational/polish paths do), an envelope that reported an
/// error is dropped, and the lifted text is validated against the intent allowlist
/// by [`relux_kernel::parse_intent_proposal`]. The raw envelope never escapes this
/// function.
fn parse_cli_intent(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainIntentProposal> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_intent_proposal(&parsed.text).ok()
}

/// Extract a task's structured slots from one message via a local CLI brain, as
/// VALIDATED [`relux_kernel::BrainTaskSlots`], or `None`.
///
/// The slot counterpart of [`classify_intent_via_cli`]: the CLI is spawned in the
/// same bounded, non-bypass mode (argv-only, prompt on stdin, wall-clock timeout,
/// output cap, secret redaction), its stdout is lifted by
/// [`parse_adapter_result`] FIRST so the raw `--output-format json` envelope never
/// reaches the parser or the UI, and only the lifted text is handed to
/// [`relux_kernel::parse_task_slots`], which rejects any unsupported field and
/// clamps/sanitizes every value. It performs NO durable action: it only proposes
/// slots the kernel then reconciles against the live agent roster behind the
/// fail-closed gate. Any failure → `None` and the deterministic slots stand.
async fn extract_task_slots_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
) -> Option<relux_kernel::BrainTaskSlots> {
    let (stdout, kind) =
        cli_brain_json(brain, status, relux_kernel::build_task_slots_prompt(message)).await?;
    parse_cli_task_slots(&stdout, kind)
}

/// Extract assignment slots (`{task_id, agent_id}`) via a CLI brain, grounded in the
/// live board, validated through the SAME no-leak boundary as the task-slot path. Any
/// failure → `None` (the deterministic clarify stands). The kernel still validates both
/// ids against the live state before promoting any assignment.
async fn extract_assign_slots_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    summary: &relux_core::StateSummary,
) -> Option<relux_kernel::BrainAssignSlots> {
    let (stdout, kind) = cli_brain_json(
        brain,
        status,
        relux_kernel::build_assign_slots_prompt(message, summary),
    )
    .await?;
    parse_cli_assign_slots(&stdout, kind)
}

/// Extract by-id task UPDATE slots via a CLI brain, grounded in the live board,
/// validated through the SAME no-leak boundary as the task-slot path. Any failure →
/// `None` (the deterministic clarify stands). The kernel still validates the
/// task/field/status/assignee against the live state before applying anything.
async fn extract_update_slots_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    summary: &relux_core::StateSummary,
) -> Option<relux_kernel::BrainUpdateSlots> {
    let (stdout, kind) = cli_brain_json(
        brain,
        status,
        relux_kernel::build_update_slots_prompt(message, summary),
    )
    .await?;
    parse_cli_update_slots(&stdout, kind)
}

/// Extract agent-creation slots via a CLI brain, validated through the SAME boundary
/// as the task-slot path: bounded non-bypass spawn → [`parse_adapter_result`] →
/// [`relux_kernel::parse_agent_slots`]. Any failure → `None` (deterministic name).
async fn extract_agent_slots_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
) -> Option<relux_kernel::BrainAgentSlots> {
    let (stdout, kind) =
        cli_brain_json(brain, status, relux_kernel::build_agent_slots_prompt(message)).await?;
    parse_cli_agent_slots(&stdout, kind)
}

/// Extract a plugin reference via a CLI brain (advisory; the install stays
/// approval-gated). Same validation boundary; any failure → `None`.
async fn extract_plugin_ref_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
) -> Option<relux_kernel::BrainPluginRef> {
    let (stdout, kind) =
        cli_brain_json(brain, status, relux_kernel::build_plugin_ref_prompt(message)).await?;
    parse_cli_plugin_ref(&stdout, kind)
}

/// Extract a permission-grant subject via a CLI brain (advisory; the grant stays
/// approval-gated). Same validation boundary; any failure → `None`.
async fn extract_permission_slots_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
) -> Option<relux_kernel::BrainPermissionSlots> {
    let (stdout, kind) = cli_brain_json(
        brain,
        status,
        relux_kernel::build_permission_slots_prompt(message),
    )
    .await?;
    parse_cli_permission_slots(&stdout, kind)
}

/// Spawn a CLI brain in the same bounded, non-bypass mode every other Prime CLI path
/// uses (argv-only, prompt on stdin, wall-clock timeout, output cap, secret
/// redaction) and return its captured `stdout` plus the adapter kind, or `None` on a
/// missing/disabled adapter, spawn error, timeout, or empty/failed output. The shared
/// spawn for every brain-assisted slot extraction so each typed extractor is just
/// "build prompt → spawn → parse".
async fn cli_brain_json(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    prompt: String,
) -> Option<(String, relux_core::AdapterKind)> {
    let kind = match brain {
        relux_kernel::PrimeBrain::ClaudeCli => relux_core::AdapterKind::ClaudeCli,
        relux_kernel::PrimeBrain::CodexCli => relux_core::AdapterKind::CodexCli,
        _ => return None,
    };
    let st = status?;
    if st.state != relux_core::AdapterRuntimeState::Available {
        return None;
    }
    let program = st.resolved_path.clone()?;

    let spec = relux_kernel::AdapterCommandSpec {
        program,
        args: relux_kernel::build_adapter_args(&kind),
        stdin: prompt,
        working_dir: st.working_dir.clone(),
        timeout: std::time::Duration::from_secs(
            st.timeout_seconds
                .unwrap_or(relux_core::DEFAULT_ADAPTER_TIMEOUT_SECONDS),
        ),
        max_output_bytes: st
            .max_output_bytes
            .unwrap_or(relux_core::DEFAULT_ADAPTER_MAX_OUTPUT_BYTES) as usize,
    };

    let run = tokio::task::spawn_blocking(move || relux_kernel::run_adapter_command(&spec)).await;
    match run {
        Ok(Ok(outcome)) if outcome.success && !outcome.stdout.trim().is_empty() => {
            Some((outcome.stdout, kind))
        }
        _ => None,
    }
}

/// Lift validated task slots out of a CLI brain's captured `stdout`, or `None`.
/// The no-leak parse boundary, kept pure so it is pinned without spawning a
/// process: [`parse_adapter_result`] lifts the human text out of the result
/// envelope (degrading to raw prose otherwise, exactly as the intent/polish paths
/// do), an envelope that reported an error is dropped, and the lifted text is
/// validated by [`relux_kernel::parse_task_slots`]. The raw envelope never escapes.
fn parse_cli_task_slots(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainTaskSlots> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_task_slots(&parsed.text).ok()
}

/// Lift validated assignment slots out of a CLI brain's `stdout` (the same no-leak
/// boundary as [`parse_cli_task_slots`]). The raw envelope never escapes.
fn parse_cli_assign_slots(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainAssignSlots> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_assign_slots(&parsed.text).ok()
}

/// Lift validated by-id update slots out of a CLI brain's `stdout` (the same no-leak
/// boundary as [`parse_cli_task_slots`]). The raw envelope never escapes.
fn parse_cli_update_slots(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainUpdateSlots> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_update_slots(&parsed.text).ok()
}

/// Lift validated agent slots out of a CLI brain's `stdout` (the same no-leak boundary
/// as [`parse_cli_task_slots`]). The raw envelope never escapes.
fn parse_cli_agent_slots(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainAgentSlots> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_agent_slots(&parsed.text).ok()
}

/// Lift a validated plugin reference out of a CLI brain's `stdout` (advisory; the
/// install stays approval-gated). Same no-leak boundary.
fn parse_cli_plugin_ref(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainPluginRef> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_plugin_ref(&parsed.text).ok()
}

/// Lift a validated permission-grant subject out of a CLI brain's `stdout` (advisory;
/// the grant stays approval-gated). Same no-leak boundary.
fn parse_cli_permission_slots(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::BrainPermissionSlots> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::parse_permission_slots(&parsed.text).ok()
}

/// Produce ONE UNIFIED Prime decision (intent + every applicable slot + optional wording) via
/// a local CLI brain (Claude / Codex) in a single spawn, validated through the SAME no-leak
/// boundary as the specialized slot paths. Any failure → `None` (the caller falls back to the
/// specialized intent / slot / wording calls). The kernel still validates each section against
/// the live state behind the fail-closed gate.
async fn decide_prime_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    summary: &relux_core::StateSummary,
    history: &str,
    observations: &str,
    correction: &str,
) -> relux_kernel::DecisionOutcome {
    let Some((stdout, kind)) = cli_brain_json(
        brain,
        status,
        relux_kernel::build_decision_prompt_with_correction(
            message,
            summary,
            history,
            observations,
            correction,
        ),
    )
    .await
    else {
        // The spawn produced no usable stdout (disabled / not found / timeout / empty): not
        // correctable.
        return relux_kernel::DecisionOutcome::ProviderError;
    };
    classify_cli_decision(&stdout, kind)
}

/// Drive the bounded **observe-then-act** decision loop for one turn and return the terminal
/// decision plus every read-only context read gathered along the way.
///
/// This is the multi-round upgrade of the single unified decision call: each round the configured
/// brain may either request READ-ONLY context tools (observe) or commit its decision (act / answer).
/// The kernel executes ONLY the validated read-only requests deterministically against the pre-taken
/// `snapshot` between rounds (the brain runs nothing) and re-calls the decision brain grounded in the
/// results, bounded by [`relux_kernel::MAX_DECISION_ROUNDS`]. The eventual ACTION (the terminal
/// decision's `action_request` / classification / slots) still flows through the UNCHANGED fail-closed
/// gate and `decide` → `prime_execute` / approval at the kernel chokepoint — this loop adds NO new
/// authority and has no mutation path. All three brains share the SAME
/// [`relux_kernel::DecisionLoop`] stepper (the per-round "prompt → decision" primitive is the only
/// per-brain difference), so the loop's control flow — round cap, read-only execution,
/// stop-on-progress — is pinned once and never drifts. `Local` (no brain) makes no decision and
/// gathers nothing.
async fn decide_prime_with_observation(
    brain: relux_kernel::PrimeBrain,
    ai_config: &relux_kernel::AiConfig,
    cli_status: Option<relux_core::AdapterRuntimeStatus>,
    snapshot: &relux_kernel::ContextSnapshot,
    message: &str,
    summary: &relux_core::StateSummary,
    history: &str,
) -> (Option<relux_kernel::PrimeBrainDecision>, Vec<relux_kernel::ContextRead>) {
    if matches!(brain, relux_kernel::PrimeBrain::Local) {
        return (None, Vec::new());
    }
    let mut lp = relux_kernel::DecisionLoop::new(snapshot);
    let mut observations = String::new();
    let mut correction = String::new();
    loop {
        let outcome = match brain {
            relux_kernel::PrimeBrain::Openrouter => {
                relux_kernel::decide_prime_via_openrouter(
                    ai_config,
                    message,
                    summary,
                    history,
                    &observations,
                    &correction,
                )
                .await
            }
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                decide_prime_via_cli(
                    brain,
                    cli_status.clone(),
                    message,
                    summary,
                    history,
                    &observations,
                    &correction,
                )
                .await
            }
            // Unreachable (Local short-circuits above); treated as a non-correctable failure.
            relux_kernel::PrimeBrain::Local => relux_kernel::DecisionOutcome::ProviderError,
        };
        match lp.step_outcome(outcome) {
            // A new read-only observation: re-ask grounded in it; the reply parsed, so clear any
            // correction.
            relux_kernel::DecisionStep::Continue(obs) => {
                observations = obs;
                correction.clear();
            }
            // A malformed reply: re-ask ONCE with the validation error injected, keeping the
            // observations so the brain does not lose the live state it already inspected.
            relux_kernel::DecisionStep::Retry(err) => correction = err,
            relux_kernel::DecisionStep::Stop => break,
        }
    }
    lp.into_parts()
}

/// Lift a validated unified decision out of a CLI brain's captured `stdout`, or `None`. The
/// no-leak parse boundary, kept pure so it is pinned without spawning a process:
/// [`parse_adapter_result`] lifts the human text out of the `--output-format json` envelope
/// (degrading to raw prose otherwise, exactly as the other CLI seams do), an envelope that
/// reported an error is dropped, and the lifted text is validated by
/// [`relux_kernel::parse_decision`] (which itself rejects unknown top-level keys and validates
/// each section through its existing allowlist). The raw envelope never escapes this function.
#[cfg(test)]
fn parse_cli_decision(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> Option<relux_kernel::PrimeBrainDecision> {
    classify_cli_decision(stdout, kind).into_decision()
}

/// The correction-aware variant of [`parse_cli_decision`]: lift the CLI brain's `stdout` into a
/// [`relux_kernel::DecisionOutcome`] through the SAME no-leak boundary, distinguishing a
/// malformed-but-correctable reply from a provider failure so the bounded self-correction loop can
/// re-ask only the former.
///
/// - An error envelope (`is_error == true`, e.g. the CLI reported a rate limit) is a
///   `ProviderError` — re-asking will not change the format, and the failure is the provider's.
/// - A reply the [`parse_adapter_result`]-lifted text fails [`relux_kernel::parse_decision`] on is
///   `Malformed` (the brain answered but the envelope was unusable — re-askable), carrying the exact
///   validation error as the correction message.
/// - A valid envelope is a `Decision`. The raw provider envelope never escapes this function.
fn classify_cli_decision(
    stdout: &str,
    kind: relux_core::AdapterKind,
) -> relux_kernel::DecisionOutcome {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return relux_kernel::DecisionOutcome::ProviderError;
    }
    match relux_kernel::parse_decision(&parsed.text) {
        Ok(d) => relux_kernel::DecisionOutcome::Decision(d),
        Err(e) => relux_kernel::DecisionOutcome::Malformed(e),
    }
}

/// Run ONE round of the read-only context loop through a CLI brain (Claude / Codex), returning
/// the lifted raw text (a tool-call JSON or a `{"done":true}` / final answer), or `None` on ANY
/// failure. Same bounded, non-bypass spawn as every other CLI brain path; [`parse_adapter_result`]
/// lifts the human text out of the `--output-format json` envelope (an error envelope is dropped),
/// so the raw envelope never reaches [`relux_kernel::interpret_reply`].
async fn cli_brain_tool_round(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    prompt: String,
) -> Option<String> {
    let (stdout, kind) = cli_brain_json(brain, status, prompt).await?;
    lift_cli_tool_text(&stdout, kind)
}

/// Lift one read-only-loop round's raw text out of a CLI brain's captured `stdout`, or `None`.
/// The no-leak boundary, kept pure so it is pinned without spawning a process:
/// [`parse_adapter_result`] lifts the human text out of the `--output-format json` envelope
/// (degrading to raw prose otherwise), an envelope that reported an error is dropped, and an
/// empty result is dropped. The raw envelope never escapes — only the text the loop's
/// [`relux_kernel::interpret_reply`] then parses.
fn lift_cli_tool_text(stdout: &str, kind: relux_core::AdapterKind) -> Option<String> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    let text = parsed.text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Drive the bounded, governed READ-ONLY context loop for one turn and return the gathered reads.
///
/// This is the first safe Prime tool loop: a configured brain may request read-only context tools
/// (inspect the board, a task, the crew, a run); the kernel validates each requested name against
/// the read-only allowlist ([`relux_kernel::classify_tool`]), executes it deterministically against
/// the pre-taken `snapshot` (OUTSIDE the lock), injects the result, and lets the brain ask again or
/// stop — bounded by [`relux_kernel::MAX_TOOL_ROUNDS`]. The loop changes nothing and never reaches
/// an action; its reads only ground the conversational reply. `Local` (no brain) gathers nothing.
///
/// All three brains share the SAME [`relux_kernel::ContextLoop`] stepper (the per-round "prompt →
/// text" primitive is the only per-brain difference), so the loop's safety logic — allowlist
/// validation, self-correction, round cap, read-only execution — is pinned once and never drifts.
async fn gather_read_only_context(
    brain: relux_kernel::PrimeBrain,
    ai_config: &relux_kernel::AiConfig,
    cli_status: Option<relux_core::AdapterRuntimeStatus>,
    snapshot: &relux_kernel::ContextSnapshot,
    message: &str,
) -> Vec<relux_kernel::ContextRead> {
    let mut lp = relux_kernel::ContextLoop::new(message, snapshot);
    while let Some(prompt) = lp.next_prompt() {
        let raw = match brain {
            relux_kernel::PrimeBrain::Openrouter => {
                relux_kernel::complete_tool_round(ai_config, prompt).await
            }
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                cli_brain_tool_round(brain, cli_status.clone(), prompt).await
            }
            relux_kernel::PrimeBrain::Local => None,
        };
        let Some(raw) = raw else {
            break;
        };
        if !lp.observe(&raw) {
            break;
        }
    }
    lp.into_reads()
}

/// The provenance label for brain-extracted slots: the OpenRouter model id, or the
/// CLI brain's display label. Surfaced on the slot card the way the intent/polish
/// provenance is. `Local` never produces slots, so it degrades to a neutral label.
fn slot_source_label(brain: relux_kernel::PrimeBrain, cfg: &relux_kernel::AiConfig) -> String {
    match brain {
        relux_kernel::PrimeBrain::Openrouter => cfg.model.clone(),
        relux_kernel::PrimeBrain::ClaudeCli => "Claude CLI".to_string(),
        relux_kernel::PrimeBrain::CodexCli => "Codex CLI".to_string(),
        relux_kernel::PrimeBrain::Local => "AI brain".to_string(),
    }
}

/// Build the [`AiOutcome`] for a conversational reply the UNIFIED decision already produced,
/// so a plain chat turn needs no extra brain call. The reply was validated through the same
/// block-sanitize + action-claim chokepoint a brainstorm reply uses
/// ([`relux_kernel::PrimeBrainDecision::validated_reply`]); here we only stamp the mode + model
/// provenance exactly as the clarify-polish path does. `Local` never produces a unified
/// decision, so it never reaches here.
fn unified_reply_outcome(
    brain: relux_kernel::PrimeBrain,
    cfg: &relux_kernel::AiConfig,
    reply: String,
) -> AiOutcome {
    let mode = match brain {
        relux_kernel::PrimeBrain::Openrouter => AiMode::Openrouter,
        relux_kernel::PrimeBrain::ClaudeCli => AiMode::ClaudeCli,
        relux_kernel::PrimeBrain::CodexCli => AiMode::CodexCli,
        relux_kernel::PrimeBrain::Local => AiMode::Deterministic,
    };
    AiOutcome {
        mode,
        reply,
        model: Some(slot_source_label(brain, cfg)),
        note: None,
    }
}

/// Produce the conversational reply for a clarify / brainstorm turn through the
/// VALIDATED wording path, returning the final outcome plus the provenance to surface.
///
/// Unlike the free-form shaper, the brain here returns ONE schema-checked, length-bounded
/// question/summary; the kernel-side validators
/// ([`relux_kernel::parse_clarify`] → [`relux_kernel::reconcile_clarify`]) reject anything
/// malformed, a clarify that is not exactly one question, an action claim, low confidence,
/// or a pure echo. On ANY failure the grounded deterministic wording stands with no
/// provenance. This runs ONLY on a non-actionful clarify/brainstorm turn, so it can never
/// touch an action (the action-free wall, `docs/prime-processing-audit.md`).
async fn run_clarify_polish(
    brain: relux_kernel::PrimeBrain,
    cfg: &relux_kernel::AiConfig,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    turn: &PrimeTurn,
    kind: relux_kernel::ClarifyKind,
    precomputed: Option<String>,
) -> (AiOutcome, Option<ReplyPolishProvenance>) {
    let deterministic = turn.reply.clone();
    let polished = match precomputed {
        // The unified decision already produced validated wording for this turn — use it
        // directly, with no second brain call (the one-call-per-turn win).
        Some(text) => Some(text),
        None => match brain {
            relux_kernel::PrimeBrain::Openrouter => {
                relux_kernel::polish_clarify_via_openrouter(cfg, message, &deterministic, kind)
                    .await
            }
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli => {
                polish_clarify_via_cli(brain, status, message, &deterministic, kind).await
            }
            relux_kernel::PrimeBrain::Local => None,
        },
    };

    match polished {
        Some(text) => {
            let mode = match brain {
                relux_kernel::PrimeBrain::Openrouter => AiMode::Openrouter,
                relux_kernel::PrimeBrain::ClaudeCli => AiMode::ClaudeCli,
                relux_kernel::PrimeBrain::CodexCli => AiMode::CodexCli,
                relux_kernel::PrimeBrain::Local => AiMode::Deterministic,
            };
            let source = slot_source_label(brain, cfg);
            let outcome = AiOutcome {
                mode,
                reply: text,
                model: Some(source.clone()),
                note: None,
            };
            let provenance = ReplyPolishProvenance {
                kind: kind.label().to_string(),
                source,
            };
            (outcome, Some(provenance))
        }
        // No brain / unavailable / invalid: keep the grounded deterministic wording.
        None => (
            AiOutcome::deterministic_fallback(deterministic, None),
            None,
        ),
    }
}

/// Re-word a clarify / brainstorm turn via a local CLI brain (Claude / Codex), returning
/// the validated polished text or `None`. Same bounded, non-bypass spawn as every other
/// CLI brain path; the captured stdout is lifted out of the result envelope and run
/// through the SAME validators ([`parse_cli_clarify`]), so the raw envelope never escapes
/// and an error envelope / prose / a non-question / an action claim all yield `None`.
async fn polish_clarify_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    deterministic_text: &str,
    kind: relux_kernel::ClarifyKind,
) -> Option<String> {
    let (stdout, akind) = cli_brain_json(
        brain,
        status,
        relux_kernel::build_clarify_prompt(kind, message, deterministic_text),
    )
    .await?;
    parse_cli_clarify(&stdout, akind, kind, deterministic_text)
}

/// Lift validated clarify/brainstorm wording out of a CLI brain's captured `stdout`, or
/// `None`. The same no-leak boundary as [`parse_cli_task_slots`]: [`parse_adapter_result`]
/// lifts the human text out of the result envelope, an error envelope is dropped, and the
/// lifted text is validated + reconciled. The raw envelope never escapes.
fn parse_cli_clarify(
    stdout: &str,
    kind: relux_core::AdapterKind,
    clarify_kind: relux_kernel::ClarifyKind,
    deterministic_text: &str,
) -> Option<String> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    let proposal = relux_kernel::parse_clarify(&parsed.text, clarify_kind).ok()?;
    relux_kernel::reconcile_clarify(deterministic_text, &proposal, clarify_kind)
}

/// Shape the POST-EXECUTION (after-action) reply for an actionful turn via a local CLI brain
/// (Claude / Codex), returning the validated wording or `None`. Same bounded, non-bypass spawn
/// as every other CLI brain path; the captured stdout is lifted out of the result envelope and
/// run through the SAME validators ([`parse_cli_after_action`]), so the raw envelope never
/// escapes and an error envelope / prose / a contradiction / an invented id all yield `None`
/// (the grounded deterministic reply then stands). The action already ran; this only re-words it.
async fn polish_after_action_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    message: &str,
    envelope: &relux_kernel::ActionEnvelope,
) -> Option<String> {
    let (stdout, akind) = cli_brain_json(
        brain,
        status,
        relux_kernel::build_after_action_prompt(message, envelope),
    )
    .await?;
    parse_cli_after_action(&stdout, akind, envelope)
}

/// Lift validated after-action wording out of a CLI brain's captured `stdout`, or `None`. The
/// same no-leak boundary as [`parse_cli_clarify`]: [`parse_adapter_result`] lifts the human text
/// out of the result envelope, an error envelope is dropped, and the lifted text is validated +
/// reconciled against the sanitized [`relux_kernel::ActionEnvelope`]. The raw envelope never
/// escapes; a contradiction / invented id / low confidence yields `None`.
fn parse_cli_after_action(
    stdout: &str,
    kind: relux_core::AdapterKind,
    envelope: &relux_kernel::ActionEnvelope,
) -> Option<String> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    if parsed.is_error == Some(true) {
        return None;
    }
    let proposal = relux_kernel::parse_after_action(&parsed.text, envelope).ok()?;
    relux_kernel::reconcile_after_action(&envelope.grounded_reply, &proposal)
}

/// Validate a CLI brain's captured `stdout` into an advisory proposal polish, or
/// `None`. The shape seam mirrors [`shape_cli_brain_reply`]: [`parse_adapter_result`]
/// lifts the human text out of a JSON result envelope (degrading to raw prose
/// otherwise, exactly as the conversational path does), and the lifted text is then
/// run through the SAME validation chokepoint ([`relux_kernel::polish_from_cli_text`]
/// -> `validate_polish`) as every other polish. An error envelope, prose with no
/// JSON object, or a suggestion that fails validation (changed count/order/agents)
/// all yield `None` and the deterministic preview stands. Pure: the no-action /
/// validation contract is pinned by unit tests without spawning a process.
fn shape_cli_brain_polish(
    stdout: &str,
    kind: relux_core::AdapterKind,
    label: &str,
    proposal: &relux_core::PrimeProposal,
) -> Option<relux_core::PrimeProposalPolish> {
    let parsed = relux_core::parse_adapter_result(stdout, kind);
    // An envelope can report an error even on a clean exit; a polish is purely
    // advisory, so just keep the deterministic preview rather than surfacing it.
    if parsed.is_error == Some(true) {
        return None;
    }
    relux_kernel::polish_from_cli_text(proposal, &parsed.text, label)
}

/// Attempt an advisory presentation polish of a plan preview via a local CLI brain
/// (Claude / Codex), mirroring [`run_cli_brain`] but producing a validated overlay
/// instead of a conversational reply.
///
/// Safety contract (`docs/RELUX_MASTER_PLAN.md` section 8.1, section 17.1): identical
/// to the OpenRouter polish path ([`relux_kernel::polish_proposal`]). The CLI may
/// refine ONLY wording; its reply is run through the SAME validation chokepoint
/// (count/order/agent ids are immutable — only titles/questions/risks/provenance can
/// change). It spawns the adapter in the same bounded, non-bypass mode the
/// assigned-run path uses (argv-only, prompt on stdin, wall-clock timeout, output
/// cap, secret redaction) and performs NO durable action: nothing in the commit path
/// ever reads the overlay. On ANY failure — adapter missing/disabled/off-PATH, spawn
/// error, timeout, error envelope, prose, or a suggestion that fails validation — it
/// returns `None` and the deterministic preview stands, with no user-facing error
/// (the card is simply not polished).
async fn polish_proposal_via_cli(
    brain: relux_kernel::PrimeBrain,
    status: Option<relux_core::AdapterRuntimeStatus>,
    proposal: &relux_core::PrimeProposal,
) -> Option<relux_core::PrimeProposalPolish> {
    // Only a genuine multi-step plan carries anything to refine (single-step /
    // empty proposals skip for every brain, exactly like the OpenRouter path).
    if !relux_kernel::proposal_wants_polish(proposal) {
        return None;
    }
    let (label, kind) = match brain {
        relux_kernel::PrimeBrain::ClaudeCli => ("Claude CLI", relux_core::AdapterKind::ClaudeCli),
        relux_kernel::PrimeBrain::CodexCli => ("Codex CLI", relux_core::AdapterKind::CodexCli),
        // Not a CLI brain — caller never routes these here.
        _ => return None,
    };

    // The adapter must be installed, enabled, and resolved on PATH. Any gap simply
    // leaves the preview unpolished (a polish is advisory; no note is warranted).
    let st = status?;
    if st.state != relux_core::AdapterRuntimeState::Available {
        return None;
    }
    let program = st.resolved_path.clone()?;

    let prompt = relux_kernel::compose_polish_prompt(proposal);
    let spec = relux_kernel::AdapterCommandSpec {
        program,
        args: relux_kernel::build_adapter_args(&kind),
        stdin: prompt,
        working_dir: st.working_dir.clone(),
        timeout: std::time::Duration::from_secs(
            st.timeout_seconds
                .unwrap_or(relux_core::DEFAULT_ADAPTER_TIMEOUT_SECONDS),
        ),
        max_output_bytes: st
            .max_output_bytes
            .unwrap_or(relux_core::DEFAULT_ADAPTER_MAX_OUTPUT_BYTES) as usize,
    };

    // The spawn is blocking (poll loop); keep it off the async reactor.
    let run = tokio::task::spawn_blocking(move || relux_kernel::run_adapter_command(&spec)).await;
    match run {
        Ok(Ok(outcome)) if outcome.success && !outcome.stdout.trim().is_empty() => {
            shape_cli_brain_polish(&outcome.stdout, kind, label, proposal)
        }
        // Timeout / non-zero exit / spawn error / interruption: silently keep the
        // deterministic preview. The conversational reply already carried any
        // actionable note about the adapter on this same turn.
        _ => None,
    }
}

#[derive(Debug, Serialize)]
struct PrimeAutonomyResponse {
    config: PrimeAutonomyConfig,
    last_tick_result: Option<PrimeAutonomyTickResult>,
}

async fn get_autonomy_config(
    State(state): State<AppState>,
) -> Result<Json<PrimeAutonomyResponse>, ApiError> {
    let config = locked_read(&state, |kernel| Ok(kernel.prime_autonomy_config.clone()))?;
    // The last_tick_summary and last_tick_at are already part of PrimeAutonomyConfig,
    // so we can reconstruct PrimeAutonomyTickResult from them if available.
    let last_tick_result = config.last_tick_at.clone().map(|tick_at| {
        PrimeAutonomyTickResult {
            tick_at,
            summary: config.last_tick_summary.clone().unwrap_or_default(),
            ..Default::default() // Fill other fields with default as they are not stored in config
        }
    });

    Ok(Json(PrimeAutonomyResponse {
        config,
        last_tick_result,
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateAutonomyConfigReq {
    enabled: Option<bool>,
    interval_seconds: Option<u64>,
    max_tasks_per_tick: Option<u32>,
    auto_assign_unassigned: Option<bool>,
}

async fn update_autonomy_config(
    State(state): State<AppState>,
    Json(req): Json<UpdateAutonomyConfigReq>,
) -> Result<Json<PrimeAutonomyConfig>, ApiError> {
    let updated_config = locked_save(&state, |kernel| {
        let mut config = kernel.prime_autonomy_config.clone();
        if let Some(enabled) = req.enabled {
            config.enabled = enabled;
        }
        if let Some(interval_seconds) = req.interval_seconds {
            config.interval_seconds = interval_seconds.clamp(5, 3600);
        }
        if let Some(max_tasks_per_tick) = req.max_tasks_per_tick {
            config.max_tasks_per_tick = max_tasks_per_tick.clamp(1, 25);
        }
        if let Some(auto_assign_unassigned) = req.auto_assign_unassigned {
            config.auto_assign_unassigned = auto_assign_unassigned;
        }
        kernel.prime_autonomy_config = config.clone();
        Ok(config)
    })?;
    Ok(Json(updated_config))
}

async fn run_autonomy_tick(
    State(state): State<AppState>,
) -> Result<Json<PrimeAutonomyTickResult>, ApiError> {
    let result = locked_save(&state, |kernel| {
        let tick_result = kernel.one_autonomy_tick();
        Ok(tick_result)
    })?;
    Ok(Json(result))
}

// --- Orchestration (multi-agent autonomy) ----------------------------------

#[derive(Debug, Deserialize)]
struct OrchestrateReq {
    goal: String,
}

/// Preview a multi-agent plan for a goal WITHOUT committing anything. Read-only:
/// lets the dashboard show "N briefs across M agents" before the user creates it.
async fn preview_orchestration(
    State(state): State<AppState>,
    Json(req): Json<OrchestrateReq>,
) -> Result<Json<OrchestrationPlan>, ApiError> {
    let goal = req.goal.trim().to_string();
    if goal.is_empty() {
        return Err(ApiError::bad_request("goal is required"));
    }
    let plan = locked_read(&state, |kernel| {
        Ok(relux_core::plan_orchestration(&goal, &kernel.inspect_state()))
    })?;
    Ok(Json(plan))
}

/// Create (plan + assign) an orchestration from a goal. Creates briefs assigned to
/// agents but does not run them; running is a separate governed batch.
async fn create_orchestration(
    State(state): State<AppState>,
    Json(req): Json<OrchestrateReq>,
) -> Result<Json<Orchestration>, ApiError> {
    let goal = req.goal.trim().to_string();
    if goal.is_empty() {
        return Err(ApiError::bad_request("goal is required"));
    }
    let record = locked_save(&state, |kernel| {
        let ctx = crate::ensure_bootstrapped(kernel)?;
        kernel.prime_orchestrate(&ctx, &goal)
    })?;
    Ok(Json(record))
}

async fn list_orchestrations(
    State(state): State<AppState>,
) -> Result<Json<Vec<Orchestration>>, ApiError> {
    let list = locked_read(&state, |kernel| {
        Ok(kernel.orchestrations().into_iter().cloned().collect::<Vec<_>>())
    })?;
    Ok(Json(list))
}

async fn get_orchestration(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Orchestration>, ApiError> {
    let oid = OrchestrationId::new(id.clone());
    let rec = locked_read(&state, |kernel| {
        kernel
            .orchestration(&oid)
            .cloned()
            .ok_or(KernelError::UnknownOrchestration(id.clone()))
    })?;
    Ok(Json(rec))
}

#[derive(Debug, Default, Deserialize)]
struct RunOrchestrationReq {
    /// Max briefs to run this batch (clamped to 1..=25 by the kernel). Defaults to
    /// 25 (the whole plan) when omitted.
    #[serde(default)]
    max: Option<usize>,
    /// Round-size cap: the most ready briefs the scheduler runs together in one
    /// round (clamped to 1..=4 by the kernel). Defaults to 2 when omitted.
    #[serde(default)]
    concurrency: Option<usize>,
}

/// Run a governed multi-agent batch for one orchestration **synchronously**: this
/// blocks until every round is done, then returns the final per-agent batch result.
/// (Callers that want to poll mid-run use the non-blocking `run-async` endpoint,
/// which returns a job id immediately.)
///
/// The work runs through the shared [`KernelState::run_orchestration`] engine, so
/// the independent briefs ready in one round execute as real concurrent OS adapter
/// processes (bounded by `concurrency`, default 2, clamp 1..=4) — the same true
/// parallelism the job path has. It is driven on a blocking worker so the async
/// reactor is never parked for the (possibly multi-second) batch, and the kernel
/// lock is held for the whole batch so two concurrent runs of the same orchestration
/// can never double-execute a brief. Persists even on partial failure so
/// blocked/failed step records survive (like the run/retry path).
async fn run_orchestration_batch(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<RunOrchestrationReq>>,
) -> Result<Json<OrchestrationBatchResult>, ApiError> {
    let oid = OrchestrationId::new(id);
    let req = body.map(|b| b.0).unwrap_or_default();
    let max = req.max.unwrap_or(25);
    let concurrency = req.concurrency.unwrap_or(2);
    // The whole batch (lock + OS-parallel adapter spawns + joins) is blocking, so run
    // it off the reactor on the blocking pool rather than parking an async worker.
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        locked_save_persisting(&state, |kernel| kernel.run_orchestration(&oid, max, concurrency))
    })
    .await
    .map_err(|e| ApiError::internal(format!("orchestration run was interrupted: {e}")))??;
    Ok(Json(result))
}

// --- Non-blocking orchestration jobs ---------------------------------------
//
// The synchronous `/run` above holds the single-owner kernel lock for the whole
// batch and only returns once every round is done, so the dashboard can show
// progress only AFTER the call returns. These job endpoints make a run
// non-blocking: `run-async` starts a background worker and returns immediately
// with a job id + status URL; the worker drives the SAME governed, tested
// `run_orchestration` one round at a time (a per-call budget equal to the round
// size), releasing the lock and persisting the orchestration record between
// rounds. Polling the job (or the durable record) therefore sees real,
// already-recorded progress mid-batch — nothing is fabricated.
//
// Honesty contract on restart: the job registry is in-memory only. If the server
// restarts mid-job the job record is lost and polling returns 404; the durable
// orchestration record still carries whatever rounds actually completed (it never
// claims completion the kernel did not record), and the dashboard falls back to
// the record. The worker never loops forever: each underlying round moves ≥1
// brief to a terminal outcome, and the worker stops as soon as a round runs no
// brief or the orchestration is no longer `running`.

/// The lifecycle state of a background orchestration job. Distinct from the
/// orchestration's own status: a job is `completed` once the worker finished its
/// rounds, even if the orchestration itself ended `needs_attention`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobState {
    /// Created, worker not yet started.
    Queued,
    /// The worker is driving rounds.
    Running,
    /// The worker finished all rounds it could run (see the embedded `result`/
    /// orchestration status for the real outcome).
    Completed,
    /// The worker hit an error it could not turn into a per-brief block (e.g. the
    /// store failed); the message is in `error`.
    Failed,
    /// Cancellation was requested and honored: the worker finished any round that
    /// was already in flight, then stopped before the next one. Remaining briefs
    /// are left in their durable (pending) state for a human to resume or retire.
    Canceled,
    /// Reconstructed from the durable record because no in-process job exists: the
    /// job registry is in-memory, so a server restart loses live jobs. A prior
    /// worker ran at least one brief but is gone, and briefs remain pending. This
    /// is terminal for *this* job (the worker is not coming back); the pending
    /// briefs can be resumed with a fresh run. Never minted by a live worker —
    /// only by [`reconstruct_job_from_record`] when the durable record proves a run
    /// happened but no worker is driving it now.
    Interrupted,
}

impl JobState {
    /// A short human label, matching the snake_case wire form.
    fn label(&self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Completed => "completed",
            JobState::Failed => "failed",
            JobState::Canceled => "canceled",
            JobState::Interrupted => "interrupted",
        }
    }
}

/// One brief's status as the job last observed it. `outcome` is the durable step
/// outcome label (`pending`/`completed`/`failed`/`blocked`), except that briefs
/// the worker is about to run this round are reported as `running` so a mid-batch
/// poll shows real in-flight work.
#[derive(Debug, Clone, Serialize)]
struct JobStepStatus {
    task_id: String,
    agent_id: String,
    title: String,
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// A pollable, in-memory record of one non-blocking orchestration run.
#[derive(Debug, Clone, Serialize)]
struct OrchestrationJob {
    id: String,
    orchestration_id: String,
    state: JobState,
    /// The per-call total cap (briefs) and round size the worker uses.
    max: usize,
    concurrency: usize,
    /// Cumulative rounds the worker has completed so far.
    current_round: u32,
    /// Cumulative per-outcome tallies across the job's rounds.
    ran: u32,
    completed: u32,
    failed: u32,
    blocked: u32,
    /// Wall-clock start/finish (unix millis). Real time, since a job is a runtime
    /// artifact (not part of the deterministic kernel state).
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<u64>,
    /// The most recent human-readable event (e.g. a round summary), for the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_event: Option<String>,
    /// An honest error message when `state == Failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Set true the moment a cancel is requested. While the job is still `running`
    /// this means "canceling - finishing the in-flight round, then stopping"; the
    /// worker flips the state to `Canceled` once that round completes. Always
    /// serialized so the dashboard can show the pending-cancel state honestly.
    cancel_requested: bool,
    /// The latest per-brief snapshot the worker recorded.
    steps: Vec<JobStepStatus>,
    /// The aggregate batch result, set once the worker finishes.
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<OrchestrationBatchResult>,
}

/// Why a job could not be started.
#[derive(Debug)]
enum JobStartError {
    /// A job for this orchestration is already queued/running.
    Duplicate(String),
    /// Too many jobs are active across the fleet right now.
    TooManyActive(usize),
}

/// The most non-terminal jobs allowed at once across all orchestrations, so a
/// burst of requests can never spawn unbounded worker threads.
const MAX_ACTIVE_JOBS: usize = 4;

/// A process-wide registry of orchestration jobs, guarded by its own short-lived
/// mutex. Crucially this lock is NEVER held across kernel work, so polling a job
/// stays responsive even while a worker holds the kernel lock for a round.
#[derive(Clone, Default)]
struct JobRegistry {
    inner: Arc<Mutex<JobStore>>,
}

#[derive(Default)]
struct JobStore {
    jobs: HashMap<String, OrchestrationJob>,
    counter: u64,
}

impl JobRegistry {
    /// Atomically mint a new job for `orchestration_id`, rejecting a duplicate
    /// (one already active for the same orchestration) or an over-cap fleet. The
    /// returned job is `Queued`; the caller spawns the worker.
    fn start(
        &self,
        orchestration_id: &str,
        max: usize,
        concurrency: usize,
    ) -> Result<OrchestrationJob, JobStartError> {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let active = store
            .jobs
            .values()
            .filter(|j| matches!(j.state, JobState::Queued | JobState::Running))
            .count();
        if let Some(existing) = store.jobs.values().find(|j| {
            j.orchestration_id == orchestration_id
                && matches!(j.state, JobState::Queued | JobState::Running)
        }) {
            return Err(JobStartError::Duplicate(existing.id.clone()));
        }
        if active >= MAX_ACTIVE_JOBS {
            return Err(JobStartError::TooManyActive(active));
        }
        store.counter += 1;
        let id = format!("job_{:04}", store.counter);
        let job = OrchestrationJob {
            id: id.clone(),
            orchestration_id: orchestration_id.to_string(),
            state: JobState::Queued,
            max,
            concurrency,
            current_round: 0,
            ran: 0,
            completed: 0,
            failed: 0,
            blocked: 0,
            started_at_ms: None,
            completed_at_ms: None,
            last_event: Some("queued".to_string()),
            error: None,
            cancel_requested: false,
            steps: Vec::new(),
            result: None,
        };
        store.jobs.insert(id, job.clone());
        Ok(job)
    }

    /// Mutate a job in place (no-op if it was evicted). The closure runs under the
    /// registry lock only — never call back into the kernel from it.
    fn update(&self, job_id: &str, f: impl FnOnce(&mut OrchestrationJob)) {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = store.jobs.get_mut(job_id) {
            f(job);
        }
    }

    fn get(&self, job_id: &str) -> Option<OrchestrationJob> {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.jobs.get(job_id).cloned()
    }

    /// The newest job for an orchestration (ids are zero-padded, so the lexically
    /// greatest id is the newest), or `None` when none has ever been started.
    fn latest_for(&self, orchestration_id: &str) -> Option<OrchestrationJob> {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store
            .jobs
            .values()
            .filter(|j| j.orchestration_id == orchestration_id)
            .max_by(|a, b| a.id.cmp(&b.id))
            .cloned()
    }

    /// Request cancellation of a job. This only sets a cooperative flag; the worker
    /// owns the actual `Canceled` state transition (it stops before its next round,
    /// after any in-flight round finishes), so the cancel path never races the
    /// worker on the state field and never kills a brief mid-flight. Idempotent on
    /// an already-canceling/canceled job; refuses a job that already finished.
    fn request_cancel(&self, job_id: &str) -> CancelOutcome {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match store.jobs.get_mut(job_id) {
            None => CancelOutcome::Unknown,
            Some(job) => match job.state {
                JobState::Queued | JobState::Running => {
                    job.cancel_requested = true;
                    job.last_event = Some(
                        "cancel requested - will stop after the in-flight round".to_string(),
                    );
                    CancelOutcome::Requested(job.clone())
                }
                JobState::Canceled => CancelOutcome::AlreadyCanceled(job.clone()),
                // `Interrupted` is never stored in the registry (it is reconstructed
                // on read), so this arm is unreachable in practice; treat it as
                // terminal for exhaustiveness and honesty.
                JobState::Completed | JobState::Failed | JobState::Interrupted => {
                    CancelOutcome::AlreadyTerminal(job.clone())
                }
            },
        }
    }

    /// Whether a cancel has been requested for `job_id` (false if it was evicted).
    /// The worker polls this between rounds to decide whether to stop.
    fn is_cancel_requested(&self, job_id: &str) -> bool {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.jobs.get(job_id).map(|j| j.cancel_requested).unwrap_or(false)
    }
}

/// The outcome of a cancellation request, so the HTTP handler can map it to an
/// honest status code without re-locking the registry.
#[derive(Debug)]
enum CancelOutcome {
    /// The job was active; the cancel flag is set (worker will stop next round).
    Requested(OrchestrationJob),
    /// The job was already canceling/canceled; nothing more to do (idempotent).
    AlreadyCanceled(OrchestrationJob),
    /// The job already finished (completed/failed); there is nothing to cancel.
    AlreadyTerminal(OrchestrationJob),
    /// No such job (never started, or lost to a restart).
    Unknown,
}

/// Wall-clock now in unix millis (0 if the clock is before the epoch, which never
/// happens in practice). Jobs use real time because they are runtime artifacts,
/// not part of the deterministic, reproducible kernel state.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Current wall-clock in unix SECONDS. Used by `/v1/auth/me` to turn a session's
/// absolute deadlines into "remaining seconds" so the Account control can render
/// an idle/re-auth readout without trusting the browser clock.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The task ids of up to `n` briefs that are ready to run right now (pending with
/// every dependency completed), in index order. Mirrors the kernel scheduler's
/// readiness rule (now owned by [`KernelState::prepare_orchestration_round`]); kept
/// as a test oracle that pins the same readiness semantics on the durable record.
#[cfg(test)]
fn ready_task_ids(orch: &Orchestration, n: usize) -> Vec<String> {
    orch.steps
        .iter()
        .filter(|s| {
            s.outcome == StepOutcome::Pending
                && s.depends_on.iter().all(|&j| {
                    orch.steps
                        .get(j)
                        .map(|d| d.outcome == StepOutcome::Completed)
                        .unwrap_or(true)
                })
        })
        .take(n)
        .map(|s| s.task_id.to_string())
        .collect()
}

/// Snapshot the orchestration's briefs for the job view. Any brief whose task id
/// is in `running` and is still `pending` is reported as `running` (it is about to
/// execute this round); everything else carries its real recorded outcome.
fn job_steps(orch: &Orchestration, running: &[String]) -> Vec<JobStepStatus> {
    orch.steps
        .iter()
        .map(|s| {
            let id = s.task_id.to_string();
            let outcome = if s.outcome == StepOutcome::Pending && running.contains(&id) {
                "running".to_string()
            } else {
                s.outcome.label().to_string()
            };
            JobStepStatus {
                task_id: id,
                agent_id: s.agent_id.to_string(),
                title: s.title.clone(),
                outcome,
                round: s.round,
                note: s.note.clone(),
            }
        })
        .collect()
}

/// Reconstruct a job-like status from the durable orchestration record alone, for
/// when no in-process job exists for the orchestration. The [`JobRegistry`] is
/// in-memory, so a server restart loses every live job; without this a poll by
/// orchestration id would 404 even though the durable record still proves real
/// progress. Reconstruction never fabricates anything — every field is derived
/// from what the kernel already persisted (per-brief outcomes, run ids, rounds).
///
/// Returns `None` when the orchestration never ran a single brief (no run id on
/// any step): no job has ever existed for it, so the honest answer is still "no
/// job started" and the dashboard falls back to the planned record. Otherwise the
/// state is honest about what the record proves: `completed` when every brief is
/// terminal (no pending left), else `interrupted` — a prior worker ran but is gone
/// (finished, canceled, or lost to a restart) and the pending briefs can be
/// resumed with a fresh run.
fn reconstruct_job_from_record(orch: &Orchestration) -> Option<OrchestrationJob> {
    let ran = orch.steps.iter().filter(|s| s.run_id.is_some()).count() as u32;
    if ran == 0 {
        // Nothing ever ran: no job has existed, so do not invent one.
        return None;
    }
    let completed = orch
        .steps
        .iter()
        .filter(|s| s.outcome == StepOutcome::Completed)
        .count() as u32;
    let failed = orch
        .steps
        .iter()
        .filter(|s| s.outcome == StepOutcome::Failed)
        .count() as u32;
    let blocked = orch
        .steps
        .iter()
        .filter(|s| s.outcome == StepOutcome::Blocked)
        .count() as u32;
    let pending = orch
        .steps
        .iter()
        .filter(|s| s.outcome == StepOutcome::Pending)
        .count();
    let current_round = orch.steps.iter().filter_map(|s| s.round).max().unwrap_or(0);
    let state = if pending == 0 {
        JobState::Completed
    } else {
        JobState::Interrupted
    };
    let last_event = Some(match state {
        JobState::Completed => format!(
            "reconstructed from the durable record: all {} brief(s) reached a terminal outcome \
             ({completed} completed, {failed} failed, {blocked} blocked)",
            orch.steps.len()
        ),
        _ => format!(
            "no live worker for this orchestration (the previous run finished, was canceled, or \
             was lost to a server restart). {completed} brief(s) completed durably; {pending} \
             pending — start a new run to resume"
        ),
    });
    Some(OrchestrationJob {
        // A clearly-synthetic, non-process-local id so a client can tell this came
        // from the durable record rather than a live worker.
        id: format!("durable:{}", orch.id.as_str()),
        orchestration_id: orch.id.to_string(),
        state,
        // Runtime params (`max`/`concurrency`/wall-clock) are not part of the
        // durable record; report honest best-effort values rather than fake ones.
        max: orch.steps.len(),
        concurrency: 1,
        current_round,
        ran,
        completed,
        failed,
        blocked,
        started_at_ms: None,
        completed_at_ms: None,
        last_event,
        error: None,
        cancel_requested: false,
        steps: job_steps(orch, &[]),
        result: None,
    })
}

/// Accumulates per-round [`OrchestrationBatchResult`]s into one job-level result.
/// Counts that grow round-over-round are summed; the current truth (status,
/// pending, waiting, next action) is taken from the most recent round.
#[derive(Default)]
struct JobAggregate {
    ran: u32,
    completed: u32,
    failed: u32,
    blocked: u32,
    dependency_blocked: u32,
    rounds: u32,
    per_agent: Vec<String>,
    skipped_reasons: Vec<String>,
    last: Option<OrchestrationBatchResult>,
}

impl JobAggregate {
    fn merge(&mut self, r: &OrchestrationBatchResult) {
        self.ran += r.ran;
        self.completed += r.completed;
        self.failed += r.failed;
        self.blocked += r.blocked;
        self.dependency_blocked += r.dependency_blocked;
        self.rounds += r.rounds;
        self.per_agent.extend(r.per_agent.iter().cloned());
        self.skipped_reasons.extend(r.skipped_reasons.iter().cloned());
        self.last = Some(r.clone());
    }

    /// Build the job-level aggregate result. Falls back to an empty completed-style
    /// result if no round ever ran (e.g. there was nothing pending).
    fn into_result(self, oid: &OrchestrationId, concurrency: usize) -> OrchestrationBatchResult {
        let last = self.last.clone();
        let (pending, waiting, status, next_action) = match &last {
            Some(r) => (r.pending, r.waiting, r.status, r.next_action.clone()),
            None => (
                0,
                0,
                OrchestrationStatus::Completed,
                "No pending briefs to run.".to_string(),
            ),
        };
        let summary = format!(
            "{} round(s) across the job, up to {} brief(s) at a time: {} ran ({} completed, {} failed, {} blocked); {} blocked by a failed dependency; {} waiting on a dependency; {} pending.",
            self.rounds,
            concurrency,
            self.ran,
            self.completed,
            self.failed,
            self.blocked,
            self.dependency_blocked,
            waiting,
            pending,
        );
        OrchestrationBatchResult {
            orchestration_id: oid.clone(),
            ran: self.ran,
            completed: self.completed,
            failed: self.failed,
            blocked: self.blocked,
            pending,
            concurrency: concurrency as u32,
            rounds: self.rounds,
            waiting,
            dependency_blocked: self.dependency_blocked,
            skipped_reasons: self.skipped_reasons,
            per_agent: self.per_agent,
            summary,
            next_action,
            status,
        }
    }
}

/// Run ONE dependency-aware round with true bounded OS-parallel adapter execution.
///
/// Three phases around the single-owner kernel lock:
///
/// 1. **Prepare (locked, persists).** [`KernelState::prepare_orchestration_round`]
///    marks dependency blocks, picks the ready set, starts each brief's run, and
///    resolves local-echo / pre-spawn-blocked briefs inline. Enabled CLI briefs come
///    back as [`relux_kernel::PreparedBrief`]s with their step already stamped (run
///    id, start, round), so a poll right now sees them as in-flight.
/// 2. **Spawn (NO lock).** [`run_briefs_in_parallel`] runs every prepared brief's
///    adapter process on its own OS thread concurrently — the real parallelism. The
///    lock is free, so polls and other requests stay responsive while the CLIs run.
/// 3. **Finalize (locked, persists).** Each finished brief is merged back via
///    [`KernelState::finalize_prepared_brief`]; the batch is then finalized.
///
/// A failure (or even a panic) in one brief's thread never corrupts another: each
/// brief owns its own run/task records and is merged independently. Returns the
/// per-round batch result (`rounds == 1`) plus the post-round record snapshot.
fn run_parallel_round(
    state: &AppState,
    oid: &OrchestrationId,
    job_id: &str,
    budget: usize,
    concurrency: usize,
    round_no: u32,
) -> Result<(OrchestrationBatchResult, Option<Orchestration>), ApiError> {
    // Phase 1: prepare under the lock. Persists the started runs + stamped steps so
    // a mid-round poll of the durable record sees real in-flight work.
    let (mut result, prepared) = locked_save_persisting(state, |kernel| {
        let mut result = kernel.new_orchestration_batch_result(oid, concurrency)?;
        let prep =
            kernel.prepare_orchestration_round(oid, budget, concurrency, round_no, &mut result)?;
        Ok((result, prep.prepared))
    })?;

    // Surface the genuinely-in-flight briefs to the job poll (real concurrency now,
    // not a pseudo-label): they have a Running run and an OS process about to spawn.
    if !prepared.is_empty() {
        let inflight: Vec<String> = prepared.iter().map(|p| p.task_id().to_string()).collect();
        let snap = locked_read(state, |k| Ok(k.orchestration(oid).cloned()))
            .ok()
            .flatten();
        state.jobs.update(job_id, |j| {
            if let Some(o) = snap.as_ref() {
                j.steps = job_steps(o, &inflight);
            }
            j.last_event = Some(format!(
                "round {round_no}: {} brief(s) running in parallel (cap {concurrency})",
                inflight.len()
            ));
        });
    }

    // The run ids about to stream — captured before `prepared` is consumed so we can
    // drop their live buffers once their canonical logs are finalized below.
    let streamed_run_ids: Vec<relux_core::RunId> =
        prepared.iter().map(|p| p.run_id().clone()).collect();

    // Phase 2: run the prepared adapter processes in parallel with the lock RELEASED,
    // through the streaming spawn primitive — each brief's stdout/stderr lines are
    // appended to the shared live run-log registry as they are read, so a poll of
    // `GET /v1/relux/runs/:id/logs` sees the tail WHILE the briefs run (the lock is
    // free during this window). The captured outcomes are identical to the
    // non-streaming driver.
    let finished = relux_kernel::run_briefs_in_parallel_streaming(
        prepared,
        &state.live_run_logs,
        &state.run_cancellations,
    );

    // Phase 3: merge every finished brief back under the lock, then finalize.
    let snap = locked_save_persisting(state, move |kernel| {
        for f in finished {
            kernel.finalize_prepared_brief(oid, f, &mut result);
        }
        result.rounds = 1;
        kernel.finalize_orchestration_batch(oid, &mut result)?;
        let snap = kernel.orchestration(oid).cloned();
        Ok((result, snap))
    })?;

    // The canonical, persisted run logs now exist (finalize captured them), so drop
    // the in-memory live buffers AND the cancel tokens — subsequent polls serve the
    // durable log, a later cancel honestly reports not-running, and both registries
    // stay bounded.
    for rid in &streamed_run_ids {
        state.live_run_logs.finish(rid);
        state.run_cancellations.finish(rid);
    }
    Ok(snap)
}

/// Drive one orchestration job to completion on a background thread.
///
/// Each iteration runs ONE governed round with true bounded OS-parallel adapter
/// execution ([`run_parallel_round`]), releasing the kernel lock during the spawn
/// window and persisting the record between rounds so progress is recorded
/// incrementally. It stops when the per-job `max` budget is spent, a round runs no
/// brief, or the orchestration is no longer `running`.
fn drive_orchestration_job(
    state: AppState,
    job_id: String,
    oid: OrchestrationId,
    user_max: usize,
    concurrency: usize,
) {
    let jobs = state.jobs.clone();
    jobs.update(&job_id, |j| {
        j.state = JobState::Running;
        j.started_at_ms.get_or_insert(now_millis());
        j.last_event = Some("running".to_string());
    });

    // The per-round budget: at most `concurrency` briefs per round, run as real
    // OS-parallel adapter processes (see [`run_parallel_round`]). The kernel lock is
    // released for the whole spawn/await window, so several briefs run at once and a
    // mid-round poll sees them all in flight.
    let per_call = concurrency.clamp(1, 4);
    let mut total_ran = 0usize;
    let mut agg = JobAggregate::default();
    let mut round_no = 0u32;
    // True once we stop *because* a cancel was requested (as opposed to finishing
    // the plan or running out of ready briefs). Drives the final Canceled state.
    let mut canceled = false;

    loop {
        // Cooperative cancellation checkpoint. The kernel lock is free here and any
        // prior round has fully finalized, so stopping now leaves remaining briefs
        // in their honest durable state - we never kill a brief mid-flight. A cancel
        // that arrives during a round is honored on the next loop iteration, after
        // that round's in-flight briefs finish and persist.
        if jobs.is_cancel_requested(&job_id) {
            canceled = true;
            break;
        }
        if total_ran >= user_max {
            break;
        }

        round_no += 1;
        let budget = per_call.min(user_max - total_ran).max(1);
        match run_parallel_round(&state, &oid, &job_id, budget, concurrency, round_no) {
            Ok((result, snap)) => {
                total_ran += result.ran as usize;
                let ran_this = result.ran;
                let status = result.status;
                agg.merge(&result);
                jobs.update(&job_id, |j| {
                    j.current_round += result.rounds;
                    j.ran = agg.ran;
                    j.completed = agg.completed;
                    j.failed = agg.failed;
                    j.blocked = agg.blocked;
                    if let Some(orch) = snap.as_ref() {
                        j.steps = job_steps(orch, &[]);
                    }
                    j.last_event = Some(result.summary.clone());
                });
                // Stop when no brief ran (nothing ready) or the orchestration is no
                // longer running (completed / needs attention). Either way the
                // pending set has stopped shrinking, so continuing would spin.
                if ran_this == 0 || !matches!(status, OrchestrationStatus::Running) {
                    break;
                }
            }
            Err(e) => {
                jobs.update(&job_id, |j| {
                    j.state = JobState::Failed;
                    j.error = Some(e.message.clone());
                    j.completed_at_ms = Some(now_millis());
                    j.last_event = Some("failed".to_string());
                });
                return;
            }
        }
    }

    let final_snap = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))
        .ok()
        .flatten();
    let rounds_done = agg.rounds;
    let final_result = agg.into_result(&oid, concurrency);
    jobs.update(&job_id, |j| {
        j.state = if canceled { JobState::Canceled } else { JobState::Completed };
        j.completed_at_ms = Some(now_millis());
        if let Some(orch) = final_snap.as_ref() {
            j.steps = job_steps(orch, &[]);
        }
        j.last_event = Some(if canceled {
            format!(
                "canceled after {rounds_done} round(s); any in-flight briefs finished and the \
                 remaining briefs were left pending for a human to resume or retire"
            )
        } else {
            final_result.summary.clone()
        });
        // The aggregate still reports exactly what really ran, so a canceled job is
        // observable (it never claims more progress than the kernel recorded).
        j.result = Some(final_result);
    });
}

/// The `run-async` response: the freshly-created job plus the URL to poll it.
#[derive(Debug, Serialize)]
struct StartJobResponse {
    #[serde(flatten)]
    job: OrchestrationJob,
    /// The relative URL the dashboard polls for this job's live status.
    status_url: String,
}

/// POST `/v1/relux/prime/orchestrations/:id/run-async` — start a non-blocking
/// run. Returns immediately with the queued job and a `status_url`. Rejects a
/// duplicate concurrent job for the same orchestration (409) and an over-cap fleet
/// (429). The orchestration must exist (404).
async fn start_orchestration_job(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<RunOrchestrationReq>>,
) -> Result<Json<StartJobResponse>, ApiError> {
    let oid = OrchestrationId::new(id.clone());
    let req = body.map(|b| b.0).unwrap_or_default();
    let max = req.max.unwrap_or(25).clamp(1, 25);
    let concurrency = req.concurrency.unwrap_or(2).clamp(1, 4);

    // Validate the orchestration exists before minting a job, so a bad id is an
    // honest 404 rather than a job that fails on its first round.
    locked_read(&state, |kernel| {
        kernel
            .orchestration(&oid)
            .map(|_| ())
            .ok_or_else(|| KernelError::UnknownOrchestration(id.clone()))
    })?;

    let job = state.jobs.start(&id, max, concurrency).map_err(|e| match e {
        JobStartError::Duplicate(existing) => ApiError {
            status: StatusCode::CONFLICT,
            message: format!(
                "an orchestration job ({existing}) is already running for {id}; poll it instead of starting another"
            ),
        },
        JobStartError::TooManyActive(n) => ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: format!(
                "too many orchestration jobs are active ({n}/{MAX_ACTIVE_JOBS}); wait for one to finish"
            ),
        },
    })?;

    // Spawn the worker on a dedicated OS thread: a job can run for minutes (real
    // CLI briefs), so it must not occupy the async reactor or the bounded blocking
    // pool the chat CLI path uses.
    let worker_state = state.clone();
    let worker_job = job.id.clone();
    let worker_oid = oid.clone();
    std::thread::spawn(move || {
        drive_orchestration_job(worker_state, worker_job, worker_oid, max, concurrency);
    });

    let status_url = format!("/v1/relux/orchestration-jobs/{}", job.id);
    Ok(Json(StartJobResponse { job, status_url }))
}

/// GET `/v1/relux/orchestration-jobs/:job_id` — poll one job's live status. 404
/// when the id is unknown. Job ids are **process-local**: the job registry is
/// in-memory, so a server restart loses them and a raw id can no longer be mapped
/// to its orchestration. The message points the caller at the restart-honest,
/// durable poll-by-orchestration-id endpoint instead of leaving a bare 404.
async fn get_orchestration_job(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<Json<OrchestrationJob>, ApiError> {
    state.jobs.get(&job_id).map(Json).ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        message: format!(
            "no orchestration job {job_id}. Job ids are process-local (the job registry is \
             in-memory) and are not retained across a server restart; poll \
             GET /v1/relux/prime/orchestrations/:id/job by orchestration id for durable, \
             restart-honest status."
        ),
    })
}

/// GET `/v1/relux/prime/orchestrations/:id/job` — the latest job for an
/// orchestration, so the dashboard can poll by orchestration id without tracking
/// the job id. A live in-process job wins (it carries real wall-clock + live
/// in-flight state). When none exists — including after a server restart, since
/// the registry is in-memory — the status is **reconstructed from the durable
/// orchestration record** so a poll stays honest instead of misleadingly 404-ing:
/// `completed` when every brief is terminal, `interrupted` when a prior run left
/// briefs pending and no worker is driving it now. The orchestration must exist
/// (404 otherwise); one that never ran a brief reports "no job started" (404) so
/// the dashboard falls back to the planned record.
async fn get_latest_orchestration_job(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<OrchestrationJob>, ApiError> {
    if let Some(job) = state.jobs.latest_for(&id) {
        return Ok(Json(job));
    }
    let oid = OrchestrationId::new(id.clone());
    let orch = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))?.ok_or_else(|| {
        ApiError {
            status: StatusCode::NOT_FOUND,
            message: format!("no orchestration {id}"),
        }
    })?;
    reconstruct_job_from_record(&orch)
        .map(Json)
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            message: format!("no orchestration job has been started for {id}"),
        })
}

/// POST `/v1/relux/orchestration-jobs/:job_id/cancel` — request cancellation of an
/// active orchestration job. This is cooperative and honest: it does NOT kill an
/// adapter process that is already running. The worker finishes the round that is
/// in flight (so no brief is interrupted), then stops before the next round and
/// marks the job `canceled`; remaining briefs stay pending for a human to resume.
/// Returns the updated job (200). 404 when unknown; 409 when the job already
/// finished (completed/failed) and so cannot be canceled.
async fn cancel_orchestration_job(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<Json<OrchestrationJob>, ApiError> {
    match state.jobs.request_cancel(&job_id) {
        CancelOutcome::Requested(job) | CancelOutcome::AlreadyCanceled(job) => Ok(Json(job)),
        CancelOutcome::AlreadyTerminal(job) => Err(ApiError {
            status: StatusCode::CONFLICT,
            message: format!(
                "orchestration job {job_id} already finished ({}); nothing to cancel",
                job.state.label()
            ),
        }),
        CancelOutcome::Unknown => Err(ApiError {
            status: StatusCode::NOT_FOUND,
            message: format!(
                "no orchestration job {job_id} (it may have been lost to a server restart)"
            ),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct InstallDirReq {
    path: String,
}

async fn install_dir(
    State(state): State<AppState>,
    Json(req): Json<InstallDirReq>,
) -> Result<Json<PluginRecord>, ApiError> {
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError::bad_request("path is required"));
    }
    let root = state.plugins_root.clone();
    let record = locked_save(&state, |kernel| {
        let installed = install_from_dir(std::path::Path::new(&path), &root, kernel)?;
        Ok(record_for(kernel, &installed))
    })?;
    Ok(Json(record))
}

#[derive(Debug, Deserialize)]
struct InstallGithubReq {
    url: String,
}

async fn install_github(
    State(state): State<AppState>,
    Json(req): Json<InstallGithubReq>,
) -> Result<Json<PluginRecord>, ApiError> {
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Err(ApiError::bad_request("url is required"));
    }
    let root = state.plugins_root.clone();
    let record = locked_save(&state, |kernel| {
        let installed = install_from_github(&url, &root, kernel)?;
        Ok(record_for(kernel, &installed))
    })?;
    Ok(Json(record))
}

async fn install_zip(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<PluginRecord>, ApiError> {
    // Pull the `file` field's bytes (await happens BEFORE we take the lock).
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart upload: {e}")))?
    {
        if field.name() == Some("file") {
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::bad_request(format!("failed to read upload: {e}")))?;
            bytes = Some(data.to_vec());
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| ApiError::bad_request("missing multipart field 'file'"))?;
    if bytes.is_empty() {
        return Err(ApiError::bad_request("uploaded file is empty"));
    }

    // Stage the upload under dev-data/relux/uploads, install, then always clean
    // up the temp file - success or failure.
    let temp = stage_upload(&state.uploads_root, &bytes)?;
    let root = state.plugins_root.clone();
    let result = locked_save(&state, |kernel| {
        let installed = install_from_zip(&temp, &root, kernel)?;
        Ok(record_for(kernel, &installed))
    });
    let _ = std::fs::remove_file(&temp);
    Ok(Json(result?))
}

async fn remove(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RemovedResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    let root = state.plugins_root.clone();
    locked_save(&state, |kernel| {
        // remove_plugin rejects bundled plugins and unknown ids, and only ever
        // deletes a directory inside the plugins root.
        remove_plugin(&id, &root, kernel)?;
        Ok(())
    })?;
    Ok(Json(RemovedResponse { removed: id }))
}

// --- Tool runtime (HTTP loopback) ------------------------------------------

/// The runtime status/config for one plugin. Carries no secrets - just the
/// loopback base URL, the enabled flag, and the timeout.
#[derive(Debug, Serialize)]
struct RuntimeConfigResponse {
    plugin_id: String,
    /// Whether a runtime is configured at all.
    configured: bool,
    /// The runtime kind, e.g. `"http_loopback"` (only when configured).
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
}

fn runtime_response(
    plugin_id: &str,
    config: Option<&relux_core::ToolRuntimeConfig>,
) -> RuntimeConfigResponse {
    match config {
        Some(c) => RuntimeConfigResponse {
            plugin_id: plugin_id.to_string(),
            configured: true,
            kind: Some(c.kind.as_str().to_string()),
            base_url: Some(c.base_url.clone()),
            enabled: c.enabled,
            timeout_ms: Some(c.timeout_ms),
        },
        None => RuntimeConfigResponse {
            plugin_id: plugin_id.to_string(),
            configured: false,
            kind: None,
            base_url: None,
            enabled: false,
            timeout_ms: None,
        },
    }
}

/// GET `/v1/relux/plugins/:id/runtime` - the current runtime config/status.
async fn get_plugin_runtime(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RuntimeConfigResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    let resp = locked_read(&state, |kernel| {
        // 404 for an unknown plugin so the UI can tell "not installed" from
        // "installed, no runtime".
        if kernel.installed_plugin(&plugin_id).is_none() {
            return Err(KernelError::PluginNotInstalled(id.clone()));
        }
        Ok(runtime_response(&id, kernel.tool_runtime_config(&plugin_id)))
    })?;
    Ok(Json(resp))
}

#[derive(Debug, Deserialize)]
struct RuntimeConfigReq {
    /// The loopback base URL. Required when no runtime exists yet; optional on a
    /// PATCH that only toggles `enabled`/`timeout_ms`.
    base_url: Option<String>,
    /// Defaults to enabled when configuring; can be set false to disable.
    enabled: Option<bool>,
    timeout_ms: Option<u64>,
}

/// PUT/PATCH `/v1/relux/plugins/:id/runtime` - configure (or update) the HTTP
/// loopback runtime. The base URL is validated as loopback-only; the plugin must
/// be installed and non-bundled. No secrets are accepted or stored.
async fn set_plugin_runtime(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<RuntimeConfigReq>,
) -> Result<Json<RuntimeConfigResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    let resp = locked_save(&state, |kernel| {
        // Merge with any existing config so a PATCH can omit base_url/timeout.
        let existing = kernel.tool_runtime_config(&plugin_id).cloned();
        let base_url = req
            .base_url
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| existing.as_ref().map(|c| c.base_url.clone()))
            .ok_or_else(|| KernelError::InvalidRuntimeConfig {
                plugin: id.clone(),
                message: "base_url is required to configure a runtime".to_string(),
            })?;
        let timeout_ms = req
            .timeout_ms
            .or_else(|| existing.as_ref().map(|c| c.timeout_ms));
        let enabled = req
            .enabled
            .or_else(|| existing.as_ref().map(|c| c.enabled))
            .unwrap_or(true);
        let cfg = kernel.configure_tool_runtime(&plugin_id, &base_url, enabled, timeout_ms)?;
        Ok(runtime_response(&id, Some(&cfg)))
    })?;
    Ok(Json(resp))
}

/// DELETE `/v1/relux/plugins/:id/runtime` - clear the runtime config entirely.
async fn delete_plugin_runtime(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RuntimeConfigResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    locked_save(&state, |kernel| {
        kernel.remove_tool_runtime(&plugin_id)?;
        Ok(())
    })?;
    Ok(Json(runtime_response(&id, None)))
}

/// The starter `relux-plugin.json` a metadata-only wrapper needs to become a real
/// ToolSet. Returned by the manifest-template endpoint so the dashboard can offer
/// a copy/download affordance: a generated wrapper declares NO tools, so a loopback
/// runtime alone never surfaces anything - the operator must add tool definitions.
#[derive(Debug, Serialize)]
struct ManifestTemplateResponse {
    plugin_id: String,
    /// The file name the operator should write into their plugin folder.
    filename: String,
    /// The absolute install directory of this plugin (where the file would live in
    /// the local index) - shown so the operator knows exactly where it goes.
    install_dir: String,
    /// Whether this plugin is a generated metadata-only wrapper (the case the
    /// template primarily serves).
    generated: bool,
    /// A complete, ready-to-edit `relux-plugin.json` with one example tool wired to
    /// this plugin's id. Filling it in (and pointing a loopback runtime at a local
    /// server) is what makes the plugin runnable. Relux never infers tools itself.
    manifest_json: String,
}

/// GET `/v1/relux/plugins/:id/manifest-template` - a starter `relux-plugin.json`
/// for an installed plugin (primarily a generated metadata-only wrapper). Honest
/// next step: a wrapper has no tool definitions, so configuring a runtime alone
/// surfaces nothing; the operator adds tools with this template, re-installs, then
/// configures a loopback runtime. Read-only; touches no config and stores no secret.
async fn plugin_manifest_template(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ManifestTemplateResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    let resp = locked_read(&state, |kernel| {
        let installed = kernel
            .installed_plugin(&plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(id.clone()))?;
        let manifest = kernel.plugin(&plugin_id);
        let name = manifest
            .map(|m| m.name.clone())
            .unwrap_or_else(|| id.clone());
        let generated = manifest.map(relux_kernel::is_generated_manifest).unwrap_or(false);
        // A starter ToolSet manifest keyed to THIS plugin's id, so the permission
        // strings line up with the kernel's `tool:<id>:<verb>` convention. The
        // operator edits the single example tool (or adds more) to describe what
        // their local loopback server actually exposes.
        let template = serde_json::json!({
            "id": id,
            "name": name,
            "version": "0.1.0",
            "kind": "ToolSet",
            "description": "Describe what this ToolSet does. Each tool below is exposed by a loopback HTTP server you run locally; Relux never runs downloaded code itself.",
            "author": "you",
            "trust_level": "community",
            "capabilities": {
                "tools": [
                    {
                        "name": "example.run",
                        "description": "Replace with a real tool your loopback server implements.",
                        "risk": "low",
                        "permission": format!("tool:{id}:run"),
                        "approval": "never",
                        "timeout_secs": 5
                    }
                ],
                "permissions": [
                    format!("tool:{id}:run")
                ]
            },
            "health": "unknown"
        });
        let manifest_json = serde_json::to_string_pretty(&template)
            .unwrap_or_else(|_| "{}".to_string());
        Ok(ManifestTemplateResponse {
            plugin_id: id.clone(),
            filename: "relux-plugin.json".to_string(),
            install_dir: installed.install_dir.clone(),
            generated,
            manifest_json,
        })
    })?;
    Ok(Json(resp))
}

/// POST `/v1/relux/plugins/:id/tools` - add or replace ONE operator-configured
/// tool on a user-installed ToolSet/wrapper plugin (`docs/RELUX_MASTER_PLAN.md`
/// §7.4, §8.2). This is the in-UI alternative to hand-editing a `relux-plugin.json`
/// and re-installing: the operator describes a tool and the kernel validates it
/// hard (allowlist fields, derived permission, risk-driven approval) before it
/// enters the manifest. Returns the updated plugin record so the page can refresh
/// the tool count + status. Bundled/protected and non-ToolSet plugins are refused.
async fn configure_plugin_tool(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<PluginRecord>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    // Validate + sanitize the operator payload BEFORE taking the lock; a bad
    // payload is an honest 400 and never touches the store.
    let input = relux_kernel::parse_plugin_tool_input(&body).map_err(ApiError::bad_request)?;
    let plugin_id = relux_core::PluginId::new(id.clone());
    let record = locked_save(&state, |kernel| {
        kernel.configure_plugin_tool(&plugin_id, input)?;
        let installed = kernel
            .installed_plugin(&plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(id.clone()))?
            .clone();
        Ok(record_for(kernel, &installed))
    })?;
    Ok(Json(record))
}

/// DELETE `/v1/relux/plugins/:id/tools/:tool` - remove one operator-configured
/// tool from a user-installed ToolSet plugin by name. Symmetric with
/// [`configure_plugin_tool`]: bundled plugins are refused and an unknown tool is a
/// 404. Returns the updated plugin record.
async fn remove_plugin_tool(
    State(state): State<AppState>,
    AxumPath((id, tool)): AxumPath<(String, String)>,
) -> Result<Json<PluginRecord>, ApiError> {
    let id = id.trim().to_string();
    let tool = tool.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("plugin id is required"));
    }
    if tool.is_empty() {
        return Err(ApiError::bad_request("tool name is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    let record = locked_save(&state, |kernel| {
        kernel.remove_plugin_tool(&plugin_id, &tool)?;
        let installed = kernel
            .installed_plugin(&plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(id.clone()))?
            .clone();
        Ok(record_for(kernel, &installed))
    })?;
    Ok(Json(record))
}

// --- Adapter runtime (local coding-agent CLIs) -----------------------------

/// GET `/v1/relux/adapters` - every installed Adapter plugin with its honest
/// runtime status (`docs/RELUX_MASTER_PLAN.md` section 8.1, Adapter Runtime v1).
/// No secrets - just kind/enabled/binary-on-PATH/limits.
async fn list_adapters(
    State(state): State<AppState>,
) -> Result<Json<Vec<relux_core::AdapterRuntimeStatus>>, ApiError> {
    let adapters = locked_read(&state, |kernel| Ok(kernel.adapter_runtime_status()))?;
    Ok(Json(adapters))
}

/// GET `/v1/relux/adapters/:id/runtime` - one adapter's runtime status. 404 when
/// the plugin is not an installed Adapter.
async fn get_adapter_runtime(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<relux_core::AdapterRuntimeStatus>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("adapter id is required"));
    }
    let status = locked_read(&state, |kernel| {
        kernel
            .adapter_runtime_status()
            .into_iter()
            .find(|a| a.plugin_id == id)
            .ok_or_else(|| KernelError::NotAnAdapter { plugin: id.clone() })
    })?;
    Ok(Json(status))
}

#[derive(Debug, Deserialize)]
struct AdapterRuntimeReq {
    /// Whether the CLI runtime is enabled. Omitted on a configure PUT means the
    /// kernel keeps the prior value (or `false` on first configure).
    enabled: Option<bool>,
    /// Optional binary override (required for a generic command adapter).
    command: Option<String>,
    timeout_seconds: Option<u64>,
    max_output_bytes: Option<u64>,
    working_dir: Option<String>,
}

/// PUT/PATCH `/v1/relux/adapters/:id/runtime` - configure (or update) an
/// adapter's local CLI runtime. Disabled by default; the local-prime adapter and
/// non-Adapter plugins are refused. No secrets are accepted or stored.
async fn set_adapter_runtime(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<AdapterRuntimeReq>,
) -> Result<Json<relux_core::AdapterRuntimeStatus>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("adapter id is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    let status = locked_save(&state, |kernel| {
        kernel.configure_adapter_runtime(
            &plugin_id,
            req.enabled,
            req.command,
            req.timeout_seconds,
            req.max_output_bytes,
            req.working_dir,
        )?;
        kernel
            .adapter_runtime_status()
            .into_iter()
            .find(|a| a.plugin_id == id)
            .ok_or_else(|| KernelError::NotAnAdapter { plugin: id.clone() })
    })?;
    Ok(Json(status))
}

/// DELETE `/v1/relux/adapters/:id/runtime` - clear an adapter's runtime config.
async fn delete_adapter_runtime(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<relux_core::AdapterRuntimeStatus>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("adapter id is required"));
    }
    let plugin_id = relux_core::PluginId::new(id.clone());
    let status = locked_save(&state, |kernel| {
        kernel.remove_adapter_runtime(&plugin_id)?;
        kernel
            .adapter_runtime_status()
            .into_iter()
            .find(|a| a.plugin_id == id)
            .ok_or_else(|| KernelError::NotAnAdapter { plugin: id.clone() })
    })?;
    Ok(Json(status))
}

// --- MCP servers (loopback HTTP discovery — MCP v1) ------------------------
// `docs/mcp.md`, `docs/RELUX_MASTER_PLAN.md` §8.2/§18, `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9.
// Operator-curated, loopback-ONLY MCP server registry + live `tools/list`
// discovery. No secrets are accepted or stored. MCP tool INVOCATION is not wired
// into the agent tool-call path yet (discovered tools are `not_implemented`).

/// One MCP server row for the list/register response. Mirrors the stored config
/// (no secrets) plus an honest one-word `status`.
#[derive(Debug, Serialize)]
struct McpServerResponse {
    id: String,
    transport: String,
    endpoint: String,
    description: String,
    enabled: bool,
    timeout_ms: u64,
    /// `configured` (enabled) or `disabled`. Reachability is dynamic — see the
    /// per-server tools endpoint.
    status: String,
    /// Operator-set per-tool risk/approval classifications, keyed by MCP tool name.
    /// Empty when no tool has been classified (every tool then uses the fail-closed
    /// default: Medium + Required → gated).
    tool_overrides: std::collections::BTreeMap<String, relux_core::McpToolClassification>,
}

impl McpServerResponse {
    fn from_config(c: &relux_core::McpServerConfig) -> Self {
        Self {
            id: c.id.clone(),
            transport: c.transport.as_str().to_string(),
            endpoint: c.endpoint.clone(),
            description: c.description.clone(),
            enabled: c.enabled,
            timeout_ms: c.timeout_ms,
            status: c.status_str().to_string(),
            tool_overrides: c.tool_overrides.clone(),
        }
    }
}

/// GET `/v1/relux/mcp/servers` — every registered MCP server (no secrets).
async fn list_mcp_servers(
    State(state): State<AppState>,
) -> Result<Json<Vec<McpServerResponse>>, ApiError> {
    let rows = locked_read(&state, |kernel| {
        Ok(kernel
            .mcp_servers()
            .into_iter()
            .map(McpServerResponse::from_config)
            .collect::<Vec<_>>())
    })?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct McpServerReq {
    /// Stable, unique id (`[A-Za-z0-9._-]`). Required.
    id: String,
    /// The loopback endpoint (validated loopback-only). Required.
    endpoint: String,
    description: Option<String>,
    /// Defaults to enabled on first register; can be set false to disable.
    enabled: Option<bool>,
    timeout_ms: Option<u64>,
}

/// POST `/v1/relux/mcp/servers` — register or update (upsert by id) an MCP server.
/// The endpoint is validated as loopback-only; no secrets are accepted or stored.
async fn register_mcp_server(
    State(state): State<AppState>,
    Json(req): Json<McpServerReq>,
) -> Result<Json<McpServerResponse>, ApiError> {
    let id = req.id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("MCP server id is required"));
    }
    let endpoint = req.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("MCP server endpoint is required"));
    }
    let description = req.description.unwrap_or_default();
    let enabled = req.enabled.unwrap_or(true);
    let resp = locked_save(&state, |kernel| {
        let cfg =
            kernel.register_mcp_server(&id, &endpoint, &description, enabled, req.timeout_ms)?;
        Ok(McpServerResponse::from_config(&cfg))
    })?;
    Ok(Json(resp))
}

/// DELETE `/v1/relux/mcp/servers/:id` — remove an MCP server registration.
async fn delete_mcp_server(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("MCP server id is required"));
    }
    locked_save(&state, |kernel| kernel.remove_mcp_server(&id))?;
    Ok(Json(serde_json::json!({ "removed": id })))
}

/// The live discovery response for one MCP server. Honest by construction: on a
/// transport/protocol failure the route surfaces the error (HTTP 4xx/5xx via
/// [`ApiError`]); a successful probe returns the discovered tools as
/// [`relux_core::ToolDescriptor`]s. Each tool's `executable` reflects its operator
/// classification: an unclassified tool is `needs_approval` (gated), a low-risk
/// auto-approve tool is `ready` (directly callable through the tool-invoke gates).
#[derive(Debug, Serialize)]
struct McpToolsResponse {
    server_id: String,
    /// True when the live `tools/list` probe succeeded.
    reachable: bool,
    tools: Vec<relux_core::ToolDescriptor>,
}

/// GET `/v1/relux/mcp/servers/:id/tools` — run a live `tools/list` against the
/// server and map the result into the Tools surface. Unknown id → 404; disabled →
/// 409; a transport/protocol failure → 502 (never a fabricated empty list).
async fn mcp_server_tools(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<McpToolsResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("MCP server id is required"));
    }
    // Discovery performs a bounded loopback network call; it mutates no state, so a
    // read lock is enough.
    let tools = locked_read(&state, |kernel| kernel.discover_mcp_tools(&id))?;
    Ok(Json(McpToolsResponse {
        server_id: id,
        reachable: true,
        tools,
    }))
}

/// The operator-set risk/approval classification for one MCP tool. Accepts the
/// native serde shapes: `risk` is `"low"|"medium"|"high"|"critical"`; `approval`
/// is `"never"|"required"|{"required_when_risk":"<level>"}`.
#[derive(Debug, Deserialize)]
struct McpToolClassificationReq {
    risk: relux_core::RiskLevel,
    approval: relux_core::ApprovalRequirement,
}

/// PUT/PATCH `/v1/relux/mcp/servers/:id/tools/:tool/classification` — set the risk +
/// approval for one discovered MCP tool, so it can become directly runnable
/// (low + never) or stay gated behind approval. Unknown server → 404; an invalid
/// tool name → 400. Returns the updated server config.
async fn set_mcp_tool_classification(
    State(state): State<AppState>,
    AxumPath((id, tool)): AxumPath<(String, String)>,
    Json(req): Json<McpToolClassificationReq>,
) -> Result<Json<McpServerResponse>, ApiError> {
    let id = id.trim().to_string();
    let tool = tool.trim().to_string();
    if id.is_empty() || tool.is_empty() {
        return Err(ApiError::bad_request("server id and tool name are required"));
    }
    let resp = locked_save(&state, |kernel| {
        let cfg = kernel.set_mcp_tool_classification(&id, &tool, req.risk, req.approval)?;
        Ok(McpServerResponse::from_config(&cfg))
    })?;
    Ok(Json(resp))
}

/// DELETE `/v1/relux/mcp/servers/:id/tools/:tool/classification` — clear a tool's
/// classification, reverting it to the fail-closed default (gated). Unknown server
/// → 404. Returns the updated server config.
async fn clear_mcp_tool_classification(
    State(state): State<AppState>,
    AxumPath((id, tool)): AxumPath<(String, String)>,
) -> Result<Json<McpServerResponse>, ApiError> {
    let id = id.trim().to_string();
    let tool = tool.trim().to_string();
    if id.is_empty() || tool.is_empty() {
        return Err(ApiError::bad_request("server id and tool name are required"));
    }
    let resp = locked_save(&state, |kernel| {
        let cfg = kernel.clear_mcp_tool_classification(&id, &tool)?;
        Ok(McpServerResponse::from_config(&cfg))
    })?;
    Ok(Json(resp))
}

/// The live `resources/list` response for one MCP server. Honest by construction: on
/// a transport/protocol failure the route surfaces the error (4xx/5xx via
/// [`ApiError`]); a successful probe returns the advertised resources. Resources are
/// READ-ONLY context — listing them performs no action and changes nothing.
#[derive(Debug, Serialize)]
struct McpResourcesResponse {
    server_id: String,
    /// True when the live `resources/list` probe succeeded.
    reachable: bool,
    resources: Vec<relux_core::McpResource>,
}

/// GET `/v1/relux/mcp/servers/:id/resources` — run a live `resources/list` against
/// the server. Unknown id → 404; disabled → 409; a transport/protocol failure → 502
/// (never a fabricated empty list). Read-only: a read lock is enough.
async fn mcp_server_resources(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<McpResourcesResponse>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("MCP server id is required"));
    }
    let resources = locked_read(&state, |kernel| kernel.list_mcp_resources(&id))?;
    Ok(Json(McpResourcesResponse {
        server_id: id,
        reachable: true,
        resources,
    }))
}

/// Query params for `GET /v1/relux/mcp/servers/:id/resources/read`.
#[derive(Debug, Deserialize)]
struct McpResourceReadQuery {
    /// The resource URI to read (validated non-empty, bounded, control-char free).
    uri: String,
}

/// GET `/v1/relux/mcp/servers/:id/resources/read?uri=…` — read ONE resource by URI,
/// returning the shaped, sanitized, secret-redacted content (text only; binary
/// summarized honestly). Unknown id → 404; disabled → 409; an invalid/empty URI →
/// 400; a transport/protocol failure → 502. Read-only: a `resources/read` is inert.
async fn mcp_read_resource_route(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    query: axum::extract::Query<McpResourceReadQuery>,
) -> Result<Json<relux_core::McpResourceContent>, ApiError> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("MCP server id is required"));
    }
    let uri = query.uri.trim().to_string();
    if uri.is_empty() {
        return Err(ApiError::bad_request("a resource `uri` query param is required"));
    }
    let content = locked_read(&state, |kernel| kernel.read_mcp_resource(&id, &uri))?;
    Ok(Json(content))
}

// --- Store access (serialized) ---------------------------------------------

/// Lock, open the store, load + bootstrap, run `f`, then SAVE. For mutations.
fn locked_save<F, T>(state: &AppState, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&mut KernelState) -> Result<T, KernelError>,
{
    // Recover from a poisoned lock rather than propagating the panic: the guard
    // only protects store ordering, and any partial state was already discarded.
    let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = SqliteStore::open(&state.db_path)?;
    let mut kernel = store.load()?;
    crate::ensure_bootstrapped(&mut kernel)?;
    let out = f(&mut kernel)?;
    store.save(&kernel)?;
    Ok(out)
}

/// Like [`locked_save`] but persists the kernel **even when `f` returns an
/// error**, then surfaces the error to the caller. Used for adapter execution and
/// retry. A CLI run that fails has already recorded a failed run, its transcript,
/// and an audit entry in kernel state; that record must survive so the dashboard
/// can show what happened and offer a retry, instead of being rolled back. The
/// CLI path already saves before propagating; this keeps the HTTP path consistent.
fn locked_save_persisting<F, T>(state: &AppState, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&mut KernelState) -> Result<T, KernelError>,
{
    let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = SqliteStore::open(&state.db_path)?;
    let mut kernel = store.load()?;
    crate::ensure_bootstrapped(&mut kernel)?;
    let result = f(&mut kernel);
    // Persist whatever the kernel recorded, success or honest failure, before
    // propagating the outcome.
    store.save(&kernel)?;
    Ok(result?)
}

/// Lock, open the store, load + bootstrap, run `f`. Read-only: no save.
fn locked_read<F, T>(state: &AppState, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&KernelState) -> Result<T, KernelError>,
{
    let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
    let store = SqliteStore::open(&state.db_path)?;
    let mut kernel = store.load()?;
    crate::ensure_bootstrapped(&mut kernel)?;
    Ok(f(&kernel)?)
}

/// Write uploaded bytes to a unique temp file under `uploads_root`.
///
/// The name is process- and counter-unique so concurrent uploads never collide;
/// the file always lands inside the uploads directory (never an arbitrary path).
fn stage_upload(uploads_root: &std::path::Path, bytes: &[u8]) -> Result<PathBuf, ApiError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    std::fs::create_dir_all(uploads_root).map_err(|e| {
        ApiError::internal(format!(
            "failed to create uploads dir {}: {e}",
            uploads_root.display()
        ))
    })?;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("upload-{}-{}.zip", std::process::id(), n);
    let path = uploads_root.join(name);
    std::fs::write(&path, bytes)
        .map_err(|e| ApiError::internal(format!("failed to stage upload: {e}")))?;
    Ok(path)
}

// --- Response shapes -------------------------------------------------------

/// Concise control-plane state summary - the JSON twin of the `state` CLI.
#[derive(Debug, Serialize)]
struct StateResponse {
    db_path: String,
    plugins: usize,
    installed_plugins: usize,
    namespaces: usize,
    agents: usize,
    tasks: usize,
    runs: usize,
    approvals: usize,
    open_tasks: usize,
    active_runs: usize,
    waiting_approval: usize,
    blocked: usize,
    failed: usize,
    pending_approvals: usize,
}

fn state_response(kernel: &KernelState, db_path: &std::path::Path) -> StateResponse {
    let s = kernel.inspect_state();
    StateResponse {
        db_path: db_path.display().to_string(),
        plugins: kernel.plugin_count(),
        installed_plugins: kernel.installed_plugin_count(),
        namespaces: kernel.namespace_count(),
        agents: kernel.agent_count(),
        tasks: kernel.task_count(),
        runs: kernel.run_count(),
        approvals: kernel.approval_count(),
        open_tasks: s.tasks_open,
        active_runs: s.runs_active,
        waiting_approval: s.tasks_waiting_approval,
        blocked: s.tasks_blocked,
        failed: s.tasks_failed,
        pending_approvals: s.pending_approvals,
    }
}

/// Consolidated health and readiness status for the Relux kernel.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    version: String,
    db_path: String,
    db_ok: bool,
    dashboard_bundle_present: bool,
    installed_plugin_count: usize,
    agent_count: usize,
    task_count: usize,
    run_count: usize,
    ai_status: AiStatus,
    warnings: Vec<String>,
    errors: Vec<String>,
}

async fn get_health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    let mut ok = true;
    let mut warnings = vec![];
    let mut errors = vec![];

    let version = crate::get_kernel_version().to_string();
    let db_path = state.db_path.display().to_string();
    let dashboard_bundle_present = state.dashboard_dir.is_some();
    let ai_status = resolve_ai(&state).status();

    if !dashboard_bundle_present {
        warnings.push("Dashboard bundle not found. Run `npm run build` in `apps/dashboard`".to_string());
        ok = false; // Missing dashboard bundle is a hard failure for readiness
    }

    if ai_status.mode == AiMode::Openrouter && !ai_status.configured {
        warnings.push("AI mode: OpenRouter (not configured, set OPENROUTER_API_KEY)".to_string());
    }

    let (
        db_ok,
        installed_plugin_count,
        agent_count,
        task_count,
        run_count,
    ) = {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        match SqliteStore::open(&state.db_path) {
            Ok(store) => match store.load() {
                Ok(kernel) => (
                    true,
                    kernel.installed_plugin_count(),
                    kernel.agent_count(),
                    kernel.task_count(),
                    kernel.run_count(),
                ),
                Err(e) => {
                    errors.push(format!("Failed to load kernel state from DB: {}", e));
                    ok = false;
                    (false, 0, 0, 0, 0)
                }
            },
            Err(e) => {
                errors.push(format!("Failed to open DB at {}: {}", state.db_path.display(), e));
                ok = false;
                (false, 0, 0, 0, 0)
            }
        }
    };

    Ok(Json(HealthResponse {
        ok,
        version,
        db_path,
        db_ok,
        dashboard_bundle_present,
        installed_plugin_count,
        agent_count,
        task_count,
        run_count,
        ai_status,
        warnings,
        errors,
    }))
}

/// GET `/v1/relux/doctor` — a read-only, structured operator diagnostics report
/// (`docs/relix-dashboard-design.md` §15). It reuses the SAME cheap reads as
/// `/v1/relux/health` (store open/load, dashboard bundle, AI status, adapter +
/// tool readiness, agent + approval counts) and turns them into severity rows via
/// [`relux_kernel::doctor::build_doctor_report`]. No heavy work (no cargo
/// build/test), no network, no mutation; the report carries no secrets or paths.
async fn get_doctor(
    State(state): State<AppState>,
) -> Result<Json<relux_kernel::doctor::DoctorReport>, ApiError> {
    let dashboard_bundle_present = state.dashboard_dir.is_some();
    let ai = resolve_ai(&state).status();

    // One serialized read of the store; on any open/load failure we still produce
    // an honest report whose `kernel.store` row fails (rather than 500ing).
    let (db_ok, adapters, tools, agent_count, pending_approvals, runs_needing_action, runs_retry_pending) = {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        match SqliteStore::open(&state.db_path).and_then(|store| store.load()) {
            Ok(kernel) => {
                let tools = kernel
                    .discover_tools(None)
                    .into_iter()
                    .filter(|t| !relux_kernel::is_internal_plugin(&t.plugin_id))
                    .collect::<Vec<_>>();
                (
                    true,
                    kernel.adapter_runtime_status(),
                    tools,
                    kernel.agent_count(),
                    kernel.pending_approval_count(),
                    kernel.runs_needing_operator_action(),
                    kernel.runs_retry_pending(),
                )
            }
            Err(_) => (false, Vec::new(), Vec::new(), 0, 0, 0, 0),
        }
    };

    let generated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let report = relux_kernel::doctor::build_doctor_report(
        &relux_kernel::doctor::DoctorInputs {
            db_ok,
            dashboard_bundle_present,
            ai: &ai,
            adapters: &adapters,
            tools: &tools,
            agent_count,
            pending_approvals,
            runs_needing_action,
            runs_retry_pending,
        },
        generated_at,
    );
    Ok(Json(report))
}

/// One task, flattened for the dashboard table.
#[derive(Debug, Serialize)]
struct TaskRecord {
    #[serde(flatten)]
    task: relux_core::Task,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignee_name: Option<String>,
}

/// One run, flattened for the dashboard table, with derived run-depth fields so
/// the Work page can show adapter/status/phase/duration/output excerpt + a clear
/// failure reason and retry affordance without re-deriving from the transcript
/// (master plan section 11.3 Active Runs).
#[derive(Debug, Serialize)]
struct RunRecord {
    #[serde(flatten)]
    run: relux_core::Run,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_title: Option<String>,
    /// The latest transcript event kind, i.e. the current/last phase
    /// (`run_started`, `adapter_spawn`, `adapter_output`, `run_completed`,
    /// `run_failed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    /// A bounded, already-redacted excerpt of the adapter's last output, for the
    /// run header. Pulled from the recorded transcript - never re-run.
    #[serde(skip_serializing_if = "Option::is_none")]
    output_excerpt: Option<String>,
    /// The honest failure reason for a failed run (the run's recorded error).
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<String>,
    /// The safe, static operator remediation for the run's failure class (single
    /// source of truth in `relux_core::run_failure`), so the UI shows guidance
    /// without re-implementing the class→advice mapping. `None` unless failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_remediation: Option<String>,
    /// Whether the dashboard should offer a Retry action: a failed run whose task
    /// still exists and is still assigned.
    retryable: bool,
    /// Whether the dashboard should offer a Resume action: a terminal run whose
    /// task is still assigned AND that captured a resumable provider session
    /// (`session.resume_supported`). Distinct from `retryable` — resume continues
    /// the recorded adapter session, retry starts cold (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md`
    /// §3). Mirrors the pure [`relux_core::plan_resume`] decision.
    resumable: bool,
}

/// The maximum number of characters of adapter output surfaced in a run header
/// excerpt. The full (capped, redacted) output stays available on the transcript.
const RUN_OUTPUT_EXCERPT_CHARS: usize = 2000;

/// Build a [`RunRecord`] with the derived run-depth fields, reading only the
/// already-persisted run + transcript (no re-execution).
fn build_run_record(kernel: &KernelState, run: relux_core::Run) -> RunRecord {
    let task = kernel.task(&run.task_id);
    let task_title = task.map(|t| t.title.clone());
    let events = kernel.run_events(&run.id);
    let phase = events.last().map(|e| e.kind.clone());
    // The latest adapter_output event's stdout is the freshest real output.
    let output_excerpt = events
        .iter()
        .rev()
        .find(|e| e.kind == "adapter_output")
        .and_then(|e| e.payload.get("stdout"))
        .and_then(|v| v.as_str())
        .map(|s| {
            let trimmed = s.trim();
            trimmed.chars().take(RUN_OUTPUT_EXCERPT_CHARS).collect::<String>()
        })
        .filter(|s| !s.is_empty());
    let failure_reason = run.error.clone();
    let failure_remediation = run.failure_class.map(|c| c.remediation().to_string());
    let retryable = run.status == relux_core::RunStatus::Failed
        && task.map(|t| t.assigned_agent.is_some()).unwrap_or(false);
    // Resume is offered when the task is still assigned AND the run captured a
    // resumable provider session and is terminal (the pure core decision is the
    // single source of truth — the UI label and the action agree).
    let terminal = matches!(
        run.status,
        relux_core::RunStatus::Completed | relux_core::RunStatus::Failed
    );
    let resumable = task.map(|t| t.assigned_agent.is_some()).unwrap_or(false)
        && relux_core::plan_resume(run.session.as_ref(), terminal).is_supported();
    RunRecord {
        run,
        task_title,
        phase,
        output_excerpt,
        failure_reason,
        failure_remediation,
        retryable,
        resumable,
    }
}

/// Build a [`TaskRecord`] from a `Task` and its assigned `Agent` (if any).
fn task_record(task: &relux_core::Task, agent: Option<&relux_core::Agent>) -> TaskRecord {
    TaskRecord {
        task: task.clone(),
        assignee_name: agent.map(|a| a.name.clone()),
    }
}

/// One agent record, flattened for the dashboard table.
#[derive(Debug, Serialize)]
struct AgentRecord {
    id: String,
    name: String,
    description: String,
    adapter_plugin: String,
    namespace: String,
    status: String,
    permissions_summary: String,
    /// The agent's EXPLICIT permission strings (the governance surface on the Crew
    /// page reads/edits these directly). Least privilege: there are no implicit/
    /// built-in capabilities, so this list is the agent's full effective power.
    permissions: Vec<String>,
    /// The agent's starter persona / operating style, when one was set (today via the
    /// brain-assisted agent-slot path). Omitted when none, so the wire stays compact.
    #[serde(skip_serializing_if = "Option::is_none")]
    persona: Option<String>,
    /// The agent's specialty tags/skills (bounded slugs). Always present (possibly
    /// empty) so the Crew UI can render chips and the assignment matcher reads them.
    skills: Vec<String>,
    /// This operative's Lead (`reports_to`) — the id of its manager in the org lattice,
    /// when set. `None` = top-level. Self-derivable from the agent (no roster needed).
    #[serde(skip_serializing_if = "Option::is_none")]
    reports_to: Option<String>,
    /// The Lead's display name, resolved against the live roster (the list endpoint
    /// enriches this; single-record responses leave it `None`). Display convenience only.
    #[serde(skip_serializing_if = "Option::is_none")]
    reports_to_name: Option<String>,
    /// Ids of this operative's DIRECT reports (the first level of its Branch), resolved
    /// against the live roster (list endpoint only). Always present (possibly empty) so
    /// the Crew card can show a compact count without a null check.
    reports: Vec<String>,
    created_at: String,
}

/// Build a [`AgentRecord`] from an `Agent`. The Lead id is self-derivable; the Lead's
/// display name and the direct-report ids need the roster and are enriched only by the
/// list endpoint (see [`list_agents`]), so they default to empty here.
fn agent_record(agent: &relux_core::Agent) -> AgentRecord {
    AgentRecord {
        id: agent.id.as_str().to_string(),
        name: agent.name.clone(),
        description: agent.description.clone(),
        adapter_plugin: agent.adapter_plugin.as_str().to_string(),
        namespace: agent.namespace_id.as_str().to_string(),
        status: format!("{:?}", agent.status),
        permissions_summary: format!("{} permissions", agent.permissions.len()),
        permissions: agent.permissions.iter().map(|p| p.to_string()).collect(),
        persona: agent.persona.clone(),
        skills: agent.skills.clone(),
        reports_to: agent.reports_to.as_ref().map(|m| m.as_str().to_string()),
        reports_to_name: None,
        reports: Vec::new(),
        created_at: agent.created_at.clone(),
    }
}

/// One installed plugin, flattened for the dashboard table. Carries the durable
/// install record plus the manifest's display fields when the manifest is in the
/// live index (it always is for a successful install).
#[derive(Debug, Serialize)]
struct PluginRecord {
    id: String,
    name: String,
    description: String,
    kind: String,
    version: String,
    enabled: bool,
    source_kind: String,
    source_label: String,
    install_dir: String,
    /// Bundled plugins are protected - they cannot be removed via the API.
    protected: bool,
    bundled: bool,
    /// True when Relux scaffolded this plugin's manifest because the source had no
    /// `relux-plugin.json`. Such a plugin is installed as metadata only and runs
    /// nothing until the operator configures a runtime or adds tool definitions.
    generated: bool,
    /// Count of tools the manifest declares. Zero for a generated wrapper (and any
    /// non-ToolSet plugin), so the dashboard can be honest that "metadata only"
    /// means there is nothing to make runnable until tool definitions are added.
    tool_count: usize,
    trust_level: Option<String>,
    health: Option<String>,
}

/// Build a [`PluginRecord`] from an install record + its (optional) manifest.
fn plugin_record(installed: &InstalledPlugin, manifest: Option<&PluginManifest>) -> PluginRecord {
    let bundled = installed.source_kind == PluginSourceKind::Bundled;
    PluginRecord {
        id: installed.id.as_str().to_string(),
        name: manifest
            .map(|m| m.name.clone())
            .unwrap_or_else(|| installed.id.as_str().to_string()),
        description: manifest.map(|m| m.description.clone()).unwrap_or_default(),
        kind: format!("{:?}", installed.kind),
        version: installed.version.clone(),
        enabled: installed.enabled,
        source_kind: format!("{:?}", installed.source_kind),
        source_label: installed.source_label.clone(),
        install_dir: installed.install_dir.clone(),
        protected: bundled,
        bundled,
        generated: manifest.map(relux_kernel::is_generated_manifest).unwrap_or(false),
        tool_count: manifest.map(|m| m.capabilities.tools.len()).unwrap_or(0),
        trust_level: manifest.map(|m| format!("{:?}", m.trust_level)),
        health: manifest.map(|m| format!("{:?}", m.health)),
    }
}

/// Build the record for one installed plugin by id, pulling its live manifest.
fn record_for(kernel: &KernelState, installed: &InstalledPlugin) -> PluginRecord {
    plugin_record(installed, kernel.plugin(&installed.id))
}

/// All installed plugins as flat records, sorted by id (kernel order).
fn plugin_records(kernel: &KernelState) -> Vec<PluginRecord> {
    kernel
        .installed_plugins()
        .into_iter()
        .map(|p| plugin_record(p, kernel.plugin(&p.id)))
        .collect()
}

#[derive(Debug, Serialize)]
struct RemovedResponse {
    removed: String,
}

// --- Errors ----------------------------------------------------------------

/// A handler error rendered as `(status, { "error": message })`.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}

impl From<KernelError> for ApiError {
    fn from(err: KernelError) -> Self {
        ApiError {
            status: status_for(&err),
            message: err.to_string(),
        }
    }
}

/// Map a [`KernelError`] to an honest HTTP status. Bad input is a 4xx; missing
/// plugins are 404; protected (bundled) removal is 409; storage/io is 500.
fn status_for(err: &KernelError) -> StatusCode {
    match err {
        KernelError::PluginNotInstalled(_)
        | KernelError::ToolNotFound { .. }
        | KernelError::PluginToolNotFound { .. }
        | KernelError::UnknownPlugin(_)
        | KernelError::UnknownAgent(_)
        // Revoking a permission the agent never held: the target capability is not
        // present, so a 404 (not a 400) is the honest shape.
        | KernelError::PermissionNotGranted(..)
        | KernelError::UnknownOrchestration(_)
        | KernelError::UnknownMcpServer(_)
        | KernelError::UnknownPersistentGrant(_) => StatusCode::NOT_FOUND,
        // A disabled MCP server is a resolvable conflict (enable it first); a live
        // discovery failure against the operator's loopback server is an upstream
        // (bad gateway) failure, surfaced honestly rather than as a fake empty list.
        KernelError::McpServerDisabled(_) => StatusCode::CONFLICT,
        // A live resource list/read failure against the operator's loopback server is
        // an upstream (bad gateway) failure, surfaced honestly (never a fake empty).
        KernelError::McpDiscoveryFailed { .. } | KernelError::McpResourceFetchFailed { .. } => {
            StatusCode::BAD_GATEWAY
        }
        // An invalid MCP tool name / resource URI (classification, invocation, read)
        // is bad input.
        KernelError::InvalidMcpToolName { .. } | KernelError::InvalidMcpResourceUri { .. } => {
            StatusCode::BAD_REQUEST
        }
        // A configured tool that requires approval cannot be invoked directly yet:
        // a conflict the operator resolves by lowering risk / enabling auto-approve,
        // or by requesting a per-call approval.
        KernelError::ToolRequiresApproval { .. } => StatusCode::CONFLICT,
        // Per-tool-call approval flow honest 4xx mappings: requesting approval for a
        // directly-runnable tool / oversized args is bad input (400); an
        // approval id with no bound invocation is a 404; executing an
        // undecided/consumed approval is a resolvable conflict (409); a tampered
        // stored snapshot is unprocessable (422, fail closed).
        KernelError::ToolDoesNotRequireApproval { .. }
        | KernelError::ToolInvocationArgsTooLarge { .. }
        | KernelError::InvalidAgentConfig(_) => StatusCode::BAD_REQUEST,
        KernelError::NoBoundToolInvocation(_) => StatusCode::NOT_FOUND,
        KernelError::ToolInvocationNotApproved { .. }
        | KernelError::ToolInvocationConsumed(_) => StatusCode::CONFLICT,
        KernelError::ToolInvocationArgsTampered(_) => StatusCode::UNPROCESSABLE_ENTITY,
        // Configuring a tool on a non-ToolSet / malformed definition: honest 400s.
        KernelError::PluginNotToolConfigurable { .. }
        | KernelError::InvalidToolDefinition { .. } => StatusCode::BAD_REQUEST,
        // A goal that does not split into multiple briefs: honest unprocessable
        // input, not a server fault.
        KernelError::OrchestrationNotMultiAgent => StatusCode::UNPROCESSABLE_ENTITY,
        KernelError::BundledPluginProtected(_) => StatusCode::CONFLICT,
        KernelError::RuntimeNotConfigured { .. } => StatusCode::NOT_FOUND,
        // Adapter runtime errors, mapped honestly. "Not an adapter" / "not
        // configured" are 404; disabled is a resolvable conflict (409); a missing
        // binary the operator must install is 422; a failed/timed-out process is
        // an upstream failure (502); a bad config is a 400.
        KernelError::NotAnAdapter { .. } | KernelError::AdapterRuntimeNotConfigured { .. } => {
            StatusCode::NOT_FOUND
        }
        KernelError::AdapterRuntimeDisabled { .. } => StatusCode::CONFLICT,
        // Retrying a run that is not in a failed state is a resolvable conflict.
        KernelError::RunNotRetryable { .. } => StatusCode::CONFLICT,
        // (Re)assigning a task that has reached a terminal state is a resolvable
        // conflict — the operator/manager must target a live task.
        KernelError::TaskNotAssignable { .. } => StatusCode::CONFLICT,
        // An honest "this run cannot be resumed" (no captured session, an adapter
        // without safe non-interactive resume, or a run still in flight): the
        // request is well-formed but the action does not apply — unprocessable.
        KernelError::RunResumeNotSupported { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        // Proposed-change apply outcomes, mapped honestly: an unknown change is a
        // 404; a not-approved change or a baseline conflict is a resolvable
        // conflict (409); a structurally inapplicable change (no baseline hash, no
        // workspace root, unsafe/irregular target) is unprocessable (422).
        KernelError::UnknownProposedChange { .. } => StatusCode::NOT_FOUND,
        KernelError::ProposedChangeNotApproved { .. }
        | KernelError::ProposedChangeConflict { .. } => StatusCode::CONFLICT,
        KernelError::ProposedChangeNotApplicable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        // The transactional (multi-file) apply maps the same way: a baseline
        // conflict is a resolvable 409; any other refusal is unprocessable (422).
        KernelError::ProposedChangeSetConflict { .. } => StatusCode::CONFLICT,
        KernelError::ProposedChangeSetNotApplicable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        KernelError::AdapterBinaryMissing { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        KernelError::AdapterExecutionFailed { .. } => StatusCode::BAD_GATEWAY,
        KernelError::AdapterNotConfigurable { .. } | KernelError::InvalidAdapterConfig { .. } => {
            StatusCode::BAD_REQUEST
        }
        // A tool installed as metadata but with no runtime handler/config yet:
        // honest "not implemented", not a server fault or a fabricated success.
        KernelError::ToolRuntimeUnavailable { .. } => StatusCode::NOT_IMPLEMENTED,
        // A configured-but-disabled runtime: a conflict the operator can resolve.
        KernelError::ToolRuntimeDisabled { .. } => StatusCode::CONFLICT,
        // The operator's loopback server failed/timed out/returned bad data: this
        // is an upstream (bad gateway) failure, surfaced honestly.
        KernelError::ToolRuntimeInvocation { .. } => StatusCode::BAD_GATEWAY,
        KernelError::PermissionDenied { .. } => StatusCode::FORBIDDEN,
        KernelError::UnsafePluginPath(_)
        | KernelError::PluginInstall(_)
        | KernelError::InvalidRuntimeConfig { .. }
        | KernelError::ManifestParse { .. }
        | KernelError::ManifestInvalid { .. } => StatusCode::BAD_REQUEST,
        KernelError::Io { .. } | KernelError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::{
        permission::{ApprovalRequirement, RiskLevel, ToolDefinition},
        Permission, PluginCapability, PluginHealth, PluginId, PluginKind, TrustLevel,
    };

    fn echo_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-echo"),
            name: "Echo".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "echoes input".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![ToolDefinition {
                    name: "echo.say".to_string(),
                    description: "echoes".to_string(),
                    risk: RiskLevel::Low,
                    permission: Permission::new("tool:relux-tools-echo:say").unwrap(),
                    approval: ApprovalRequirement::Never,
                    timeout_secs: Some(5),
                }],
                permissions: vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    #[test]
    fn bind_failure_message_addr_in_use_is_actionable() {
        let addr: SocketAddr = "127.0.0.1:19891".parse().unwrap();
        let err = std::io::Error::new(std::io::ErrorKind::AddrInUse, "address in use");
        let msg = bind_failure_message(addr, &err);
        // Names the busy address and explains the likely cause.
        assert!(msg.contains("127.0.0.1:19891"), "got: {msg}");
        assert!(msg.contains("already in use"), "got: {msg}");
        assert!(msg.contains("http://127.0.0.1:19891/dashboard"), "got: {msg}");
        // Surfaces the documented override on both a source checkout and the bundle.
        assert!(msg.contains("RELUX_HTTP_ADDR"), "got: {msg}");
        assert!(msg.contains("Start-Relux.ps1 -Port"), "got: {msg}");
        // Suggests a concrete alternative port to use.
        assert!(msg.contains("20000"), "got: {msg}");
    }

    fn run_with(artifacts: Vec<relux_core::RunArtifact>) -> relux_core::Run {
        relux_core::Run {
            id: relux_core::RunId::new("run_0001"),
            task_id: relux_core::TaskId::new("task_0001"),
            agent_id: relux_core::AgentId::new("agent_0001"),
            adapter_plugin: PluginId::new("relux-adapter-claude-cli"),
            status: relux_core::RunStatus::Completed,
            started_at: Some("t0".into()),
            ended_at: Some("t1".into()),
            summary: Some("done".into()),
            error: None,
            duration_ms: Some(10),
            usage: None,
            cost: None,
            retried_from: None,
            resumed_from: None,
            session: None,
            artifacts,
            proposed_changes: Vec::new(),
            failure_class: None,
            retry: None,
        }
    }

    fn record_of(run: relux_core::Run) -> RunRecord {
        RunRecord {
            run,
            task_title: Some("a task".into()),
            phase: Some("run_completed".into()),
            output_excerpt: None,
            failure_reason: None,
            failure_remediation: None,
            retryable: false,
            resumable: false,
        }
    }

    #[test]
    fn run_record_flattens_artifacts_onto_the_detail_response() {
        // GET /v1/relux/runs/:id returns a flattened RunRecord; an artifact-bearing
        // run must surface `artifacts[].type` on the wire for the dashboard.
        let run = run_with(vec![relux_core::RunArtifact {
            name: "main.rs".into(),
            kind: relux_core::ArtifactKind::File,
            summary: Some("edited".into()),
            source: "claude-cli".into(),
            path: Some("src/main.rs".into()),
            bytes: Some(42),
            truncated: false,
        }]);
        let json = serde_json::to_value(record_of(run)).unwrap();
        let arts = json.get("artifacts").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].get("type").and_then(|v| v.as_str()), Some("file"));
        assert_eq!(arts[0].get("name").and_then(|v| v.as_str()), Some("main.rs"));
        assert_eq!(arts[0].get("source").and_then(|v| v.as_str()), Some("claude-cli"));
    }

    #[test]
    fn run_record_omits_artifacts_when_there_are_none() {
        // The honest empty state: no `artifacts` key, so the dashboard renders the
        // empty state rather than an empty (or fabricated) list.
        let json = serde_json::to_value(record_of(run_with(Vec::new()))).unwrap();
        assert!(json.get("artifacts").is_none());
        assert!(json.get("proposed_changes").is_none());
    }

    #[test]
    fn run_record_flattens_proposed_changes_with_status_onto_the_detail() {
        // GET /v1/relux/runs/:id flattens the run, so a proposed change surfaces
        // with its path + lifecycle status for the dashboard's review/apply UI.
        let mut run = run_with(Vec::new());
        run.proposed_changes = vec![relux_core::ProposedChange {
            path: "src/main.rs".into(),
            action: relux_core::ProposedChangeAction::Replace,
            dest_path: None,
            new_content: "fn main() {}\n".into(),
            baseline_sha256: Some(relux_core::sha256_hex(b"old")),
            new_sha256: relux_core::sha256_hex(b"fn main() {}\n"),
            bytes: 13,
            source: "claude-cli".into(),
            status: relux_core::ProposedChangeStatus::Approved,
            review_note: None,
            refused_reason: None,
            applied_at: None,
        }];
        let json = serde_json::to_value(record_of(run)).unwrap();
        let cs = json.get("proposed_changes").and_then(|v| v.as_array()).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].get("path").and_then(|v| v.as_str()), Some("src/main.rs"));
        assert_eq!(cs[0].get("status").and_then(|v| v.as_str()), Some("approved"));
        assert_eq!(cs[0].get("action").and_then(|v| v.as_str()), Some("replace"));
        assert_eq!(cs[0].get("new_content").and_then(|v| v.as_str()), Some("fn main() {}\n"));
    }

    #[test]
    fn bind_failure_message_suggests_distinct_alt_port() {
        // When the busy port already equals the suggested alternative, suggest another.
        let addr: SocketAddr = "127.0.0.1:20000".parse().unwrap();
        let err = std::io::Error::new(std::io::ErrorKind::AddrInUse, "address in use");
        let msg = bind_failure_message(addr, &err);
        assert!(msg.contains("20001"), "got: {msg}");
    }

    #[test]
    fn bind_failure_message_other_errors_stay_generic() {
        let addr: SocketAddr = "127.0.0.1:19891".parse().unwrap();
        let err = std::io::Error::other("boom");
        let msg = bind_failure_message(addr, &err);
        assert!(msg.starts_with("failed to bind 127.0.0.1:19891"), "got: {msg}");
        assert!(msg.contains("boom"), "got: {msg}");
    }

    #[tokio::test]
    async fn bind_failure_message_maps_a_real_port_conflict() {
        // Hold an ephemeral loopback port, then try to bind the SAME address again.
        // A second bind without SO_REUSEADDR is AddrInUse on Linux/macOS/Windows.
        let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = held.local_addr().unwrap();
        let err = tokio::net::TcpListener::bind(addr).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse, "real second bind kind");
        let msg = bind_failure_message(addr, &err);
        assert!(msg.contains(&addr.to_string()), "got: {msg}");
        assert!(msg.contains("already in use"), "got: {msg}");
    }

    #[test]
    fn bundled_record_is_protected_and_carries_manifest_fields() {
        let mut kernel = KernelState::new();
        let installed = kernel.install_plugin(
            echo_manifest(),
            PluginSourceKind::Bundled,
            "bundled example".to_string(),
            "examples/relux-plugins/relux-tools-echo".to_string(),
            true,
        );
        let record = record_for(&kernel, &installed);
        assert_eq!(record.id, "relux-tools-echo");
        assert_eq!(record.name, "Echo");
        assert_eq!(record.description, "echoes input");
        assert_eq!(record.kind, "ToolSet");
        assert_eq!(record.source_kind, "Bundled");
        assert!(record.protected, "bundled plugins must be protected");
        assert!(record.bundled);
        assert!(record.enabled);
        assert_eq!(record.trust_level.as_deref(), Some("Official"));
    }

    #[test]
    fn record_without_manifest_falls_back_to_id() {
        let installed = InstalledPlugin {
            id: PluginId::new("relux-tools-orphan"),
            version: "9.9.9".to_string(),
            kind: PluginKind::ToolSet,
            installed_at: "T0".to_string(),
            source_kind: PluginSourceKind::LocalDir,
            source_label: "/tmp/orphan".to_string(),
            install_dir: "/data/orphan".to_string(),
            enabled: true,
        };
        let record = plugin_record(&installed, None);
        assert_eq!(record.name, "relux-tools-orphan");
        assert_eq!(record.description, "");
        assert!(!record.protected, "a local-dir plugin is removable");
        assert_eq!(record.trust_level, None);
    }

    #[test]
    fn plugin_record_serializes_expected_keys() {
        let installed = InstalledPlugin {
            id: PluginId::new("relux-tools-echo"),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            installed_at: "T0".to_string(),
            source_kind: PluginSourceKind::LocalDir,
            source_label: "/src".to_string(),
            install_dir: "/dst".to_string(),
            enabled: true,
        };
        let v = serde_json::to_value(plugin_record(&installed, None)).unwrap();
        for key in [
            "id",
            "name",
            "description",
            "kind",
            "version",
            "enabled",
            "source_kind",
            "source_label",
            "install_dir",
            "protected",
            "bundled",
            "generated",
            "tool_count",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
    }

    /// A metadata-only wrapper, shaped exactly like `scaffold_manifest` produces:
    /// authored by [`GENERATED_MANIFEST_AUTHOR`], zero tools, zero permissions.
    fn generated_wrapper_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: PluginId::new(id),
            name: format!("{id} (metadata only)"),
            version: "0.0.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "Installed as metadata: no runnable tools yet.".to_string(),
            author: relux_kernel::GENERATED_MANIFEST_AUTHOR.to_string(),
            trust_level: TrustLevel::Unverified,
            capabilities: PluginCapability {
                tools: Vec::new(),
                permissions: Vec::new(),
            },
            health: PluginHealth::Unknown,
        }
    }

    #[test]
    fn generated_wrapper_record_is_flagged_and_has_zero_tools() {
        let mut kernel = KernelState::new();
        let installed = kernel.install_plugin(
            generated_wrapper_manifest("relux-plugin-my-cool-repo"),
            PluginSourceKind::Github,
            "https://github.com/owner/my-cool-repo".to_string(),
            "/data/relux-plugin-my-cool-repo".to_string(),
            true,
        );
        let record = record_for(&kernel, &installed);
        assert!(record.generated, "wrapper must be flagged generated");
        assert_eq!(record.tool_count, 0, "a wrapper declares no tools");
        assert!(!record.protected, "a github install is removable");
    }

    #[test]
    fn real_toolset_record_reports_its_tool_count() {
        let mut kernel = KernelState::new();
        let installed = kernel.install_plugin(
            echo_manifest(),
            PluginSourceKind::LocalDir,
            "/src/echo".to_string(),
            "/data/echo".to_string(),
            true,
        );
        let record = record_for(&kernel, &installed);
        assert!(!record.generated, "a real manifest is not generated");
        assert_eq!(record.tool_count, 1, "echo declares one tool");
    }

    /// The honest dead-end: a generated wrapper has no tool definitions, so even an
    /// enabled loopback runtime surfaces NOTHING. This pins why the dashboard must
    /// route a metadata-only plugin to "add a manifest", not "configure a runtime".
    #[test]
    fn enabling_a_runtime_on_a_wrapper_surfaces_no_tools() {
        let mut kernel = KernelState::new();
        let id = PluginId::new("relux-plugin-empty");
        kernel.install_plugin(
            generated_wrapper_manifest("relux-plugin-empty"),
            PluginSourceKind::Github,
            "https://github.com/owner/empty".to_string(),
            "/data/relux-plugin-empty".to_string(),
            true,
        );
        kernel
            .configure_tool_runtime(&id, "http://127.0.0.1:19999", true, None)
            .expect("configure runtime");
        let tools = kernel.discover_tools(None);
        assert!(
            !tools.iter().any(|t| t.plugin_id == "relux-plugin-empty"),
            "a wrapper with no tool definitions yields no tools even with a runtime"
        );
    }

    /// The resolution of that dead-end: configuring a tool ON the wrapper makes a
    /// tool appear (and the record's tool_count tracks it), so the in-UI "add a
    /// tool" path genuinely turns a metadata-only wrapper into something usable.
    #[test]
    fn configuring_a_tool_on_a_wrapper_makes_it_appear_and_bumps_the_record() {
        let mut kernel = KernelState::new();
        let id = PluginId::new("relux-plugin-empty");
        let installed = kernel.install_plugin(
            generated_wrapper_manifest("relux-plugin-empty"),
            PluginSourceKind::Github,
            "https://github.com/owner/empty".to_string(),
            "/data/relux-plugin-empty".to_string(),
            true,
        );
        // Before: a generated wrapper with zero tools.
        assert!(relux_kernel::is_generated_manifest(kernel.plugin(&id).unwrap()));
        assert_eq!(record_for(&kernel, &installed).tool_count, 0);

        // Add a low-risk tool the same way the HTTP handler validates it.
        let body = serde_json::json!({ "name": "report.fetch", "risk": "low" });
        let input = relux_kernel::parse_plugin_tool_input(&body).expect("valid input");
        let def = kernel.configure_plugin_tool(&id, input).expect("configure tool");
        assert_eq!(def.permission.as_str(), "tool:relux-plugin-empty:fetch");

        // After: the tool is discoverable, and the record's tool_count tracks it.
        let tools = kernel.discover_tools(None);
        assert!(tools
            .iter()
            .any(|t| t.plugin_id == "relux-plugin-empty" && t.tool_name == "report.fetch"));
        let installed = kernel.installed_plugin(&id).unwrap().clone();
        assert_eq!(record_for(&kernel, &installed).tool_count, 1);

        // It is still honestly NOT runnable until a loopback runtime is enabled.
        let t = tools.iter().find(|t| t.tool_name == "report.fetch").unwrap();
        assert_eq!(t.executable, relux_core::ToolExecutability::RuntimeNotConfigured);
    }

    /// The HTTP-layer status mapping for the new errors is honest: a bundled plugin
    /// is a 409 conflict, an unknown plugin a 404, and a tool that requires approval
    /// a 409 (the operator resolves it by lowering risk).
    #[test]
    fn tool_config_error_status_codes_are_honest() {
        assert_eq!(
            status_for(&KernelError::BundledPluginProtected("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&KernelError::PluginToolNotFound {
                plugin: "x".into(),
                tool: "y".into()
            }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&KernelError::ToolRequiresApproval {
                plugin: "x".into(),
                tool: "y".into()
            }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&KernelError::PluginNotToolConfigurable {
                plugin: "x".into(),
                message: "m".into()
            }),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn manifest_template_is_valid_json_keyed_to_the_plugin() {
        // Build the template inline the same way the handler does, then prove it is
        // a re-installable manifest: valid JSON, ToolSet, with permission strings
        // bound to THIS plugin id.
        let id = "relux-plugin-my-cool-repo";
        let template = serde_json::json!({
            "id": id,
            "name": "My Cool Repo",
            "version": "0.1.0",
            "kind": "ToolSet",
            "description": "x",
            "author": "you",
            "trust_level": "community",
            "capabilities": {
                "tools": [{
                    "name": "example.run",
                    "description": "x",
                    "risk": "low",
                    "permission": format!("tool:{id}:run"),
                    "approval": "never",
                    "timeout_secs": 5
                }],
                "permissions": [format!("tool:{id}:run")]
            },
            "health": "unknown"
        });
        let json = serde_json::to_string_pretty(&template).unwrap();
        // It round-trips into a real PluginManifest and validates like any other.
        let parsed: PluginManifest = serde_json::from_str(&json).expect("template parses");
        assert_eq!(parsed.id.as_str(), id);
        assert_eq!(parsed.kind, PluginKind::ToolSet);
        assert_eq!(parsed.capabilities.tools.len(), 1);
        assert_eq!(
            parsed.capabilities.tools[0].permission.as_str(),
            "tool:relux-plugin-my-cool-repo:run"
        );
    }

    #[test]
    fn error_status_mapping_is_honest() {
        assert_eq!(
            status_for(&KernelError::PluginNotInstalled("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&KernelError::BundledPluginProtected("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&KernelError::PluginInstall("bad".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&KernelError::UnsafePluginPath("../x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&KernelError::Storage("disk".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        // Tool invocation errors map honestly: not-implemented -> 501,
        // permission denied -> 403, unknown tool -> 404.
        assert_eq!(
            status_for(&KernelError::ToolRuntimeUnavailable {
                plugin: "p".into(),
                tool: "t".into()
            }),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            status_for(&KernelError::PermissionDenied {
                agent: "a".into(),
                permission: "tool:x:y".into()
            }),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_for(&KernelError::ToolNotFound {
                plugin: "p".into(),
                tool: "t".into()
            }),
            StatusCode::NOT_FOUND
        );
        // Runtime config + invocation errors map honestly.
        assert_eq!(
            status_for(&KernelError::InvalidRuntimeConfig {
                plugin: "p".into(),
                message: "bad url".into()
            }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&KernelError::RuntimeNotConfigured { plugin: "p".into() }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&KernelError::ToolRuntimeDisabled { plugin: "p".into() }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&KernelError::ToolRuntimeInvocation {
                plugin: "p".into(),
                tool: "t".into(),
                message: "timeout".into()
            }),
            StatusCode::BAD_GATEWAY
        );
        // Transactional (multi-file) apply: a workspace conflict is a 409; any
        // other refusal of the set is a 422 — mirroring the single-change apply.
        assert_eq!(
            status_for(&KernelError::ProposedChangeSetConflict {
                run: "r".into(),
                reason: "baseline mismatch".into()
            }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&KernelError::ProposedChangeSetNotApplicable {
                run: "r".into(),
                reason: "duplicate target".into()
            }),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    fn conversational_turn(reply: &str) -> PrimeTurn {
        PrimeTurn {
            intent: relux_core::PrimeIntent::Greeting,
            reply: reply.to_string(),
            disposition: relux_core::PrimeDisposition::Answered,
            action: None,
            created_task: None,
            started_run: None,
            created_agent: None,
            approval: None,
            invoked_tool: None,
            tool_output: None,
            tool_error: None,
            suggested_actions: Vec::new(),
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
            assign_slots: None,
            update: None,
            context_reads: vec![],
        }
    }

    #[tokio::test]
    async fn cli_brain_not_installed_falls_back_with_actionable_note() {
        // No adapter status at all -> keep the grounded reply, tell the operator
        // exactly what to do. Never blank, never a fabricated Claude answer.
        let turn = conversational_turn("There is 1 active run.");
        let outcome =
            run_cli_brain(relux_kernel::PrimeBrain::ClaudeCli, None, "hey", &turn, "").await;
        assert_eq!(outcome.mode, AiMode::Deterministic);
        assert_eq!(outcome.reply, "There is 1 active run.");
        let note = outcome.note.expect("a note explaining the next step");
        assert!(note.contains("Claude CLI"), "note: {note}");
        assert!(note.contains("Crew"), "note points to Crew → Adapters: {note}");
    }

    #[tokio::test]
    async fn cli_brain_disabled_adapter_explains_how_to_enable() {
        // Adapter exists but is disabled -> actionable "enable it" note, grounded
        // reply preserved, mode stays deterministic (Claude did not answer).
        let turn = conversational_turn("Idle.");
        let status = relux_core::AdapterRuntimeStatus {
            plugin_id: relux_core::CLAUDE_CLI_ADAPTER_ID.to_string(),
            adapter_name: "Claude CLI".to_string(),
            kind: Some("claude_cli".to_string()),
            configured: true,
            enabled: false,
            command: Some("claude".to_string()),
            available_on_path: true,
            resolved_path: None,
            timeout_seconds: Some(120),
            max_output_bytes: Some(1_000_000),
            working_dir: None,
            state: relux_core::AdapterRuntimeState::Disabled,
            detail: "configured but disabled".to_string(),
        };
        let outcome = run_cli_brain(
            relux_kernel::PrimeBrain::ClaudeCli,
            Some(status),
            "hey",
            &turn,
            "",
        )
        .await;
        assert_eq!(outcome.mode, AiMode::Deterministic);
        assert_eq!(outcome.reply, "Idle.");
        let note = outcome.note.expect("a note");
        assert!(note.to_lowercase().contains("enable"), "note: {note}");
    }

    // --- Non-blocking orchestration job tests -----------------------------

    use relux_core::{
        AgentId, NamespaceId, Orchestration, OrchestrationRole, OrchestrationStep, TaskId,
    };

    fn step(task: &str, role: OrchestrationRole, outcome: StepOutcome, deps: Vec<usize>) -> OrchestrationStep {
        OrchestrationStep {
            task_id: TaskId::new(task),
            agent_id: AgentId::new("prime"),
            role,
            title: format!("brief {task}"),
            outcome,
            depends_on: deps,
            run_id: None,
            note: None,
            started_at: None,
            finished_at: None,
            round: None,
        }
    }

    fn orchestration_with(steps: Vec<OrchestrationStep>) -> Orchestration {
        Orchestration {
            id: OrchestrationId::new("orch_0001"),
            goal: "goal".to_string(),
            created_by: "founder".to_string(),
            namespace_id: NamespaceId::new("workspace"),
            status: OrchestrationStatus::Planned,
            steps,
            notes: vec![],
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
            last_batch_summary: None,
        }
    }

    #[test]
    fn registry_starts_a_queued_job_with_a_status_url_shape() {
        let reg = JobRegistry::default();
        let job = reg.start("orch_0001", 25, 2).expect("first job starts");
        assert_eq!(job.state, JobState::Queued);
        assert_eq!(job.orchestration_id, "orch_0001");
        assert_eq!(job.id, "job_0001");
        assert_eq!(job.max, 25);
        assert_eq!(job.concurrency, 2);
        assert!(job.started_at_ms.is_none(), "queued job has not started");
        assert!(reg.get("job_0001").is_some());
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn registry_rejects_a_duplicate_concurrent_job_for_the_same_orchestration() {
        let reg = JobRegistry::default();
        let first = reg.start("orch_0001", 25, 2).expect("first starts");
        match reg.start("orch_0001", 25, 2) {
            Err(JobStartError::Duplicate(existing)) => assert_eq!(existing, first.id),
            other => panic!("expected duplicate rejection, got {:?}", other.is_ok()),
        }
        // A different orchestration is allowed concurrently.
        assert!(reg.start("orch_0002", 25, 2).is_ok());
    }

    #[test]
    fn registry_allows_a_new_job_once_the_prior_one_is_terminal() {
        let reg = JobRegistry::default();
        let first = reg.start("orch_0001", 25, 2).unwrap();
        reg.update(&first.id, |j| j.state = JobState::Completed);
        // The completed job no longer blocks a fresh run of the same orchestration.
        let second = reg.start("orch_0001", 25, 2).expect("a new job after completion");
        assert_ne!(second.id, first.id);
        // latest_for returns the newest (zero-padded ids sort lexically).
        assert_eq!(reg.latest_for("orch_0001").unwrap().id, second.id);
    }

    #[test]
    fn registry_caps_the_number_of_active_jobs() {
        let reg = JobRegistry::default();
        for i in 0..MAX_ACTIVE_JOBS {
            reg.start(&format!("orch_{i:04}"), 25, 2)
                .expect("under the cap");
        }
        match reg.start("orch_overflow", 25, 2) {
            Err(JobStartError::TooManyActive(n)) => assert_eq!(n, MAX_ACTIVE_JOBS),
            _ => panic!("expected the active-job cap to reject the overflow"),
        }
    }

    #[test]
    fn ready_task_ids_respects_dependencies_and_the_limit() {
        // brief 0 pending (ready), brief 1 depends on 0 (not ready), brief 2 pending
        // independent (ready).
        let orch = orchestration_with(vec![
            step("task_0", OrchestrationRole::Research, StepOutcome::Pending, vec![]),
            step("task_1", OrchestrationRole::Implementation, StepOutcome::Pending, vec![0]),
            step("task_2", OrchestrationRole::Research, StepOutcome::Pending, vec![]),
        ]);
        let ready = ready_task_ids(&orch, 4);
        assert_eq!(ready, vec!["task_0".to_string(), "task_2".to_string()]);
        // The limit caps how many are returned, in index order.
        assert_eq!(ready_task_ids(&orch, 1), vec!["task_0".to_string()]);
        // Once the dependency completes, the dependent becomes ready too.
        let orch2 = orchestration_with(vec![
            step("task_0", OrchestrationRole::Research, StepOutcome::Completed, vec![]),
            step("task_1", OrchestrationRole::Implementation, StepOutcome::Pending, vec![0]),
        ]);
        assert_eq!(ready_task_ids(&orch2, 4), vec!["task_1".to_string()]);
    }

    #[test]
    fn job_steps_marks_in_flight_briefs_running_and_keeps_real_outcomes() {
        let orch = orchestration_with(vec![
            step("task_0", OrchestrationRole::Research, StepOutcome::Completed, vec![]),
            step("task_1", OrchestrationRole::Implementation, StepOutcome::Pending, vec![]),
            step("task_2", OrchestrationRole::Testing, StepOutcome::Pending, vec![]),
        ]);
        let running = vec!["task_1".to_string()];
        let snap = job_steps(&orch, &running);
        assert_eq!(snap[0].outcome, "completed"); // real outcome preserved
        assert_eq!(snap[1].outcome, "running"); // about-to-run brief is marked running
        assert_eq!(snap[2].outcome, "pending"); // not in the running set: untouched
    }

    #[test]
    fn job_aggregate_sums_counts_and_takes_current_truth_from_the_last_round() {
        let oid = OrchestrationId::new("orch_0001");
        let mut agg = JobAggregate::default();
        let round1 = OrchestrationBatchResult {
            orchestration_id: oid.clone(),
            ran: 2,
            completed: 2,
            failed: 0,
            blocked: 0,
            pending: 2,
            concurrency: 2,
            rounds: 1,
            waiting: 1,
            dependency_blocked: 0,
            skipped_reasons: vec![],
            per_agent: vec!["round 1 prime: task_0 completed".to_string()],
            summary: "r1".to_string(),
            next_action: "continue".to_string(),
            status: OrchestrationStatus::Running,
        };
        let mut round2 = round1.clone();
        round2.ran = 2;
        round2.completed = 1;
        round2.failed = 1;
        round2.rounds = 1;
        round2.pending = 0;
        round2.waiting = 0;
        round2.status = OrchestrationStatus::NeedsAttention;
        round2.next_action = "fix the failed brief".to_string();
        round2.per_agent = vec!["round 2 prime: task_2 failed".to_string()];
        agg.merge(&round1);
        agg.merge(&round2);
        let result = agg.into_result(&oid, 2);
        // Counts are summed across rounds.
        assert_eq!(result.ran, 4);
        assert_eq!(result.completed, 3);
        assert_eq!(result.failed, 1);
        assert_eq!(result.rounds, 2);
        assert_eq!(result.per_agent.len(), 2);
        // Current truth (status / pending / next action) comes from the last round.
        assert_eq!(result.status, OrchestrationStatus::NeedsAttention);
        assert_eq!(result.pending, 0);
        assert_eq!(result.next_action, "fix the failed brief");
    }

    #[test]
    fn job_aggregate_with_no_rounds_reports_a_completed_empty_result() {
        let oid = OrchestrationId::new("orch_0001");
        let agg = JobAggregate::default();
        let result = agg.into_result(&oid, 2);
        assert_eq!(result.ran, 0);
        assert_eq!(result.rounds, 0);
        assert_eq!(result.status, OrchestrationStatus::Completed);
    }

    // --- Cancellation: registry state machine -----------------------------

    #[test]
    fn registry_request_cancel_sets_the_flag_on_an_active_job() {
        let reg = JobRegistry::default();
        let job = reg.start("orch_0001", 25, 2).unwrap();
        assert!(!reg.is_cancel_requested(&job.id), "fresh job has no cancel");
        match reg.request_cancel(&job.id) {
            CancelOutcome::Requested(j) => assert!(j.cancel_requested),
            other => panic!("expected Requested, got {other:?}"),
        }
        assert!(reg.is_cancel_requested(&job.id), "flag is now set");
        // Still active (worker not running here), so a repeat request is accepted
        // idempotently rather than erroring.
        assert!(matches!(reg.request_cancel(&job.id), CancelOutcome::Requested(_)));
    }

    #[test]
    fn registry_request_cancel_is_idempotent_once_canceled() {
        let reg = JobRegistry::default();
        let job = reg.start("orch_0001", 25, 2).unwrap();
        reg.update(&job.id, |j| j.state = JobState::Canceled);
        assert!(matches!(
            reg.request_cancel(&job.id),
            CancelOutcome::AlreadyCanceled(_)
        ));
    }

    #[test]
    fn registry_refuses_to_cancel_a_finished_job() {
        let reg = JobRegistry::default();
        let done = reg.start("orch_0001", 25, 2).unwrap();
        reg.update(&done.id, |j| j.state = JobState::Completed);
        assert!(matches!(
            reg.request_cancel(&done.id),
            CancelOutcome::AlreadyTerminal(_)
        ));
        let failed = reg.start("orch_0002", 25, 2).unwrap();
        reg.update(&failed.id, |j| j.state = JobState::Failed);
        assert!(matches!(
            reg.request_cancel(&failed.id),
            CancelOutcome::AlreadyTerminal(_)
        ));
    }

    #[test]
    fn registry_request_cancel_unknown_job_is_unknown() {
        let reg = JobRegistry::default();
        assert!(matches!(reg.request_cancel("nope"), CancelOutcome::Unknown));
        assert!(!reg.is_cancel_requested("nope"));
    }

    #[test]
    fn a_canceled_job_does_not_block_a_fresh_run() {
        // Cancellation is resumable: a canceled job is terminal, so it no longer
        // counts as the orchestration's active job and a new run can start.
        let reg = JobRegistry::default();
        let first = reg.start("orch_0001", 25, 2).unwrap();
        reg.update(&first.id, |j| j.state = JobState::Canceled);
        let second = reg.start("orch_0001", 25, 2).expect("a new job after cancel");
        assert_ne!(second.id, first.id);
    }

    // --- Cancellation: cooperative worker behavior ------------------------

    /// Build a real AppState backed by a temp SQLite store, seeded with a
    /// multi-brief orchestration whose briefs all fall back to Prime (the local
    /// echo adapter), so a round runs inline without spawning any CLI process.
    fn app_state_with_prime_orchestration() -> (AppState, tempfile::TempDir, OrchestrationId) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("local.db");
        let mut store = SqliteStore::open(&db_path).expect("open store");
        let mut kernel = store.load().expect("load");
        let ctx = crate::ensure_bootstrapped(&mut kernel).expect("bootstrap");
        // Research + documentation: two clauses, both Prime fallback (no specialist).
        let orch = kernel
            .prime_orchestrate(&ctx, "research the rust options and document the findings")
            .expect("plan orchestration");
        assert!(orch.steps.len() >= 2, "needs a multi-brief plan");
        store.save(&kernel).expect("save");
        let oid = orch.id.clone();
        let state = AppState {
            db_path,
            plugins_root: dir.path().join("plugins"),
            uploads_root: dir.path().join("uploads"),
            dashboard_dir: None,
            ai_config_path: dir.path().join("ai.json"),
            dashboard_auth: relux_kernel::DashboardAuth::from_admin_path(
                &dir.path().join("dashboard-admin.json"),
            ),
            agent_tokens: relux_kernel::AgentTokenStore::from_path(
                &dir.path().join("dashboard-agent-tokens.json"),
            ),
            auth_disabled: false,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
            live_run_logs: relux_kernel::LiveRunLogs::new(),
            run_cancellations: relux_kernel::RunCancellations::new(),
        };
        (state, dir, oid)
    }

    #[test]
    fn worker_cancel_requested_up_front_runs_no_round_and_marks_canceled() {
        let (state, _dir, oid) = app_state_with_prime_orchestration();
        let job = state.jobs.start(oid.as_str(), 25, 2).unwrap();
        // Request cancellation BEFORE the worker runs its first round.
        assert!(matches!(
            state.jobs.request_cancel(&job.id),
            CancelOutcome::Requested(_)
        ));

        drive_orchestration_job(state.clone(), job.id.clone(), oid.clone(), 25, 2);

        let done = state.jobs.get(&job.id).expect("job survives");
        assert_eq!(done.state, JobState::Canceled, "honored the cancel");
        assert_eq!(done.current_round, 0, "no round ran");
        assert_eq!(done.ran, 0, "no brief ran");
        assert!(done.completed_at_ms.is_some(), "job finished");
        // The durable record proves nothing executed: every brief is still pending.
        let orch = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        assert!(
            orch.steps.iter().all(|s| s.outcome == StepOutcome::Pending),
            "cancellation must not run any brief"
        );
    }

    #[test]
    fn worker_without_cancel_runs_the_briefs_and_completes() {
        // Positive control: the SAME seeded orchestration runs to completion when no
        // cancel is requested, proving the cancel test above is not vacuous.
        let (state, _dir, oid) = app_state_with_prime_orchestration();
        let job = state.jobs.start(oid.as_str(), 25, 2).unwrap();

        drive_orchestration_job(state.clone(), job.id.clone(), oid.clone(), 25, 2);

        let done = state.jobs.get(&job.id).expect("job survives");
        assert_eq!(done.state, JobState::Completed, "ran to completion");
        assert!(done.ran >= 1, "at least one brief ran");
        let orch = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        assert!(
            orch.steps.iter().any(|s| s.outcome != StepOutcome::Pending),
            "without cancel the briefs actually run"
        );
    }

    #[test]
    fn a_second_job_resumes_only_pending_briefs_and_preserves_completed_runs() {
        // Resume-after-cancel invariant (RELUX_MASTER_PLAN Sec 15). When a job leaves
        // an orchestration partially done — some briefs completed, the rest pending —
        // a fresh job must run ONLY the still-pending briefs. It must never re-run an
        // already-completed brief: each completed brief keeps the exact run id and
        // round it earned in the first job, while the resumed briefs earn brand-new
        // run ids of their own.
        let (state, _dir, oid) = app_state_with_prime_orchestration();

        // First job, budgeted to exactly ONE brief. The round runs one ready brief,
        // then the per-job budget is spent and the worker stops — leaving the rest
        // pending, the same partial shape a mid-flight cancel leaves behind.
        let job1 = state.jobs.start(oid.as_str(), 1, 2).unwrap();
        drive_orchestration_job(state.clone(), job1.id.clone(), oid.clone(), 1, 2);
        let done1 = state.jobs.get(&job1.id).expect("job1 survives");
        assert_eq!(done1.ran, 1, "first job ran exactly one brief (budget=1)");

        let after1 = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        let completed1: Vec<_> = after1
            .steps
            .iter()
            .filter(|s| s.outcome == StepOutcome::Completed)
            .collect();
        let pending1: Vec<_> = after1
            .steps
            .iter()
            .filter(|s| s.outcome == StepOutcome::Pending)
            .collect();
        assert_eq!(completed1.len(), 1, "exactly one brief completed");
        assert!(!pending1.is_empty(), "downstream briefs still pending to resume");
        // Snapshot the completed brief's earned identity so we can prove the resume
        // never touches it.
        let done_task = completed1[0].task_id.clone();
        let done_run = completed1[0].run_id.clone().expect("completed brief has a run id");
        let done_round = completed1[0].round;
        assert!(done_round.is_some(), "completed brief recorded its round");
        let pending_tasks: Vec<_> = pending1.iter().map(|s| s.task_id.clone()).collect();

        // Fresh, non-blocking job on the SAME orchestration. The first job is terminal
        // (Completed), so this is accepted rather than rejected as a duplicate.
        let job2 = state
            .jobs
            .start(oid.as_str(), 25, 2)
            .expect("a fresh job resumes the partially-done orchestration");
        assert_ne!(job2.id, job1.id);
        drive_orchestration_job(state.clone(), job2.id.clone(), oid.clone(), 25, 2);
        let done2 = state.jobs.get(&job2.id).expect("job2 survives");
        assert_eq!(done2.state, JobState::Completed, "resumed job completes");
        assert_eq!(
            done2.ran as usize,
            pending_tasks.len(),
            "the resumed job ran ONLY the previously-pending briefs, never re-running the completed one"
        );

        let after2 = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        // The already-completed brief is byte-for-byte untouched: same run id, same round.
        let done_after = after2
            .steps
            .iter()
            .find(|s| s.task_id == done_task)
            .expect("completed brief still present");
        assert_eq!(done_after.outcome, StepOutcome::Completed);
        assert_eq!(
            done_after.run_id, Some(done_run.clone()),
            "the completed brief kept its original run id (was not re-run)"
        );
        assert_eq!(
            done_after.round, done_round,
            "the completed brief kept its original round"
        );
        // Every resumed brief now completed, each with a brand-new run id distinct from
        // the first job's.
        for tid in &pending_tasks {
            let s = after2
                .steps
                .iter()
                .find(|s| &s.task_id == tid)
                .expect("resumed brief present");
            assert_eq!(s.outcome, StepOutcome::Completed, "resumed brief completed");
            let rid = s.run_id.clone().expect("resumed brief earned a run id");
            assert_ne!(rid, done_run, "resumed brief earned a NEW run id");
            assert!(s.round.is_some(), "resumed brief recorded its round");
        }
        assert!(
            after2.steps.iter().all(|s| s.outcome == StepOutcome::Completed),
            "the orchestration is fully completed after resume"
        );
    }

    // --- Restart honesty: reconstruct job status from the durable record -------

    /// A fresh [`AppState`] over the SAME on-disk store, with a brand-new (empty)
    /// in-memory [`JobRegistry`] — the exact shape a server restart produces: the
    /// durable orchestration record survives, every live job is gone.
    fn restarted_state_over(dir: &tempfile::TempDir) -> AppState {
        AppState {
            db_path: dir.path().join("local.db"),
            plugins_root: dir.path().join("plugins"),
            uploads_root: dir.path().join("uploads"),
            dashboard_dir: None,
            ai_config_path: dir.path().join("ai.json"),
            dashboard_auth: relux_kernel::DashboardAuth::from_admin_path(
                &dir.path().join("dashboard-admin.json"),
            ),
            agent_tokens: relux_kernel::AgentTokenStore::from_path(
                &dir.path().join("dashboard-agent-tokens.json"),
            ),
            auth_disabled: false,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
            live_run_logs: relux_kernel::LiveRunLogs::new(),
            run_cancellations: relux_kernel::RunCancellations::new(),
        }
    }

    #[test]
    fn reconstruct_returns_none_when_no_brief_ever_ran() {
        // A planned orchestration that never ran a brief has no job history, so
        // reconstruction invents nothing — the poll still honestly 404s and the
        // dashboard falls back to the planned record.
        let (state, _dir, oid) = app_state_with_prime_orchestration();
        let orch = locked_read(&state, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        assert!(
            reconstruct_job_from_record(&orch).is_none(),
            "no run id on any step => no reconstructed job"
        );
    }

    #[test]
    fn reconstruct_reports_interrupted_after_partial_run_across_restart() {
        // Restart-honesty invariant (RELUX_MASTER_PLAN Sec 15). A job budgeted to one
        // brief leaves the orchestration partially done; after a "restart" (fresh
        // registry over the same store) the live job is gone, but a poll by
        // orchestration id reconstructs an honest `interrupted` status from the
        // durable record — never a misleading "never existed" 404.
        let (state, dir, oid) = app_state_with_prime_orchestration();
        let job1 = state.jobs.start(oid.as_str(), 1, 2).unwrap();
        drive_orchestration_job(state.clone(), job1.id.clone(), oid.clone(), 1, 2);

        // Simulate the restart: a new state with an empty registry over the same db.
        let restarted = restarted_state_over(&dir);
        assert!(
            restarted.jobs.latest_for(oid.as_str()).is_none(),
            "the in-memory job is gone after a restart"
        );

        let orch = locked_read(&restarted, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        let rebuilt = reconstruct_job_from_record(&orch).expect("a prior run => a reconstructed job");
        assert_eq!(rebuilt.state, JobState::Interrupted, "pending briefs remain");
        assert_eq!(rebuilt.ran, 1, "exactly one brief had run before the restart");
        assert_eq!(rebuilt.completed, 1, "that brief completed durably");
        assert!(rebuilt.current_round >= 1, "a round was recorded");
        assert!(
            rebuilt.steps.iter().any(|s| s.outcome == "pending"),
            "the reconstructed view shows the still-pending briefs to resume"
        );
        assert!(
            rebuilt.id.starts_with("durable:"),
            "the reconstructed id is clearly synthetic, not a live job id"
        );
        assert!(
            rebuilt.last_event.as_deref().unwrap_or("").contains("pending"),
            "the message honestly explains the pending work to resume"
        );
    }

    #[test]
    fn reconstruct_reports_completed_when_all_briefs_terminal_across_restart() {
        // The positive control: a fully-run orchestration reconstructs as `completed`
        // (not `interrupted`) after a restart, since no brief is left pending.
        let (state, dir, oid) = app_state_with_prime_orchestration();
        let job = state.jobs.start(oid.as_str(), 25, 2).unwrap();
        drive_orchestration_job(state.clone(), job.id.clone(), oid.clone(), 25, 2);

        let restarted = restarted_state_over(&dir);
        let orch = locked_read(&restarted, |k| Ok(k.orchestration(&oid).cloned()))
            .unwrap()
            .unwrap();
        let rebuilt = reconstruct_job_from_record(&orch).expect("a run happened");
        assert_eq!(rebuilt.state, JobState::Completed, "no brief left pending");
        assert!(
            rebuilt.steps.iter().all(|s| s.outcome != "pending"),
            "every brief reached a terminal outcome"
        );
        assert!(rebuilt.ran >= 1, "at least one brief ran");
    }

    // --- Prime CLI brain reply shaping (no raw JSON envelope) ----------------
    //
    // Regression guard for the reported bug: Prime with the Claude CLI selected
    // showed the entire `--output-format json` envelope (`{ "type":"result", ...
    // "result":"Hey...", "duration_ms":.., "session_id":.., "usage":{..} }`) as
    // the chat answer. These pin that the bubble shows only the human text.

    #[test]
    fn claude_cli_brain_shows_only_human_text_not_raw_envelope() {
        let stdout = r#"{
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "Hey! How can I help you today?",
            "duration_ms": 1234,
            "session_id": "abc-123-session",
            "total_cost_usd": 0.0021,
            "num_turns": 1,
            "usage": { "input_tokens": 12, "output_tokens": 8 }
        }"#;
        let reply = shape_cli_brain_reply(stdout, false, relux_core::AdapterKind::ClaudeCli, "Claude CLI")
            .expect("a success envelope yields a human reply");
        assert_eq!(reply, "Hey! How can I help you today?");
        // None of the envelope scaffolding/metadata leaks into the chat bubble.
        assert!(!reply.contains('{'), "no JSON braces: {reply}");
        assert!(!reply.contains("duration_ms"), "no duration field: {reply}");
        assert!(!reply.contains("session_id"), "no session field: {reply}");
        assert!(!reply.contains("usage"), "no usage field: {reply}");
        assert!(!reply.contains("total_cost_usd"), "no cost field: {reply}");
        assert!(!reply.contains("\"type\""), "no type field: {reply}");
    }

    #[test]
    fn claude_cli_brain_error_envelope_falls_back_with_note_not_json() {
        let stdout = r#"{"type":"result","subtype":"error","is_error":true,"result":"hit a rate limit","session_id":"x"}"#;
        let err = shape_cli_brain_reply(stdout, false, relux_core::AdapterKind::ClaudeCli, "Claude CLI")
            .expect_err("an error envelope must not be shown as a confident answer");
        assert!(err.contains("Claude CLI reported an error"), "note: {err}");
        assert!(err.contains("hit a rate limit"), "surfaces the error detail: {err}");
        assert!(!err.contains('{'), "never dumps the raw JSON: {err}");
        assert!(!err.contains("session_id"), "no metadata leak: {err}");
    }

    #[test]
    fn codex_cli_brain_plain_text_passes_through_unchanged() {
        // Codex `exec` / text-mode brains emit prose; it must round-trip verbatim.
        let stdout = "Sure — here is a quick summary of the repo.";
        let reply = shape_cli_brain_reply(stdout, false, relux_core::AdapterKind::CodexCli, "Codex CLI")
            .expect("plain prose is a valid reply");
        assert_eq!(reply, stdout);
    }

    #[test]
    fn cli_brain_empty_result_falls_back() {
        // An envelope whose `result` is blank is not a usable answer.
        let stdout = r#"{"type":"result","is_error":false,"result":"   "}"#;
        let err = shape_cli_brain_reply(stdout, false, relux_core::AdapterKind::ClaudeCli, "Claude CLI")
            .expect_err("a blank result yields a fallback");
        assert!(err.contains("empty answer"), "note: {err}");
    }

    #[test]
    fn cli_brain_truncation_marker_only_on_plain_text() {
        // Plain text that was byte-capped gets the marker.
        let plain = shape_cli_brain_reply("a long answer", true, relux_core::AdapterKind::CodexCli, "Codex CLI")
            .expect("plain reply");
        assert!(plain.ends_with("[output truncated]"), "plain marker: {plain}");
        // A cleanly-parsed structured envelope carries its full `result`, so no
        // misleading truncation marker is appended.
        let envelope = r#"{"type":"result","is_error":false,"result":"complete answer"}"#;
        let structured = shape_cli_brain_reply(envelope, true, relux_core::AdapterKind::ClaudeCli, "Claude CLI")
            .expect("structured reply");
        assert_eq!(structured, "complete answer");
        assert!(!structured.contains("[output truncated]"), "no marker: {structured}");
    }

    #[test]
    fn brain_chat_envelope_with_proposed_changes_shows_only_reply_no_json() {
        // A chat-turn envelope that ALSO declares proposed_changes must still show
        // only the human `result` text in the bubble — never the structured change
        // payload (path/content/baseline) and never the raw JSON envelope.
        let base = relux_core::proposed_change::sha256_hex(b"old\n");
        let stdout = format!(
            r#"{{
                "type": "result",
                "is_error": false,
                "result": "I would rewrite src/lib.rs to add the helper.",
                "proposed_changes": [
                    {{ "path": "src/lib.rs", "content": "new\n", "baseline_sha256": "{base}" }}
                ]
            }}"#
        );
        let reply = shape_cli_brain_reply(&stdout, false, relux_core::AdapterKind::ClaudeCli, "Claude CLI")
            .expect("a success envelope yields a human reply");
        assert_eq!(reply, "I would rewrite src/lib.rs to add the helper.");
        // The applyable change payload never leaks into the chat bubble.
        assert!(!reply.contains('{'), "no JSON braces: {reply}");
        assert!(!reply.contains("proposed_changes"), "no field name: {reply}");
        assert!(!reply.contains("baseline_sha256"), "no baseline field: {reply}");
        assert!(!reply.contains("new\n"), "no raw new_content: {reply}");
        assert!(!reply.contains(base.as_str()), "no baseline hash: {reply}");
    }

    #[test]
    fn brain_chat_envelope_with_proposed_changes_surfaces_honest_advisory() {
        // The proposed change is NOT silently dropped: the operator gets a bounded,
        // secret-free note that it was proposed and how to get a real review/apply
        // run (the documented assigned-run path). No run/state is created here.
        let base = relux_core::proposed_change::sha256_hex(b"old\n");
        let stdout = format!(
            r#"{{"type":"result","is_error":false,"result":"sure","proposed_changes":[{{"path":"src/lib.rs","content":"new\n","baseline_sha256":"{base}"}}]}}"#
        );
        let note = brain_envelope_advisory(&stdout, relux_core::AdapterKind::ClaudeCli, "Claude CLI")
            .expect("an envelope with proposed changes yields an advisory note");
        assert!(note.contains("Claude CLI proposed 1 file change"), "counts honestly: {note}");
        assert!(note.contains("action-free"), "explains why not captured: {note}");
        assert!(note.contains("create a task assigned to this adapter"), "points at the documented path: {note}");
        // The note carries no raw JSON, no on-disk content, and no baseline hash.
        assert!(!note.contains('{'), "no JSON in the note: {note}");
        assert!(!note.contains(base.as_str()), "no baseline hash in the note: {note}");
        assert!(!note.contains("new\n"), "no proposed content in the note: {note}");
    }

    #[test]
    fn brain_chat_greeting_envelope_has_no_advisory() {
        // A plain greeting/chat turn (no proposed_changes) stays artifact-free: no
        // advisory note, so casual chat never nags about review/apply.
        let stdout = r#"{"type":"result","is_error":false,"result":"Hey! How can I help?"}"#;
        assert!(
            brain_envelope_advisory(stdout, relux_core::AdapterKind::ClaudeCli, "Claude CLI").is_none(),
            "a greeting must not produce an advisory"
        );
        // Plain prose (Codex/text mode) likewise has nothing to advise about.
        assert!(
            brain_envelope_advisory("just a friendly reply", relux_core::AdapterKind::CodexCli, "Codex CLI").is_none(),
            "plain prose must not produce an advisory"
        );
    }

    // --- CLI-brain proposal polish (shape seam) ----------------------------

    fn polish_proposal_fixture() -> relux_core::PrimeProposal {
        relux_core::PrimeProposal {
            goal: "ship the beta".to_string(),
            multi_step: true,
            steps: vec![
                relux_core::PrimeProposalStep {
                    index: 1,
                    title: "research the options".to_string(),
                    role: "research".to_string(),
                    agent: "research-agent".to_string(),
                },
                relux_core::PrimeProposalStep {
                    index: 2,
                    title: "build a prototype".to_string(),
                    role: "implementation".to_string(),
                    agent: "prime".to_string(),
                },
            ],
            agents: vec!["research-agent".to_string(), "prime".to_string()],
            polish: None,
        }
    }

    #[test]
    fn cli_polish_accepts_valid_json_lifted_from_a_result_envelope() {
        // The Claude CLI wraps the polish JSON inside its `--output-format json`
        // result envelope; parse_adapter_result lifts the inner JSON string, then the
        // validation chokepoint accepts an exact 1:1 step match.
        let inner = r#"{\"summary\":\"A clear two-stage path.\",\"steps\":[{\"index\":1,\"title\":\"Survey the options\"},{\"index\":2,\"title\":\"Build a prototype\"}],\"questions\":[\"Which platform first?\"],\"risks\":[\"Scope creep.\"]}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        let p = polish_proposal_fixture();
        let overlay = shape_cli_brain_polish(&stdout, relux_core::AdapterKind::ClaudeCli, "Claude CLI", &p)
            .expect("a valid envelope yields a polish overlay");
        assert_eq!(overlay.model.as_deref(), Some("Claude CLI"));
        assert_eq!(overlay.step_titles.len(), 2);
        assert_eq!(overlay.step_titles[0].title, "Survey the options");
        assert_eq!(overlay.summary.as_deref(), Some("A clear two-stage path."));
    }

    #[test]
    fn cli_polish_accepts_plain_json_from_codex_text_mode() {
        // Codex `exec` / text mode emits the JSON object as raw prose (no envelope).
        let stdout = r#"{"summary":"tidy plan","steps":[{"index":1,"title":"Look into options"},{"index":2,"title":"Prototype it"}]}"#;
        let p = polish_proposal_fixture();
        let overlay = shape_cli_brain_polish(stdout, relux_core::AdapterKind::CodexCli, "Codex CLI", &p)
            .expect("raw JSON prose still validates");
        assert_eq!(overlay.model.as_deref(), Some("Codex CLI"));
        assert_eq!(overlay.step_titles.len(), 2);
    }

    #[test]
    fn cli_polish_ignores_prose_with_no_json() {
        // A chatty CLI that ignored the JSON instruction -> no overlay, the
        // deterministic preview stands (no user-facing failure).
        let stdout = r#"{"type":"result","is_error":false,"result":"This plan looks solid to me!"}"#;
        let p = polish_proposal_fixture();
        assert!(
            shape_cli_brain_polish(stdout, relux_core::AdapterKind::ClaudeCli, "Claude CLI", &p).is_none(),
            "prose with no JSON object must not polish the card"
        );
    }

    // --- Brain-mediated intent classification: the no-leak parse boundary ------
    //
    // These pin `parse_cli_intent` — the exact composition `classify_intent_via_cli`
    // runs after the spawn — WITHOUT spawning a real CLI (the task's honest bar:
    // test the parser/adapter boundary, never let `echo` pretend to be intelligence).
    // The raw `--output-format json` envelope must never reach the validated
    // proposal, and only an allowlisted intent label survives.

    #[test]
    fn cli_intent_classification_lifted_from_a_result_envelope() {
        // The Claude CLI wraps the classifier's JSON inside its result envelope;
        // parse_adapter_result lifts the inner string, then parse_intent_proposal
        // validates the label against the PrimeIntent allowlist.
        let inner = r#"{\"intent\":\"task_creation\",\"confidence\":0.92,\"rationale\":\"explicit ask to fix a bug\"}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc","duration_ms":12}}"#);
        let p = parse_cli_intent(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields a validated intent proposal");
        assert_eq!(p.intent, relux_core::PrimeIntent::TaskCreation);
        assert_eq!(p.confidence, 0.92);
        // No envelope scaffolding leaks into the validated proposal.
        assert!(!p.rationale.contains("session_id"), "no metadata leak: {}", p.rationale);
        assert!(!p.rationale.contains("duration_ms"), "no metadata leak: {}", p.rationale);
        assert!(!p.rationale.contains("\"type\""), "no envelope type leak: {}", p.rationale);
    }

    #[test]
    fn cli_intent_plain_json_from_codex_text_mode() {
        // Codex `exec` / text mode emits the JSON object as raw prose (no envelope).
        let stdout = r#"{"intent":"brainstorming","confidence":0.7,"rationale":"musing"}"#;
        let p = parse_cli_intent(stdout, relux_core::AdapterKind::CodexCli)
            .expect("raw JSON prose still validates");
        assert_eq!(p.intent, relux_core::PrimeIntent::Brainstorming);
    }

    #[test]
    fn cli_intent_error_envelope_yields_no_classification() {
        // An envelope reporting an error is dropped — the turn falls back to the
        // deterministic classifier, and the raw error text never leaks.
        let stdout = r#"{"type":"result","is_error":true,"result":"rate limited","session_id":"x"}"#;
        assert!(
            parse_cli_intent(stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an error envelope must yield no brain classification"
        );
    }

    #[test]
    fn cli_intent_prose_with_no_json_yields_no_classification() {
        // A chatty CLI that ignored the JSON instruction -> no proposal, the
        // deterministic classifier decides.
        let stdout = r#"{"type":"result","is_error":false,"result":"I think you want to create a task."}"#;
        assert!(
            parse_cli_intent(stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "prose with no JSON object must not produce a brain intent"
        );
    }

    #[test]
    fn cli_intent_off_allowlist_label_yields_no_classification() {
        // A hallucinated intent the enum does not define is refused at the seam.
        let inner = r#"{\"intent\":\"delete_everything\",\"confidence\":1.0}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_intent(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an off-allowlist label must be rejected, never acted on"
        );
    }

    // --- Brain-assisted task slot extraction: the no-leak parse boundary -------
    //
    // These pin `parse_cli_task_slots` — the composition `extract_task_slots_via_cli`
    // runs after the spawn — WITHOUT spawning a real CLI. The raw `--output-format
    // json` envelope must never reach the validated slots, an unsupported field
    // fails the whole proposal closed, and an error/prose envelope yields nothing.

    #[test]
    fn cli_slots_lifted_from_a_result_envelope() {
        let inner = r#"{\"title\":\"Fix the login redirect bug\",\"assignee\":\"code-agent\",\"priority\":8,\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc","duration_ms":12}}"#);
        let s = parse_cli_task_slots(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields validated slots");
        assert_eq!(s.title, "Fix the login redirect bug");
        assert_eq!(s.assignee.as_deref(), Some("code-agent"));
        assert_eq!(s.priority, Some(8));
        // No envelope scaffolding leaks into the validated slots.
        assert!(!s.title.contains("session_id") && !s.title.contains("\"type\""));
    }

    #[test]
    fn cli_slots_plain_json_from_codex_text_mode() {
        let stdout = r#"{"title":"Summarize the README","confidence":0.8}"#;
        let s = parse_cli_task_slots(stdout, relux_core::AdapterKind::CodexCli)
            .expect("raw JSON prose still validates");
        assert_eq!(s.title, "Summarize the README");
        assert!(s.details.is_none());
    }

    #[test]
    fn cli_slots_error_envelope_yields_nothing() {
        let stdout = r#"{"type":"result","is_error":true,"result":"rate limited","session_id":"x"}"#;
        assert!(
            parse_cli_task_slots(stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an error envelope must yield no slots"
        );
    }

    #[test]
    fn cli_slots_prose_with_no_json_yields_nothing() {
        let stdout = r#"{"type":"result","is_error":false,"result":"Sure, I will make that task for you."}"#;
        assert!(
            parse_cli_task_slots(stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "prose with no JSON object must not produce slots"
        );
    }

    #[test]
    fn cli_slots_unsupported_field_fails_closed() {
        // A brain that tries to smuggle an executable key fails the whole proposal
        // closed at the seam, so the create falls back to deterministic slots.
        let inner = r#"{\"title\":\"x\",\"run_tool\":\"relux-tools-shell\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_task_slots(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an unsupported field must fail closed"
        );
    }

    // --- Brain-assisted ASSIGNMENT slots: the no-leak parse boundary ----------
    //
    // These pin `parse_cli_assign_slots` WITHOUT spawning a real CLI: the raw envelope
    // must never reach the validated slots, an error/prose envelope yields nothing, and
    // an unsupported field fails the whole proposal closed.

    #[test]
    fn cli_assign_slots_lifted_from_a_result_envelope() {
        let inner = r#"{\"task_id\":\"task_0001\",\"agent_id\":\"researcher\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let s = parse_cli_assign_slots(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields validated assign slots");
        assert_eq!(s.task_id.as_deref(), Some("task_0001"));
        assert_eq!(s.agent_id.as_deref(), Some("researcher"));
    }

    #[test]
    fn cli_assign_slots_error_envelope_and_prose_yield_nothing() {
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(parse_cli_assign_slots(err, relux_core::AdapterKind::ClaudeCli).is_none());
        let prose = r#"{"type":"result","is_error":false,"result":"Sure, I'll assign that."}"#;
        assert!(parse_cli_assign_slots(prose, relux_core::AdapterKind::ClaudeCli).is_none());
    }

    #[test]
    fn cli_assign_slots_unsupported_field_fails_closed() {
        let inner = r#"{\"task_id\":\"task_0001\",\"agent_id\":\"researcher\",\"run_now\":true,\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_assign_slots(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an unsupported field must fail closed"
        );
    }

    // --- Brain-assisted by-id UPDATE slots: the no-leak parse boundary --------
    //
    // These pin `parse_cli_update_slots` WITHOUT spawning a real CLI: a valid envelope
    // yields validated update slots, an error/prose envelope yields nothing, an
    // unsupported field fails closed, and a non-settable status is dropped (not fatal).

    #[test]
    fn cli_update_slots_lifted_from_a_result_envelope() {
        let inner = r#"{\"task_id\":\"task_0001\",\"priority\":8,\"status\":\"blocked\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let s = parse_cli_update_slots(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields validated update slots");
        assert_eq!(s.task_id.as_deref(), Some("task_0001"));
        assert_eq!(s.priority, Some(8));
        assert_eq!(s.status, Some(relux_core::TaskStatus::Blocked));
    }

    #[test]
    fn cli_update_slots_error_envelope_and_prose_yield_nothing() {
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(parse_cli_update_slots(err, relux_core::AdapterKind::ClaudeCli).is_none());
        let prose = r#"{"type":"result","is_error":false,"result":"Sure, I'll change that."}"#;
        assert!(parse_cli_update_slots(prose, relux_core::AdapterKind::ClaudeCli).is_none());
    }

    #[test]
    fn cli_update_slots_unsupported_field_fails_closed() {
        let inner = r#"{\"task_id\":\"task_0001\",\"run_now\":true,\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_update_slots(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an unsupported field must fail closed"
        );
    }

    #[test]
    fn cli_update_slots_drops_a_non_settable_status() {
        // "completed" is not operator-settable — it is dropped, the rest survives.
        let inner = r#"{\"task_id\":\"task_0001\",\"status\":\"completed\",\"priority\":3,\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        let s = parse_cli_update_slots(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("the rest of the proposal still validates");
        assert!(s.status.is_none(), "a non-settable status is dropped");
        assert_eq!(s.priority, Some(3));
    }

    // --- Brain-assisted AGENT slots: the no-leak parse boundary ---------------

    #[test]
    fn cli_agent_slots_lifted_from_a_result_envelope() {
        let inner = r#"{\"name\":\"CI Watcher\",\"role\":\"Watches CI\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let s = parse_cli_agent_slots(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields validated agent slots");
        assert_eq!(s.name, "CI Watcher");
        assert_eq!(s.role.as_deref(), Some("Watches CI"));
        assert!(!s.name.contains("session_id") && !s.name.contains("\"type\""));
    }

    #[test]
    fn cli_agent_slots_error_envelope_and_unsupported_field_yield_nothing() {
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(parse_cli_agent_slots(err, relux_core::AdapterKind::ClaudeCli).is_none());
        // An unsupported field (a smuggled permission key) fails the whole proposal.
        let inner = r#"{\"name\":\"x\",\"permissions\":[\"tool:relux-tools-shell:exec\"],\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_agent_slots(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an unsupported field must fail closed"
        );
    }

    // --- Brain-assisted ADMIN slots: the no-leak parse boundary ---------------

    #[test]
    fn cli_plugin_ref_lifted_and_normalized_from_an_envelope() {
        let inner = r#"{\"plugin_id\":\"Relux-Tools-GitHub\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        let p = parse_cli_plugin_ref(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields a validated plugin ref");
        assert_eq!(p.plugin_id, "relux-tools-github");
    }

    #[test]
    fn cli_permission_slots_lifted_from_an_envelope_and_kind_fails_closed() {
        let inner = r#"{\"subject_kind\":\"agent\",\"subject_id\":\"code-agent\",\"permission\":\"tool:relux-tools-github:access\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        let s = parse_cli_permission_slots(&stdout, relux_core::AdapterKind::CodexCli)
            .expect("a valid envelope yields validated permission slots");
        assert_eq!(s.subject_id.as_deref(), Some("code-agent"));
        assert_eq!(s.permission.as_deref(), Some("tool:relux-tools-github:access"));
        // An off-allowlist subject_kind fails the whole proposal closed at the seam.
        let bad = r#"{\"subject_kind\":\"plugin\",\"subject_id\":\"relux-tools-github\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{bad}"}}"#);
        assert!(
            parse_cli_permission_slots(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an unsupported subject_kind must fail closed"
        );
    }

    // --- Brain-assisted CLARIFY / BRAINSTORM wording: the no-leak parse boundary ---

    #[test]
    fn cli_clarify_lifted_from_a_result_envelope() {
        let inner = r#"{\"text\":\"Which task should I update - task_42 or task_7?\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let text = parse_cli_clarify(
            &stdout,
            relux_core::AdapterKind::ClaudeCli,
            relux_kernel::ClarifyKind::Clarify,
            "Which task should I update, and what should change?",
        )
        .expect("a valid envelope yields validated wording");
        assert!(text.ends_with('?'));
        // No envelope scaffolding leaks into the wording.
        assert!(!text.contains("session_id") && !text.contains("\"type\""));
    }

    #[test]
    fn cli_clarify_error_envelope_and_non_question_yield_nothing() {
        // An error envelope is dropped.
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(parse_cli_clarify(
            err,
            relux_core::AdapterKind::ClaudeCli,
            relux_kernel::ClarifyKind::Clarify,
            "Which task?",
        )
        .is_none());
        // A clarify that is not exactly one question fails validation at the seam.
        let inner = r#"{\"text\":\"I will update the task now.\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_clarify(
                &stdout,
                relux_core::AdapterKind::ClaudeCli,
                relux_kernel::ClarifyKind::Clarify,
                "Which task?",
            )
            .is_none(),
            "a non-question clarify must yield nothing"
        );
    }

    #[test]
    fn cli_clarify_brainstorm_rejects_an_action_claim() {
        // A brainstorm reply that narrates a completed action is rejected at the seam,
        // so the user is never told something false even though the turn is action-free.
        let inner = r#"{\"text\":\"I created the task and started the run.\",\"confidence\":0.95}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_clarify(
                &stdout,
                relux_core::AdapterKind::CodexCli,
                relux_kernel::ClarifyKind::Brainstorm,
                "Good - let's think it through.",
            )
            .is_none(),
            "an action claim must fail closed"
        );
    }

    // --- After-action (post-execution) wording: the no-leak parse boundary ------
    //
    // These pin `parse_cli_after_action` WITHOUT spawning a real CLI. The raw
    // `--output-format json` envelope must never reach the validated wording, and the lifted text
    // is validated against the sanitized result envelope (no claim of unexecuted work, no invented
    // id, no "installed" on a still-pending proposal).

    fn executed_create_envelope() -> relux_kernel::ActionEnvelope {
        relux_kernel::ActionEnvelope {
            kind: relux_kernel::ActionResultKind::Executed,
            action_label: "created a task".to_string(),
            facts: relux_kernel::ActionFacts {
                task_created: true,
                ..Default::default()
            },
            ids: vec!["task_0001".to_string()],
            grounded_reply: "Created task_0001: Fix the login redirect.".to_string(),
        }
    }

    #[test]
    fn cli_after_action_lifted_from_a_result_envelope() {
        let env = executed_create_envelope();
        let inner = r#"{\"text\":\"Done - I created task_0001 to fix the login redirect.\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let text = parse_cli_after_action(&stdout, relux_core::AdapterKind::ClaudeCli, &env)
            .expect("a valid envelope yields validated wording");
        assert!(text.contains("task_0001"));
        // No envelope scaffolding leaks into the wording.
        assert!(!text.contains("session_id") && !text.contains("\"type\""));
    }

    #[test]
    fn cli_after_action_drops_error_envelope_contradiction_and_invented_id() {
        let env = executed_create_envelope();
        // An error envelope is dropped.
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(parse_cli_after_action(err, relux_core::AdapterKind::ClaudeCli, &env).is_none());
        // A claim of a run that did not start fails validation at the seam.
        let inner = r#"{\"text\":\"Created task_0001 and started the run.\",\"confidence\":0.95}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_after_action(&stdout, relux_core::AdapterKind::ClaudeCli, &env).is_none(),
            "a claim of unexecuted work must yield nothing"
        );
        // An invented id is rejected.
        let inner = r#"{\"text\":\"Created task_9999 for you.\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_after_action(&stdout, relux_core::AdapterKind::CodexCli, &env).is_none(),
            "an invented id must yield nothing"
        );
    }

    #[test]
    fn cli_after_action_proposal_rejects_installed_claim() {
        let env = relux_kernel::ActionEnvelope {
            kind: relux_kernel::ActionResultKind::Proposed,
            action_label: "proposed installing a plugin (awaiting your approval)".to_string(),
            facts: relux_kernel::ActionFacts::default(),
            ids: vec!["appr_0001".to_string()],
            grounded_reply: "Installing a plugin needs your approval (appr_0001).".to_string(),
        };
        // "installed" on a still-pending proposal fails closed.
        let inner = r#"{\"text\":\"The plugin is now installed.\",\"confidence\":0.95}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(parse_cli_after_action(&stdout, relux_core::AdapterKind::ClaudeCli, &env).is_none());
        // Proposal-language is accepted.
        let inner = r#"{\"text\":\"I've proposed the install; it needs your approval (appr_0001).\",\"confidence\":0.9}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(parse_cli_after_action(&stdout, relux_core::AdapterKind::ClaudeCli, &env).is_some());
    }

    // --- Unified decision: the no-leak parse boundary --------------------------
    //
    // These pin `parse_cli_decision` — the composition `decide_prime_via_cli` runs after the
    // spawn — WITHOUT spawning a real CLI. The raw `--output-format json` envelope must never
    // reach the validated decision, and each section must survive only through its existing
    // allowlist validator.

    #[test]
    fn cli_decision_lifted_from_a_result_envelope() {
        // The Claude CLI wraps the unified decision JSON inside its result envelope;
        // parse_adapter_result lifts the inner string, then parse_decision validates each
        // section. One envelope carries intent + task slots together.
        let inner = r#"{\"classification\":{\"intent\":\"task_creation\",\"confidence\":0.9},\"task\":{\"title\":\"Fix the login redirect bug\",\"priority\":8,\"confidence\":0.9}}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc","duration_ms":12}}"#);
        let d = parse_cli_decision(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields a validated decision");
        assert_eq!(
            d.classification.as_ref().unwrap().intent,
            relux_core::PrimeIntent::TaskCreation
        );
        let task = d.task.as_ref().unwrap();
        assert_eq!(task.title, "Fix the login redirect bug");
        assert_eq!(task.priority, Some(8));
        // No envelope scaffolding leaks into the validated sections.
        assert!(!task.title.contains("session_id") && !task.title.contains("\"type\""));
    }

    #[test]
    fn cli_decision_plain_json_from_codex_text_mode() {
        let stdout = r#"{"classification":{"intent":"brainstorming","confidence":0.7},"wording":{"text":"What outcome are you after?","confidence":0.8}}"#;
        let d = parse_cli_decision(stdout, relux_core::AdapterKind::CodexCli)
            .expect("raw JSON prose still validates");
        assert_eq!(
            d.classification.as_ref().unwrap().intent,
            relux_core::PrimeIntent::Brainstorming
        );
        assert!(d.wording.is_some());
    }

    #[test]
    fn cli_decision_error_envelope_and_prose_yield_nothing() {
        let err = r#"{"type":"result","is_error":true,"result":"rate limited","session_id":"x"}"#;
        assert!(
            parse_cli_decision(err, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an error envelope must yield no decision"
        );
        let prose = r#"{"type":"result","is_error":false,"result":"Sure, here is what I think."}"#;
        assert!(
            parse_cli_decision(prose, relux_core::AdapterKind::ClaudeCli).is_none(),
            "prose with no JSON object must not produce a decision"
        );
    }

    #[test]
    fn classify_cli_decision_distinguishes_malformed_from_provider_failure() {
        use relux_kernel::DecisionOutcome;
        // A valid envelope is a Decision.
        let ok = r#"{"type":"result","is_error":false,"result":"{\"classification\":{\"intent\":\"greeting\",\"confidence\":0.9}}"}"#;
        assert!(matches!(
            classify_cli_decision(ok, relux_core::AdapterKind::ClaudeCli),
            DecisionOutcome::Decision(_)
        ));
        // The brain answered but the envelope failed parse_decision (prose with no JSON object):
        // Malformed -> re-askable via the bounded self-correction loop, carrying the error string.
        let prose = r#"{"type":"result","is_error":false,"result":"Sure, here is what I think."}"#;
        assert!(matches!(
            classify_cli_decision(prose, relux_core::AdapterKind::ClaudeCli),
            DecisionOutcome::Malformed(_)
        ));
        // A smuggled un-modeled key also yields a Malformed (the whole envelope fails closed), so
        // the brain gets one corrective re-ask rather than a silent fallback.
        let bad_key = r#"{"type":"result","is_error":false,"result":"{\"classification\":{\"intent\":\"greeting\",\"confidence\":0.9},\"execute\":true}"}"#;
        assert!(matches!(
            classify_cli_decision(bad_key, relux_core::AdapterKind::ClaudeCli),
            DecisionOutcome::Malformed(_)
        ));
        // An error envelope is the provider's failure: ProviderError, never re-asked for format.
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(matches!(
            classify_cli_decision(err, relux_core::AdapterKind::ClaudeCli),
            DecisionOutcome::ProviderError
        ));
    }

    #[test]
    fn cli_decision_carries_reply_and_plan_polish_through_the_no_leak_seam() {
        // A conversational + plan turn answered in ONE envelope: a free-form reply and an
        // advisory plan-polish ride alongside the intent, all lifted out of the CLI result
        // envelope with no scaffolding leak. The reply/polish are carried raw here; the kernel
        // validates each later against the turn (validated_reply / validated_polish).
        let inner = r#"{\"classification\":{\"intent\":\"greeting\",\"confidence\":0.9},\"reply\":{\"text\":\"Hey - what can I help with?\",\"confidence\":0.9},\"plan_polish\":{\"summary\":\"Two phases.\"}}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let d = parse_cli_decision(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a reply + plan_polish envelope validates");
        assert!(d.reply.is_some());
        assert!(d.plan_polish.is_some());
        // The validated reply is honored and carries no envelope scaffolding.
        let reply = d.validated_reply("Hi.").expect("a confident, distinct reply is honored");
        assert_eq!(reply, "Hey - what can I help with?");
        assert!(!reply.contains("session_id") && !reply.contains("\"type\""));
    }

    #[test]
    fn cli_decision_carries_read_only_tool_requests_through_the_no_leak_seam() {
        // An inspection turn answered in ONE envelope: the brain requests read-only context tools
        // up front, lifted out of the CLI result envelope with no scaffolding leak and validated
        // against the read-only allowlist. A smuggled mutating request is dropped, never executed.
        let inner = r#"{\"classification\":{\"intent\":\"status_question\",\"confidence\":0.9},\"tool_requests\":[{\"tool\":\"get_task\",\"args\":{\"task_id\":\"task_0001\"}},{\"tool\":\"delete_task\"}]}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let d = parse_cli_decision(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a tool_requests envelope validates");
        assert_eq!(d.context_requests.len(), 1, "only the read-only request survives");
        assert_eq!(d.context_requests[0].tool, "get_task");
        assert_eq!(
            d.context_requests[0].args.get("task_id").unwrap().as_str(),
            Some("task_0001")
        );
        // No envelope scaffolding leaks into the validated request.
        assert!(!d.context_requests[0].tool.contains("session_id"));
    }

    #[test]
    fn cli_decision_carries_a_write_tool_request_through_the_no_leak_seam() {
        // An explicitly-commanded turn answered in ONE envelope: the brain requests a governed
        // WRITE tool, lifted out of the CLI result envelope with no scaffolding leak and validated
        // against the write allowlist + the existing task validator.
        let inner = r#"{\"classification\":{\"intent\":\"task_creation\",\"confidence\":0.9},\"action_request\":{\"tool\":\"task.create\",\"args\":{\"title\":\"Fix the login redirect\"}}}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let d = parse_cli_decision(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("an action_request envelope validates");
        let wt = d.action_request.expect("a validated write tool");
        assert_eq!(wt.tool, "task.create");
        assert_eq!(wt.intent, relux_core::PrimeIntent::TaskCreation);
        match wt.slot {
            relux_kernel::WriteToolSlot::Task(s) => assert_eq!(s.title, "Fix the login redirect"),
            other => panic!("expected a task slot, got {other:?}"),
        }

        // A mutating-sounding / off-allowlist write tool is dropped at the seam (never mapped to an
        // action); with no other usable section the whole envelope falls back to the specialized path.
        let bogus = r#"{\"action_request\":{\"tool\":\"task.delete\",\"args\":{\"task_id\":\"task_0001\"}}}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{bogus}"}}"#);
        assert!(
            parse_cli_decision(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an off-allowlist write tool with no other section must fail closed"
        );
    }

    #[test]
    fn cli_decision_unknown_top_level_field_fails_closed() {
        // A smuggled un-modeled top-level key fails the WHOLE envelope at the seam, so the
        // turn falls back to the specialized paths rather than acting on a partial decision.
        let inner = r#"{\"classification\":{\"intent\":\"task_creation\",\"confidence\":0.9},\"execute\":true}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}"}}"#);
        assert!(
            parse_cli_decision(&stdout, relux_core::AdapterKind::ClaudeCli).is_none(),
            "an unknown top-level field must fail the envelope closed"
        );
    }

    #[test]
    fn cli_tool_round_lifts_text_and_drops_error_envelopes() {
        // The read-only loop's CLI round: a brain's tool-call JSON wrapped in the result envelope
        // is lifted out with no scaffolding leak, so interpret_reply sees only the inner text.
        let inner = r#"{\"tool\":\"get_task\",\"args\":{\"task_id\":\"task_0001\"}}"#;
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"{inner}","session_id":"abc"}}"#);
        let text = lift_cli_tool_text(&stdout, relux_core::AdapterKind::ClaudeCli)
            .expect("a valid envelope yields the inner tool-call text");
        assert!(text.contains("get_task") && text.contains("task_0001"));
        assert!(!text.contains("session_id") && !text.contains("\"type\""));
        // The lifted text feeds the SAME validator the loop uses → a real allowlisted call.
        assert!(matches!(
            relux_kernel::interpret_reply(&text),
            relux_kernel::BrainTurn::Call(_)
        ));
        // An error envelope is dropped (the loop round ends; the raw error never leaks).
        let err = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        assert!(lift_cli_tool_text(err, relux_core::AdapterKind::ClaudeCli).is_none());
        // Codex text mode emits the JSON as raw prose (no envelope) — still lifted.
        let raw = r#"{"done": true}"#;
        let text = lift_cli_tool_text(raw, relux_core::AdapterKind::CodexCli).expect("raw prose lifts");
        assert!(matches!(
            relux_kernel::interpret_reply(&text),
            relux_kernel::BrainTurn::Done
        ));
    }

    #[test]
    fn cli_polish_rejects_error_envelope() {
        // An envelope reporting an error is never used as a polish, even though it
        // might contain JSON-looking text.
        let stdout = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        let p = polish_proposal_fixture();
        assert!(
            shape_cli_brain_polish(stdout, relux_core::AdapterKind::ClaudeCli, "Claude CLI", &p).is_none(),
            "an error envelope must fall back to the deterministic preview"
        );
    }

    #[test]
    fn cli_polish_rejects_structural_drift_at_the_seam() {
        let p = polish_proposal_fixture();
        // Added step -> dropped; no other usable content -> None.
        let added = r#"{"steps":[{"index":1,"title":"a"},{"index":2,"title":"b"},{"index":3,"title":"c"}]}"#;
        assert!(shape_cli_brain_polish(added, relux_core::AdapterKind::CodexCli, "Codex CLI", &p).is_none());
        // Reorder/rename of an index -> titles dropped, but a valid summary survives.
        let reordered = r#"{"summary":"nice","steps":[{"index":1,"title":"a"},{"index":3,"title":"b"}]}"#;
        let overlay = shape_cli_brain_polish(reordered, relux_core::AdapterKind::CodexCli, "Codex CLI", &p)
            .expect("the summary is still usable");
        assert!(overlay.step_titles.is_empty(), "drifted indexes drop the titles");
        assert_eq!(overlay.summary.as_deref(), Some("nice"));
        // The authoritative proposal is never mutated by validation.
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[1].agent, "prime");
    }

    #[tokio::test]
    async fn cli_polish_via_cli_returns_none_when_adapter_not_installed() {
        // No adapter status -> no spawn, no overlay; the deterministic preview stands.
        let p = polish_proposal_fixture();
        assert!(
            polish_proposal_via_cli(relux_kernel::PrimeBrain::ClaudeCli, None, &p)
                .await
                .is_none()
        );
        // A single-step proposal carries nothing to refine -> None even with a brain.
        let single = relux_core::PrimeProposal {
            multi_step: false,
            steps: vec![],
            agents: vec![],
            ..polish_proposal_fixture()
        };
        assert!(
            polish_proposal_via_cli(relux_kernel::PrimeBrain::ClaudeCli, None, &single)
                .await
                .is_none()
        );
    }

    #[test]
    fn prime_response_wire_can_never_carry_proposed_changes() {
        // Structural guarantee: the chat response flattens PrimeTurn + state +
        // ai_* fields. PrimeTurn has no proposed_changes field, so a proposed
        // change can never reach the chat wire even if an envelope declared one —
        // the only review/apply surface is the assigned-run path (GET …/runs/:id).
        let turn = PrimeTurn {
            intent: relux_core::PrimeIntent::Greeting,
            reply: "Hey!".to_string(),
            disposition: relux_core::PrimeDisposition::Answered,
            action: None,
            created_task: None,
            started_run: None,
            created_agent: None,
            approval: None,
            invoked_tool: None,
            tool_output: None,
            tool_error: None,
            suggested_actions: Vec::new(),
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
            assign_slots: None,
            update: None,
            context_reads: vec![],
        };
        let wire = serde_json::to_value(&turn).expect("PrimeTurn serializes");
        assert!(
            wire.get("proposed_changes").is_none(),
            "the Prime chat turn wire must not carry proposed_changes"
        );
        assert!(
            wire.get("artifacts").is_none(),
            "the Prime chat turn wire must not carry artifacts"
        );
    }

    // --- Local operator login (HTTP) ---------------------------------------

    use tower::ServiceExt; // for `oneshot`

    /// Build a real AppState over a throwaway temp store, plus the temp dir to
    /// keep it alive for the test's lifetime. `auth_disabled` toggles the bypass.
    fn auth_state(auth_disabled: bool) -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("local.db");
        // Bootstrap the store so protected handlers have a real kernel to read.
        let mut store = SqliteStore::open(&db_path).expect("open store");
        let mut kernel = store.load().expect("load");
        crate::ensure_bootstrapped(&mut kernel).expect("bootstrap");
        store.save(&kernel).expect("save");
        let state = AppState {
            db_path,
            plugins_root: dir.path().join("plugins"),
            uploads_root: dir.path().join("uploads"),
            dashboard_dir: None,
            ai_config_path: dir.path().join("ai.json"),
            dashboard_auth: relux_kernel::DashboardAuth::from_admin_path(
                &dir.path().join("dashboard-admin.json"),
            ),
            agent_tokens: relux_kernel::AgentTokenStore::from_path(
                &dir.path().join("dashboard-agent-tokens.json"),
            ),
            auth_disabled,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
            live_run_logs: relux_kernel::LiveRunLogs::new(),
            run_cancellations: relux_kernel::RunCancellations::new(),
        };
        (state, dir)
    }

    /// Issue one request against a freshly-built router and return
    /// (status, set-cookie value if any, body string).
    async fn call(
        state: &AppState,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        json_body: Option<&str>,
    ) -> (StatusCode, Option<String>, String) {
        let mut builder = axum::http::Request::builder().method(method).uri(path);
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, c);
        }
        let body = match json_body {
            Some(b) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                axum::body::Body::from(b.to_string())
            }
            None => axum::body::Body::empty(),
        };
        let req = builder.body(body).unwrap();
        let resp = router(state.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, set_cookie, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn create_task_with_tool_call_directive_stores_canonical_input() {
        // An operator can create a task that names ONE MCP/plugin tool to run; the
        // POST body's directive is validated + serialized into the canonical
        // `{ "tool_call": { plugin, tool, args } }` input the local run reads.
        // (`docs/mcp.md` "Run-driven MCP tool call".)
        let (state, _dir) = auth_state(true);
        let body = r#"{"title":"call fs search","tool_call":{"plugin":"mcp:fs","tool":"search","args":{"q":"files"}}}"#;
        let (status, _, tb) = call(&state, "POST", "/v1/relux/tasks", None, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "directive task create failed: {tb}");
        let task: serde_json::Value = serde_json::from_str(&tb).unwrap();
        assert_eq!(task["input"]["tool_call"]["plugin"], "mcp:fs");
        assert_eq!(task["input"]["tool_call"]["tool"], "search");
        assert_eq!(task["input"]["tool_call"]["args"]["q"], "files");
    }

    #[tokio::test]
    async fn create_task_with_empty_directive_tool_is_rejected() {
        // A directive with an empty tool is a 400 — never silently dropped to an echo
        // task (that would hide the operator's intent).
        let (state, _dir) = auth_state(true);
        let body = r#"{"title":"bad","tool_call":{"plugin":"mcp:fs","tool":"   "}}"#;
        let (status, _, _b) = call(&state, "POST", "/v1/relux/tasks", None, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Extract the `relux_session=...` pair from a Set-Cookie header so a later
    /// request can present it (mirrors a browser cookie jar).
    fn session_pair(set_cookie: &str) -> String {
        set_cookie
            .split(';')
            .next()
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn mcp_server_routes_register_list_reject_and_delete() {
        let (state, _dir) = auth_state(true); // auth disabled for the test

        // Register a loopback MCP server.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/mcp/servers",
            None,
            Some(r#"{"id":"fs","endpoint":"http://127.0.0.1:8000/mcp","description":"local fs"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "register body: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "fs");
        assert_eq!(v["transport"], "http_loopback");
        assert_eq!(v["status"], "configured");

        // It shows up in the list.
        let (status, _, body) = call(&state, "GET", "/v1/relux/mcp/servers", None, None).await;
        assert_eq!(status, StatusCode::OK);
        let list: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);

        // A non-loopback endpoint is refused (400, loopback-only).
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/mcp/servers",
            None,
            Some(r#"{"id":"remote","endpoint":"https://mcp.example.com"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Discovery against an unknown server is a 404; nothing is faked.
        let (status, _, _) =
            call(&state, "GET", "/v1/relux/mcp/servers/nope/tools", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Delete it.
        let (status, _, _) = call(&state, "DELETE", "/v1/relux/mcp/servers/fs", None, None).await;
        assert_eq!(status, StatusCode::OK);
        let (_, _, body) = call(&state, "GET", "/v1/relux/mcp/servers", None, None).await;
        let list: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn mcp_tool_classification_route_sets_and_clears() {
        let (state, _dir) = auth_state(true);
        // Register a server.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/mcp/servers",
            None,
            Some(r#"{"id":"fs","endpoint":"http://127.0.0.1:8000/mcp"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Classify a tool Low + auto-approve.
        let (status, _, body) = call(
            &state,
            "PUT",
            "/v1/relux/mcp/servers/fs/tools/search/classification",
            None,
            Some(r#"{"risk":"low","approval":"never"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "classify body: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["tool_overrides"]["search"]["risk"], "low");
        assert_eq!(v["tool_overrides"]["search"]["approval"], "never");

        // Classifying an unknown server is a 404.
        let (status, _, _) = call(
            &state,
            "PUT",
            "/v1/relux/mcp/servers/ghost/tools/search/classification",
            None,
            Some(r#"{"risk":"low","approval":"never"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Clear it → reverts to the gated default (no override row).
        let (status, _, body) = call(
            &state,
            "DELETE",
            "/v1/relux/mcp/servers/fs/tools/search/classification",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["tool_overrides"].as_object().map(|o| o.is_empty()).unwrap_or(true));
    }

    #[tokio::test]
    async fn mcp_resource_routes_are_honest_on_bad_input() {
        let (state, _dir) = auth_state(true);
        // Resources on an unknown server → 404 (never a fabricated empty list).
        let (status, _, _) =
            call(&state, "GET", "/v1/relux/mcp/servers/nope/resources", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // Read with a missing uri query param → 400.
        let (status, _, _) =
            call(&state, "GET", "/v1/relux/mcp/servers/nope/resources/read", None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Register, then disable, a server: resources on a disabled server → 409.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/mcp/servers",
            None,
            Some(r#"{"id":"fs","endpoint":"http://127.0.0.1:8000/mcp","enabled":false}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) =
            call(&state, "GET", "/v1/relux/mcp/servers/fs/resources", None, None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        // Read with a uri but a disabled server → 409 (gate before dialing).
        let (status, _, _) = call(
            &state,
            "GET",
            "/v1/relux/mcp/servers/fs/resources/read?uri=file:///x.md",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    /// Write a fake CLI that prints `output` and exits 0 (cross-platform), used to
    /// seed a real captured run-log without a real coding-agent CLI.
    fn write_fake_cli(dir: &std::path::Path, name: &str, output: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(&path, format!("@echo off\r\necho {output}\r\n")).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\necho '{output}'\n")).unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    /// Seed a completed CLI run (with a captured log) into the state's store and
    /// return its run id. Reuses the bundled claude adapter that bootstrap installs.
    fn seed_cli_run_with_log(state: &AppState, dir: &std::path::Path, body: &str) -> String {
        let fake = write_fake_cli(dir, "fake-claude", body);
        let mut store = SqliteStore::open(&state.db_path).expect("open");
        let mut kernel = store.load().expect("load");
        let ns = relux_core::NamespaceId::new("workspace");
        let adapter = relux_core::PluginId::new("relux-adapter-claude-cli");
        kernel
            .configure_adapter_runtime(
                &adapter,
                Some(true),
                Some(fake.to_string_lossy().to_string()),
                Some(30),
                Some(8192),
                None,
            )
            .expect("configure adapter");
        let agent = kernel
            .create_agent(
                "coder",
                "Coder",
                "writes code",
                &adapter,
                &ns,
                Some("careful".to_string()),
                vec![],
            )
            .expect("agent");
        let task = kernel.create_task(
            "Summarize",
            serde_json::json!({ "path": "." }),
            "founder",
            &ns,
            vec![],
        );
        kernel.assign_task(&task, &agent).expect("assign");
        let run = kernel.execute_assigned_run(&task).expect("run");
        store.save(&kernel).expect("save");
        run.to_string()
    }

    #[tokio::test]
    async fn run_logs_route_returns_bounded_tail_since_cursor_and_404() {
        let (state, dir) = auth_state(true); // auth disabled for the read
        let run_id = seed_cli_run_with_log(&state, dir.path(), "RAN_LOG_77");

        // Full tail: 200 with classified stdout + system lines.
        let (status, _c, body) =
            call(&state, "GET", &format!("/v1/relux/runs/{run_id}/logs"), None, None).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        let log: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(log["run_id"], run_id);
        let lines = log["lines"].as_array().expect("lines array");
        assert!(
            lines.iter().any(|l| l["source"] == "stdout"
                && l["text"].as_str().unwrap_or_default().contains("RAN_LOG_77")),
            "stdout body line must be present: {body}"
        );
        assert!(
            lines.iter().any(|l| l["source"] == "system"),
            "a system framing line must be present"
        );
        let last_seq = lines.last().unwrap()["seq"].as_u64().unwrap();

        // Incremental ?since=<last>: 200, no new lines (the pollable tail).
        let (status2, _c, body2) = call(
            &state,
            "GET",
            &format!("/v1/relux/runs/{run_id}/logs?since={last_seq}"),
            None,
            None,
        )
        .await;
        assert_eq!(status2, StatusCode::OK);
        let log2: serde_json::Value = serde_json::from_str(&body2).unwrap();
        assert!(log2["lines"].as_array().unwrap().is_empty(), "no new lines past the cursor");

        // Unknown run: the kernel's existing `UnknownRun` 400 (a real run with no
        // log would instead be an empty 200, not an error).
        let (status3, _c, _b) =
            call(&state, "GET", "/v1/relux/runs/run_does_not_exist/logs", None, None).await;
        assert_eq!(status3, StatusCode::BAD_REQUEST);
    }

    /// Seed a RUNNING run with NO finalized log (started, not executed) and return
    /// its run id. Mirrors an in-flight off-lock orchestration brief whose process
    /// is still streaming, so `get_run_logs` must serve the LIVE registry, not the
    /// (absent) durable log.
    fn seed_running_run_without_log(state: &AppState) -> String {
        let mut store = SqliteStore::open(&state.db_path).expect("open");
        let mut kernel = store.load().expect("load");
        let ns = relux_core::NamespaceId::new("workspace");
        let adapter = relux_core::PluginId::new("relux-adapter-local-prime");
        let agent = kernel
            .create_agent("coder", "Coder", "writes code", &adapter, &ns, None, vec![])
            .expect("agent");
        let task = kernel.create_task(
            "Summarize",
            serde_json::json!({ "path": "." }),
            "founder",
            &ns,
            vec![],
        );
        kernel.assign_task(&task, &agent).expect("assign");
        let run = kernel.start_run(&task).expect("start run");
        store.save(&kernel).expect("save");
        run.to_string()
    }

    #[tokio::test]
    async fn run_logs_route_serves_the_live_tail_before_finalization() {
        let (state, _dir) = auth_state(true);
        // A real, RUNNING run that has not finalized — so the durable log is empty.
        let run_id = seed_running_run_without_log(&state);
        let rid = relux_core::RunId::new(run_id.clone());

        // With no live buffer yet, the route returns the honest empty tail (200).
        let (status0, _c, body0) =
            call(&state, "GET", &format!("/v1/relux/runs/{run_id}/logs"), None, None).await;
        assert_eq!(status0, StatusCode::OK, "body: {body0}");
        let log0: serde_json::Value = serde_json::from_str(&body0).unwrap();
        assert!(log0["lines"].as_array().unwrap().is_empty(), "no live + no durable ⇒ empty");

        // Now stream some live lines (as an off-lock spawn would).
        let sink = state.live_run_logs.begin(&rid);
        sink.system("spawned adapter 'fake'");
        sink.append(relux_core::RunLogSource::Stdout, "live out line\n");
        sink.append(relux_core::RunLogSource::Stderr, "live err line\n");

        // A poll DURING the run now sees the live tail — before any finalize.
        let (status1, _c, body1) =
            call(&state, "GET", &format!("/v1/relux/runs/{run_id}/logs"), None, None).await;
        assert_eq!(status1, StatusCode::OK, "body: {body1}");
        let log1: serde_json::Value = serde_json::from_str(&body1).unwrap();
        let lines = log1["lines"].as_array().expect("lines");
        assert!(
            lines.iter().any(|l| l["source"] == "stdout"
                && l["text"].as_str().unwrap_or_default().contains("live out line")),
            "live stdout line must be served before finalization: {body1}"
        );
        assert!(
            lines.iter().any(|l| l["source"] == "system"),
            "the live system framing line must be present: {body1}"
        );
        let last_seq = lines.last().unwrap()["seq"].as_u64().unwrap();

        // Incremental ?since=<seq> over the LIVE tail returns only newer lines.
        sink.append(relux_core::RunLogSource::Stdout, "another live line\n");
        let (status2, _c, body2) = call(
            &state,
            "GET",
            &format!("/v1/relux/runs/{run_id}/logs?since={last_seq}"),
            None,
            None,
        )
        .await;
        assert_eq!(status2, StatusCode::OK);
        let log2: serde_json::Value = serde_json::from_str(&body2).unwrap();
        let tail = log2["lines"].as_array().unwrap();
        assert_eq!(tail.len(), 1, "only the line past the cursor: {body2}");
        assert!(tail[0]["text"].as_str().unwrap().contains("another live line"));

        // Once the run finalizes (durable log persisted) the live buffer is dropped
        // and the canonical log wins — modelled by finishing the live entry.
        state.live_run_logs.finish(&rid);
        let (status3, _c, body3) =
            call(&state, "GET", &format!("/v1/relux/runs/{run_id}/logs"), None, None).await;
        assert_eq!(status3, StatusCode::OK);
        let log3: serde_json::Value = serde_json::from_str(&body3).unwrap();
        // The durable log for this never-executed run is empty (no fabricated lines).
        assert!(log3["lines"].as_array().unwrap().is_empty(), "durable wins, honestly empty: {body3}");
    }

    #[tokio::test]
    async fn cancel_route_requests_cancel_for_a_live_run_and_is_honest_otherwise() {
        let (state, _dir) = auth_state(true);
        // A real, RUNNING run; the off-lock spawn would register a cancel token.
        let run_id = seed_running_run_without_log(&state);
        let rid = relux_core::RunId::new(run_id.clone());

        // No live token yet ⇒ honest not_running (cancelling=false), still 200.
        let (status0, _c, body0) =
            call(&state, "POST", &format!("/v1/relux/runs/{run_id}/cancel"), None, None).await;
        assert_eq!(status0, StatusCode::OK, "body: {body0}");
        let r0: serde_json::Value = serde_json::from_str(&body0).unwrap();
        assert_eq!(r0["status"], "not_running");
        assert_eq!(r0["cancelling"], false);

        // Simulate the off-lock spawn opening a cancel token for this run.
        let token = state.run_cancellations.begin(&rid);
        assert!(!token.is_cancelled());

        // First cancel ⇒ requested + the spawn's flag is now set.
        let (status1, _c, body1) =
            call(&state, "POST", &format!("/v1/relux/runs/{run_id}/cancel"), None, None).await;
        assert_eq!(status1, StatusCode::OK, "body: {body1}");
        let r1: serde_json::Value = serde_json::from_str(&body1).unwrap();
        assert_eq!(r1["status"], "requested");
        assert_eq!(r1["cancelling"], true);
        assert!(token.is_cancelled(), "the off-lock spawn must now see the cancel flag");

        // Repeat ⇒ idempotent already_requested (still cancelling).
        let (status2, _c, body2) =
            call(&state, "POST", &format!("/v1/relux/runs/{run_id}/cancel"), None, None).await;
        assert_eq!(status2, StatusCode::OK);
        let r2: serde_json::Value = serde_json::from_str(&body2).unwrap();
        assert_eq!(r2["status"], "already_requested");
        assert_eq!(r2["cancelling"], true);

        // Once finalized (token dropped) a later cancel honestly reports not_running.
        state.run_cancellations.finish(&rid);
        let (status3, _c, body3) =
            call(&state, "POST", &format!("/v1/relux/runs/{run_id}/cancel"), None, None).await;
        assert_eq!(status3, StatusCode::OK);
        let r3: serde_json::Value = serde_json::from_str(&body3).unwrap();
        assert_eq!(r3["status"], "not_running");

        // An unknown run id is the kernel's existing UnknownRun 400.
        let (status4, _c, _b) =
            call(&state, "POST", "/v1/relux/runs/run_nope/cancel", None, None).await;
        assert_eq!(status4, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn health_is_public_but_state_requires_a_session() {
        let (state, _dir) = auth_state(false);
        // Liveness probe works with no session (a probe must run before login).
        let (health, _, _) = call(&state, "GET", "/v1/relux/health", None, None).await;
        assert_eq!(health, StatusCode::OK);
        // A protected control-plane route is 401 without a session, and reports
        // needs_setup so the dashboard routes to the setup screen.
        let (state_status, _, body) =
            call(&state, "GET", "/v1/relux/state", None, None).await;
        assert_eq!(state_status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("\"needs_setup\":true"), "got: {body}");
    }

    #[tokio::test]
    async fn doctor_requires_a_session_and_returns_structured_checks() {
        // Protected like /v1/relux/state: no session → 401.
        let (gated, _g) = auth_state(false);
        let (status, _, _) = call(&gated, "GET", "/v1/relux/doctor", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Dev bypass: the report is served over a bootstrapped store.
        let (state, _dir) = auth_state(true);
        let (status, _, body) = call(&state, "GET", "/v1/relux/doctor", None, None).await;
        assert_eq!(status, StatusCode::OK, "got: {body}");
        let report: serde_json::Value = serde_json::from_str(&body).unwrap();

        // The expected check rows are present and the store loaded cleanly.
        let ids: Vec<&str> = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        for want in [
            "kernel.store",
            "dashboard.bundle",
            "prime.brain",
            "adapters.real_work",
            "plugins.tools",
            "crew",
            "approvals.pending",
        ] {
            assert!(ids.contains(&want), "missing check {want}; got {ids:?}");
        }
        let store_row = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "kernel.store")
            .unwrap();
        assert_eq!(store_row["severity"], "ok");
        assert!(report["overall"].is_string());
        assert!(report["summary"]["fail"].is_number());

        // Redaction: the report never carries the on-disk db path or temp dir.
        let db_path = state.db_path.display().to_string();
        assert!(!body.contains(&db_path), "db path leaked into doctor report");
    }

    #[tokio::test]
    async fn agent_create_and_edit_workflow_over_http() {
        // Dev bypass so the calls need no session; exercises the manual Crew config path.
        let (state, _dir) = auth_state(true);

        // Create with a persona; the default (local Prime) adapter is used.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"Research Bot","role":"does research","persona":"calm and precise","skills":["Research","research","Data Science"]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create failed: {body}");
        assert!(body.contains("\"id\":\"research-bot\""), "got: {body}");
        assert!(body.contains("\"persona\":\"calm and precise\""), "got: {body}");
        assert!(
            body.contains("relux-adapter-local-prime"),
            "default adapter expected: {body}"
        );
        // Skills are slugified + deduped (case-insensitive) and round-trip on the wire.
        assert!(
            body.contains("\"skills\":[\"research\",\"data-science\"]"),
            "skills not normalized/persisted: {body}"
        );

        // A duplicate display name is an honest 400.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"research bot"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "dup name should 400: {body}");
        assert!(body.contains("already exists"), "got: {body}");

        // An unknown adapter is rejected (not a known/installed adapter).
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"Bad Bot","adapter_plugin":"relux-adapter-bogus"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "unknown adapter should 400: {body}");
        assert!(body.contains("unknown adapter"), "got: {body}");

        // An unsanitizable skill (an entry with real content that slugs to nothing) is a 400.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"Emoji Bot","skills":["💥🔥"]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "invalid skill should 400: {body}");
        assert!(body.contains("invalid skill"), "got: {body}");

        // Edit: pause the agent, clear the persona, and REPLACE the skill list.
        let (status, _, body) = call(
            &state,
            "PATCH",
            "/v1/relux/agents/research-bot",
            None,
            Some(r#"{"status":"disabled","persona":"","skills":["frontend"]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "edit failed: {body}");
        assert!(body.contains("\"status\":\"Disabled\""), "got: {body}");
        assert!(!body.contains("\"persona\""), "persona should be cleared: {body}");
        assert!(body.contains("\"skills\":[\"frontend\"]"), "skills not replaced: {body}");

        // Edit again with an empty skills array CLEARS the list (present => replace).
        let (status, _, body) = call(
            &state,
            "PATCH",
            "/v1/relux/agents/research-bot",
            None,
            Some(r#"{"skills":[]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "clear-skills edit failed: {body}");
        assert!(body.contains("\"skills\":[]"), "skills should be cleared: {body}");

        // The list endpoint reflects the agent (skills now empty after the clear).
        let (status, _, body) = call(&state, "GET", "/v1/relux/agents", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"id\":\"research-bot\""), "list missing agent: {body}");

        // Editing a non-existent agent is a 404.
        let (status, _, _) = call(
            &state,
            "PATCH",
            "/v1/relux/agents/ghost",
            None,
            Some(r#"{"role":"x"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The agent record now carries its EXPLICIT permission list (the create
        // granted only the minimal echo tool — never a dangerous capability).
        let (status, _, body) = call(&state, "GET", "/v1/relux/agents", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("tool:relux-tools-echo:say"), "explicit perms expected: {body}");

        // Grant a permission through the explicit, audited operator path.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents/research-bot/permissions",
            None,
            Some(r#"{"permission":"tool:relux-tools-github:read"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "grant failed: {body}");
        assert!(body.contains("tool:relux-tools-github:read"), "got: {body}");

        // Revoke it again; the explicit list shrinks.
        let (status, _, body) = call(
            &state,
            "DELETE",
            "/v1/relux/agents/research-bot/permissions",
            None,
            Some(r#"{"permission":"tool:relux-tools-github:read"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "revoke failed: {body}");
        assert!(!body.contains("tool:relux-tools-github:read"), "should be revoked: {body}");

        // Revoking a permission the agent does not hold is an honest 404.
        let (status, _, _) = call(
            &state,
            "DELETE",
            "/v1/relux/agents/research-bot/permissions",
            None,
            Some(r#"{"permission":"tool:relux-tools-github:read"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // A malformed permission string is rejected before anything is mutated.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/research-bot/permissions",
            None,
            Some(r#"{"permission":"not-a-valid-prefix"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// The operator-assisted manager-subtree grant over HTTP
    /// (`POST /v1/relux/agents/:id/manager-grant`). Exercises the real authority gate
    /// end-to-end: a live manager with the subtree scope reaches its own-Branch
    /// subordinate (200); a sibling target, a manager with no scope, and a paused
    /// manager are all 403; a malformed permission is a 400; and both the success and a
    /// denial land in the audit log under `operator:authorize_manager_grant`.
    #[tokio::test]
    async fn manager_grant_to_subordinate_over_http_enforces_authority_and_audits() {
        let (state, _dir) = auth_state(true);

        // Build the topology director <- lead <- ic ; peer reports to director (a
        // sibling of lead). Each create supplies an explicit id + Lead.
        for (id, reports_to) in [
            ("director", None),
            ("lead", Some("director")),
            ("ic", Some("lead")),
            ("peer", Some("director")),
        ] {
            let body = match reports_to {
                Some(r) => format!(r#"{{"id":"{id}","name":"{id}","reports_to":"{r}"}}"#),
                None => format!(r#"{{"id":"{id}","name":"{id}"}}"#),
            };
            let (status, _, b) =
                call(&state, "POST", "/v1/relux/agents", None, Some(&body)).await;
            assert_eq!(status, StatusCode::OK, "create {id} failed: {b}");
        }

        // Grant the lead its manager-subtree scope through the ordinary operator path.
        let (status, _, b) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/permissions",
            None,
            Some(r#"{"permission":"agent:lead:subtree:grant_permission"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "scope grant failed: {b}");

        let perm = r#""permission":"tool:relux-tools-github:create_pr""#;

        // (1) SUCCESS: lead grants to its real subordinate ic; ic's list grows.
        let (status, _, b) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/manager-grant",
            None,
            Some(&format!(r#"{{"target_id":"ic",{perm}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "manager grant to subordinate failed: {b}");
        assert!(b.contains("tool:relux-tools-github:create_pr"), "ic should now hold it: {b}");

        // (2) SIBLING denial: ic's lead cannot reach peer (lead's sibling).
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/manager-grant",
            None,
            Some(&format!(r#"{{"target_id":"peer",{perm}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "sibling target must be denied");

        // (3) NO-SCOPE denial: director has a real subordinate (lead) but holds no
        // subtree scope, so it cannot grant.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/director/manager-grant",
            None,
            Some(&format!(r#"{{"target_id":"lead",{perm}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "manager without scope must be denied");

        // (4) MALFORMED permission: rejected (400) before any authority check.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/manager-grant",
            None,
            Some(r#"{"target_id":"ic","permission":"not-a-valid-prefix"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "malformed permission must be 400");

        // (5) PAUSED manager denial: a non-Active manager wields no subtree authority.
        let (status, _, b) = call(
            &state,
            "PATCH",
            "/v1/relux/agents/lead",
            None,
            Some(r#"{"status":"paused"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pause lead failed: {b}");
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/manager-grant",
            None,
            Some(&format!(r#"{{"target_id":"ic",{perm}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "paused manager must be denied");

        // (6) AUDIT: both a Success and a Denied operator-attribution row are present.
        let (status, _, audit) =
            call(&state, "GET", "/v1/relux/audit?limit=500", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            audit.contains("operator:authorize_manager_grant"),
            "operator attribution audit missing: {audit}"
        );
        assert!(audit.contains("\"trust_boundary\""), "trust-boundary detail missing: {audit}");
    }

    /// Issue a request carrying an `Authorization: Bearer <token>` (and no cookie),
    /// returning (status, body). The bearer-auth twin of [`call`].
    async fn call_bearer(
        state: &AppState,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        json_body: Option<&str>,
    ) -> (StatusCode, String) {
        let mut builder = axum::http::Request::builder().method(method).uri(path);
        if let Some(t) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let body = match json_body {
            Some(b) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                axum::body::Body::from(b.to_string())
            }
            None => axum::body::Body::empty(),
        };
        let req = builder.body(body).unwrap();
        let resp = router(state.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn agent_token_mint_authenticate_self_grant_and_revoke_over_http() {
        let (state, _dir) = auth_state(true);

        // Topology: lead <- ic ; outsider top-level + unrelated.
        for (id, reports_to) in [("lead", None), ("ic", Some("lead")), ("outsider", None)] {
            let body = match reports_to {
                Some(r) => format!(r#"{{"id":"{id}","name":"{id}","reports_to":"{r}"}}"#),
                None => format!(r#"{{"id":"{id}","name":"{id}"}}"#),
            };
            let (status, _, b) =
                call(&state, "POST", "/v1/relux/agents", None, Some(&body)).await;
            assert_eq!(status, StatusCode::OK, "create {id} failed: {b}");
        }
        // Grant the lead its manager-subtree scope via the operator path.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/permissions",
            None,
            Some(r#"{"permission":"agent:lead:subtree:grant_permission"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // (1) MINT a token for the lead. The raw token is returned ONCE; the response
        // states the copy-once contract.
        let (status, mint_body) =
            call_bearer(&state, "POST", "/v1/relux/agents/lead/tokens", None, Some(r#"{"label":"ci"}"#)).await;
        assert_eq!(status, StatusCode::OK, "mint failed: {mint_body}");
        let minted: serde_json::Value = serde_json::from_str(&mint_body).unwrap();
        let raw = minted["token"].as_str().expect("raw token in mint response").to_string();
        let token_id = minted["token_id"].as_str().unwrap().to_string();
        assert!(raw.starts_with("relux_agt_"), "token shape: {raw}");
        assert!(mint_body.contains("never be shown again"), "copy-once warning missing: {mint_body}");

        // (2) LIST returns metadata but NEVER the raw token or a hash.
        let (status, list_body) =
            call_bearer(&state, "GET", "/v1/relux/agents/lead/tokens", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(list_body.contains(&token_id), "token id should list: {list_body}");
        assert!(!list_body.contains(&raw), "raw token must NOT appear in list: {list_body}");

        // (3) MINT for an unknown agent → 404.
        let (status, _) =
            call_bearer(&state, "POST", "/v1/relux/agents/ghost/tokens", None, Some("{}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // (4) AUTH SUCCESS: the token authenticates as the lead on the self route; the
        // self record shows its Branch (ic is a direct report).
        let (status, me) =
            call_bearer(&state, "GET", "/v1/relux/agents/me", Some(&raw), None).await;
        assert_eq!(status, StatusCode::OK, "self-info failed: {me}");
        assert!(me.contains("\"id\":\"lead\""), "self id wrong: {me}");
        assert!(me.contains("\"ic\""), "branch direct report missing: {me}");

        // (5) AUTH FAILURE: no token, and a garbage token, are both 401.
        let (status, _) = call_bearer(&state, "GET", "/v1/relux/agents/me", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) =
            call_bearer(&state, "GET", "/v1/relux/agents/me", Some("relux_agt_bogus"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // (6) SELF MANAGER-GRANT SUCCESS: the lead grants to its subordinate ic, with no
        // operator in the loop — the acting manager is the token subject.
        let perm = r#""permission":"tool:relux-tools-github:create_pr""#;
        let (status, b) = call_bearer(
            &state,
            "POST",
            "/v1/relux/agents/me/manager-grant",
            Some(&raw),
            Some(&format!(r#"{{"target_id":"ic",{perm}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "self manager-grant failed: {b}");
        assert!(b.contains("tool:relux-tools-github:create_pr"), "ic should hold it: {b}");

        // (7) SELF MANAGER-GRANT DENIAL: the lead cannot reach an unrelated operative.
        let (status, _) = call_bearer(
            &state,
            "POST",
            "/v1/relux/agents/me/manager-grant",
            Some(&raw),
            Some(&format!(r#"{{"target_id":"outsider",{perm}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "target outside Branch must be denied");

        // (8) MALFORMED permission → 400 before any authority check.
        let (status, _) = call_bearer(
            &state,
            "POST",
            "/v1/relux/agents/me/manager-grant",
            Some(&raw),
            Some(r#"{"target_id":"ic","permission":"not-a-valid-prefix"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // (9) REVOKE the token; the same token then fails auth (401). Revoking an
        // unknown token id is an honest 404.
        let (status, _) = call_bearer(
            &state,
            "DELETE",
            &format!("/v1/relux/agents/lead/tokens/{token_id}"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "revoke failed");
        let (status, _) =
            call_bearer(&state, "GET", "/v1/relux/agents/me", Some(&raw), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "a revoked token must not authenticate");
        let (status, _) = call_bearer(
            &state,
            "DELETE",
            "/v1/relux/agents/lead/tokens/agt_nope",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // (10) AUDIT: mint, the token-authenticated grant, and revoke are all recorded.
        let (status, _, audit) =
            call(&state, "GET", "/v1/relux/audit?limit=500", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(audit.contains("agent:mint_token"), "mint audit missing: {audit}");
        assert!(audit.contains("agent:revoke_token"), "revoke audit missing: {audit}");
        assert!(
            audit.contains("agent:token_authenticated_manager_grant"),
            "token-authenticated grant audit missing: {audit}"
        );
        // The raw token NEVER appears in the audit trail (only the public token_id).
        assert!(!audit.contains(&raw), "raw token leaked into audit: {audit}");
    }

    #[tokio::test]
    async fn an_agent_token_does_not_open_operator_routes() {
        // With real operator auth ON, an agent token must NOT authenticate any operator
        // control-plane route, and the operator token-mint route still needs a session.
        let (state, _dir) = auth_state(false);
        // White-box mint (no operator session needed for this trust-boundary check).
        let minted = state.agent_tokens.mint("lead", "t", None);
        let raw = minted.secret;

        // The token is rejected on operator routes (those only ever check the cookie).
        for path in ["/v1/relux/state", "/v1/relux/agents", "/v1/relux/audit"] {
            let (status, _) = call_bearer(&state, "GET", path, Some(&raw), None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "agent token must not open {path}");
        }
        // The operator-only token-mint route requires a session, not a bearer token.
        let (status, _) =
            call_bearer(&state, "POST", "/v1/relux/agents/lead/tokens", Some(&raw), Some("{}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "minting needs an operator session");
    }

    /// End-to-end HTTP for the per-agent-authenticated manager-subtree TASK ASSIGNMENT
    /// (`POST /v1/relux/agents/me/assign-task`). Exercises the real authority gate (the
    /// acting manager is the token subject, never the body) + the assignability rule, and
    /// confirms success/denials are audited under `agent:token_authenticated_manager_assign_task`.
    #[tokio::test]
    async fn agent_token_assign_task_to_subordinate_over_http() {
        let (state, _dir) = auth_state(true);

        // Topology: director <- lead <- ic ; peer <- director (lead's sibling); outsider unrelated.
        for (id, reports_to) in [
            ("director", None),
            ("lead", Some("director")),
            ("ic", Some("lead")),
            ("peer", Some("director")),
            ("outsider", None),
        ] {
            let body = match reports_to {
                Some(r) => format!(r#"{{"id":"{id}","name":"{id}","reports_to":"{r}"}}"#),
                None => format!(r#"{{"id":"{id}","name":"{id}"}}"#),
            };
            let (status, _, b) = call(&state, "POST", "/v1/relux/agents", None, Some(&body)).await;
            assert_eq!(status, StatusCode::OK, "create {id} failed: {b}");
        }
        // Scope the lead ONLY for the assign_task subtree action.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/permissions",
            None,
            Some(r#"{"permission":"agent:lead:subtree:assign_task"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Mint a token for the lead and one for the director (who has a subordinate but no scope).
        let mint = |id: &str| {
            let st = state.clone();
            let id = id.to_string();
            async move {
                let (s, b) =
                    call_bearer(&st, "POST", &format!("/v1/relux/agents/{id}/tokens"), None, Some("{}")).await;
                assert_eq!(s, StatusCode::OK, "mint {id} failed: {b}");
                let v: serde_json::Value = serde_json::from_str(&b).unwrap();
                v["token"].as_str().unwrap().to_string()
            }
        };
        let lead_tok = mint("lead").await;
        let director_tok = mint("director").await;

        // Create a task via the operator route (auto-assigned to Prime, non-terminal).
        let (status, _, tb) =
            call(&state, "POST", "/v1/relux/tasks", None, Some(r#"{"title":"ship it"}"#)).await;
        assert_eq!(status, StatusCode::OK, "task create failed: {tb}");
        let task: serde_json::Value = serde_json::from_str(&tb).unwrap();
        let task_id = task["id"].as_str().unwrap().to_string();

        let assign = |tok: &str, target: &str, tid: &str| {
            let st = state.clone();
            let (tok, target, tid) = (tok.to_string(), target.to_string(), tid.to_string());
            async move {
                call_bearer(
                    &st,
                    "POST",
                    "/v1/relux/agents/me/assign-task",
                    Some(&tok),
                    Some(&format!(r#"{{"task_id":"{tid}","target_agent_id":"{target}"}}"#)),
                )
                .await
            }
        };

        // (1) SUCCESS: the lead assigns the live task to its subordinate ic.
        let (status, b) = assign(&lead_tok, "ic", &task_id).await;
        assert_eq!(status, StatusCode::OK, "self assign-task failed: {b}");
        assert!(b.contains("\"assigned_agent\":\"ic\""), "task should be assigned to ic: {b}");
        assert!(b.contains("\"status\":\"queued\""), "task should be queued: {b}");

        // (2) DENIALS: sibling / ancestor / self / unrelated targets are all 403.
        for bad in ["peer", "director", "lead", "outsider"] {
            let (status, _) = assign(&lead_tok, bad, &task_id).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "target {bad} must be denied");
        }

        // (3) NO SCOPE: the director holds no subtree scope — denied even over subordinate lead.
        let (status, _) = assign(&director_tok, "lead", &task_id).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "manager without scope must be denied");

        // (4) INVALID TARGET: an unknown target folds into the fail-closed authority check → 403.
        let (status, _) = assign(&lead_tok, "ghost", &task_id).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "unknown target must be denied");

        // (5) MISSING TASK: after authority passes, an unknown task is rejected as bad
        // input (UnknownTask → 400, the kernel's existing mapping for every task route).
        let (status, _) = assign(&lead_tok, "ic", "task_does_not_exist").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "missing task must be a 400");

        // (6) PAUSED MANAGER: pause the lead via the operator route — its token now wields no
        // subtree authority (liveness), even over a real subordinate.
        let (status, _, _) = call(
            &state,
            "PATCH",
            "/v1/relux/agents/lead",
            None,
            Some(r#"{"status":"paused"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pause lead failed");
        let (status, _) = assign(&lead_tok, "ic", &task_id).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "paused manager must be denied");

        // (7) AUDIT: the token-authenticated assignment is recorded; the raw token never leaks.
        let (status, _, audit) =
            call(&state, "GET", "/v1/relux/audit?limit=500", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            audit.contains("agent:token_authenticated_manager_assign_task"),
            "token-authenticated assign audit missing: {audit}"
        );
        assert!(audit.contains("task:assign"), "inner task:assign audit missing: {audit}");
        assert!(!audit.contains(&lead_tok), "raw token leaked into audit: {audit}");
    }

    /// End-to-end HTTP for the per-agent-authenticated manager-subtree permission REVOKE
    /// (`POST /v1/relux/agents/me/manager-revoke`). Exercises the real authority gate (the
    /// acting manager is the token subject, never the body) + the exact-match holding rule,
    /// and confirms success/denials are audited under
    /// `agent:token_authenticated_manager_revoke_permission`.
    #[tokio::test]
    async fn agent_token_manager_revoke_permission_over_http() {
        let (state, _dir) = auth_state(true);

        // Topology: director <- lead <- ic ; peer <- director (lead's sibling); outsider unrelated.
        for (id, reports_to) in [
            ("director", None),
            ("lead", Some("director")),
            ("ic", Some("lead")),
            ("peer", Some("director")),
            ("outsider", None),
        ] {
            let body = match reports_to {
                Some(r) => format!(r#"{{"id":"{id}","name":"{id}","reports_to":"{r}"}}"#),
                None => format!(r#"{{"id":"{id}","name":"{id}"}}"#),
            };
            let (status, _, b) = call(&state, "POST", "/v1/relux/agents", None, Some(&body)).await;
            assert_eq!(status, StatusCode::OK, "create {id} failed: {b}");
        }
        // Scope the lead ONLY for the revoke_permission subtree action.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/lead/permissions",
            None,
            Some(r#"{"permission":"agent:lead:subtree:revoke_permission"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // ic and peer both hold a concrete permission (granted via the operator path).
        for who in ["ic", "peer"] {
            let (status, _, _) = call(
                &state,
                "POST",
                &format!("/v1/relux/agents/{who}/permissions"),
                None,
                Some(r#"{"permission":"tool:relux-tools-github:create_pr"}"#),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "grant to {who} failed");
        }

        // Mint a token for the lead and one for the director (who has a subordinate but no scope).
        let mint = |id: &str| {
            let st = state.clone();
            let id = id.to_string();
            async move {
                let (s, b) =
                    call_bearer(&st, "POST", &format!("/v1/relux/agents/{id}/tokens"), None, Some("{}")).await;
                assert_eq!(s, StatusCode::OK, "mint {id} failed: {b}");
                let v: serde_json::Value = serde_json::from_str(&b).unwrap();
                v["token"].as_str().unwrap().to_string()
            }
        };
        let lead_tok = mint("lead").await;
        let director_tok = mint("director").await;

        let revoke = |tok: &str, target: &str, perm: &str| {
            let st = state.clone();
            let (tok, target, perm) = (tok.to_string(), target.to_string(), perm.to_string());
            async move {
                call_bearer(
                    &st,
                    "POST",
                    "/v1/relux/agents/me/manager-revoke",
                    Some(&tok),
                    Some(&format!(r#"{{"target_id":"{target}","permission":"{perm}"}}"#)),
                )
                .await
            }
        };
        let github_pr = "tool:relux-tools-github:create_pr";

        // (0) NO BEARER: the agent-self route is bearer-gated — without a token it is 401.
        let (status, _) = call_bearer(
            &state,
            "POST",
            "/v1/relux/agents/me/manager-revoke",
            None,
            Some(&format!(r#"{{"target_id":"ic","permission":"{github_pr}"}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "manager-revoke must require a bearer token");

        // (1) SUCCESS: the lead revokes the permission from its subordinate ic.
        let (status, b) = revoke(&lead_tok, "ic", github_pr).await;
        assert_eq!(status, StatusCode::OK, "self manager-revoke failed: {b}");
        assert!(!b.contains(github_pr), "ic should no longer hold the permission: {b}");

        // (2) DENIALS: sibling / ancestor / self / unrelated targets are all 403. The
        // sibling `peer` still holds the permission — a denied revoke mutates nothing.
        for bad in ["peer", "director", "lead", "outsider"] {
            let (status, _) = revoke(&lead_tok, bad, github_pr).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "target {bad} must be denied");
        }

        // (3) NO SCOPE: the director holds no subtree scope — denied even over subordinate lead.
        let (status, _) = revoke(&director_tok, "lead", github_pr).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "manager without scope must be denied");

        // (4) INVALID TARGET: an unknown target folds into the fail-closed authority check → 403.
        let (status, _) = revoke(&lead_tok, "ghost", github_pr).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "unknown target must be denied");

        // (5) PERMISSION NOT HELD: after authority passes, revoking a permission ic does not
        // hold is the honest PermissionNotGranted → 404 (the operator revoke's contract).
        let (status, _) = revoke(&lead_tok, "ic", github_pr).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "revoking an unheld permission must be a 404");

        // (6) MALFORMED permission → 400 before any authority check.
        let (status, _) = call_bearer(
            &state,
            "POST",
            "/v1/relux/agents/me/manager-revoke",
            Some(&lead_tok),
            Some(r#"{"target_id":"ic","permission":"not-a-valid-prefix"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // (7) PAUSED MANAGER: pause the lead — its token now wields no subtree authority,
        // even over a real subordinate that still holds the permission (peer still holds it
        // too, but lead never had Branch authority over peer).
        let (status, _, _) = call(
            &state,
            "PATCH",
            "/v1/relux/agents/lead",
            None,
            Some(r#"{"status":"paused"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pause lead failed");
        // Re-grant the permission to ic so the only failing rule is liveness.
        let (status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/agents/ic/permissions",
            None,
            Some(r#"{"permission":"tool:relux-tools-github:create_pr"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = revoke(&lead_tok, "ic", github_pr).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "paused manager must be denied");

        // (8) AUDIT: the token-authenticated revoke is recorded; the raw token never leaks.
        let (status, _, audit) =
            call(&state, "GET", "/v1/relux/audit?limit=500", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            audit.contains("agent:token_authenticated_manager_revoke_permission"),
            "token-authenticated revoke audit missing: {audit}"
        );
        assert!(audit.contains("agent:revoke_permission"), "inner agent:revoke_permission audit missing: {audit}");
        assert!(!audit.contains(&lead_tok), "raw token leaked into audit: {audit}");
    }

    #[tokio::test]
    async fn agent_presets_list_and_create_with_preset_over_http() {
        let (state, _dir) = auth_state(true);

        // The read-only presets endpoint lists the curated bundles (advisory text only).
        let (status, _, body) = call(&state, "GET", "/v1/relux/agent-presets", None, None).await;
        assert_eq!(status, StatusCode::OK, "presets list failed: {body}");
        assert!(body.contains("\"id\":\"researcher\""), "researcher preset missing: {body}");
        assert!(body.contains("\"id\":\"builder\""), "builder preset missing: {body}");
        // A preset is advisory only — it carries no permission/adapter field on the wire.
        assert!(!body.contains("permission"), "preset must not expose permissions: {body}");
        assert!(!body.contains("adapter"), "preset must not pick an adapter: {body}");

        // Create from a preset id alone: role/persona/skills are filled from the bundle
        // and validated through the normal path.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"Scout","preset":"researcher"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "preset create failed: {body}");
        assert!(body.contains("\"id\":\"scout\""), "got: {body}");
        assert!(body.contains("\"skills\":[\"research\",\"analysis\",\"writing\"]"), "preset skills: {body}");
        // CRITICAL: a preset grants NOTHING beyond the minimal echo tool — it never
        // auto-grants an elevated capability.
        assert!(body.contains("tool:relux-tools-echo:say"), "echo grant expected: {body}");
        assert!(!body.contains("\"permissions\":[\"tool:relux-tools-echo:say\",\""), "preset over-granted: {body}");
        // The default (local Prime) adapter is used — a preset picks no runtime.
        assert!(body.contains("relux-adapter-local-prime"), "preset must not pick adapter: {body}");

        // An explicit field overrides the preset default (request value wins).
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"Custom Scout","preset":"researcher","role":"my own role"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "override create failed: {body}");
        assert!(body.contains("\"description\":\"my own role\""), "override ignored: {body}");
        // Skills still come from the preset (not overridden here).
        assert!(body.contains("\"skills\":[\"research\",\"analysis\",\"writing\"]"), "preset skills: {body}");

        // An unknown preset id is an honest 400 (fail closed) — no agent created.
        let (status, _, body) = call(
            &state,
            "POST",
            "/v1/relux/agents",
            None,
            Some(r#"{"name":"Ghost","preset":"evil-overlord"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "unknown preset should 400: {body}");
        assert!(body.contains("unknown preset"), "got: {body}");
    }

    #[tokio::test]
    async fn prime_turn_records_history_and_reset_clears_it() {
        // Auth-disabled dev bypass so the call needs no session. A Prime turn (Local brain, no AI
        // configured) records a bounded conversation-history record; the reset endpoint then finds
        // and clears it, and a second reset has nothing left to drop. This pins both that
        // `run_prime` records the turn and that the reset endpoint works end-to-end.
        let (state, _dir) = auth_state(true);
        let (prime_status, _, _) = call(
            &state,
            "POST",
            "/v1/relux/prime",
            None,
            Some("{\"message\":\"what is going on?\"}"),
        )
        .await;
        assert_eq!(prime_status, StatusCode::OK);

        let (reset_status, _, reset_body) =
            call(&state, "POST", "/v1/relux/prime/reset", None, None).await;
        assert_eq!(reset_status, StatusCode::OK);
        assert!(reset_body.contains("\"cleared\":true"), "got: {reset_body}");

        // Nothing left after the first reset.
        let (_, _, again) = call(&state, "POST", "/v1/relux/prime/reset", None, None).await;
        assert!(again.contains("\"cleared\":false"), "got: {again}");
    }

    #[tokio::test]
    async fn setup_then_session_unlocks_then_logout_relocks() {
        let (state, _dir) = auth_state(false);
        // status before setup → needs_setup.
        let (_, _, status_body) = call(&state, "GET", "/v1/auth/status", None, None).await;
        assert!(status_body.contains("\"needs_setup\":true"), "got: {status_body}");

        // First-run setup mints a session cookie.
        let (s, set_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let cookie = session_pair(&set_cookie.expect("setup sets a cookie"));
        assert!(cookie.starts_with("relux_session="), "got: {cookie}");

        // The same protected route now succeeds WITH the session cookie.
        let (ok, _, _) = call(&state, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(ok, StatusCode::OK);

        // A second setup is refused — setup is first-run only.
        let (dup, _, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(dup, StatusCode::CONFLICT);

        // Logout drops the session; the protected route 401s again.
        let (lo, _, _) = call(&state, "POST", "/v1/auth/logout", Some(&cookie), None).await;
        assert_eq!(lo, StatusCode::OK);
        let (relocked, _, _) =
            call(&state, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(relocked, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_session_survives_a_serve_restart_and_logout_persists() {
        // Simulate stop/restart of `serve`: rebuild a brand-new AppState over the
        // SAME on-disk db/admin/session files. A cookie minted against the first
        // boot must still authenticate against the second — no re-login — because
        // the session table is now restart-persistent. Logout must likewise persist.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("local.db");
        let admin = dir.path().join("dashboard-admin.json");
        let sessions = dir.path().join("dashboard-sessions.json");
        {
            let mut store = SqliteStore::open(&db_path).expect("open");
            let mut kernel = store.load().expect("load");
            crate::ensure_bootstrapped(&mut kernel).expect("bootstrap");
            store.save(&kernel).expect("save");
        }
        let make_state = || AppState {
            db_path: db_path.clone(),
            plugins_root: dir.path().join("plugins"),
            uploads_root: dir.path().join("uploads"),
            dashboard_dir: None,
            ai_config_path: dir.path().join("ai.json"),
            dashboard_auth: relux_kernel::DashboardAuth::from_paths(&admin, &sessions),
            agent_tokens: relux_kernel::AgentTokenStore::from_path(
                &dir.path().join("dashboard-agent-tokens.json"),
            ),
            auth_disabled: false,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
            live_run_logs: relux_kernel::LiveRunLogs::new(),
            run_cancellations: relux_kernel::RunCancellations::new(),
        };

        // Boot 1: first-run setup mints a session cookie.
        let boot1 = make_state();
        let (s, set_cookie, _) = call(
            &boot1,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let cookie = session_pair(&set_cookie.expect("setup sets a cookie"));

        // Boot 2: a brand-new AppState recreated from the same files (the restart).
        // The cookie still unlocks a protected route with no re-login.
        let boot2 = make_state();
        let (ok, _, _) = call(&boot2, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(
            ok,
            StatusCode::OK,
            "the same cookie must authenticate after a restart"
        );

        // Logout on boot 2 persists the removal: a third boot rejects the cookie.
        let (lo, _, _) = call(&boot2, "POST", "/v1/auth/logout", Some(&cookie), None).await;
        assert_eq!(lo, StatusCode::OK);
        let boot3 = make_state();
        let (relocked, _, _) =
            call(&boot3, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(
            relocked,
            StatusCode::UNAUTHORIZED,
            "logout must persist across a restart"
        );
    }

    #[tokio::test]
    async fn deleting_the_session_file_revokes_cookies_without_a_restart() {
        // End-to-end proof of the reset-admin / no-restart guarantee: ONE running
        // AppState (never rebuilt). Login, confirm a protected route is open, then
        // delete the session file out of band — as `reset-admin` does — and the very
        // next request through the real auth middleware must 401.
        let (state, dir) = auth_state(false);
        let sessions = dir.path().join("dashboard-sessions.json");
        let (_, _, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        let (login, set_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(login, StatusCode::OK);
        let cookie = session_pair(&set_cookie.expect("login sets a cookie"));
        // The cookie unlocks a protected route.
        let (ok, _, _) = call(&state, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(ok, StatusCode::OK);
        assert!(sessions.exists(), "a live session writes the file");

        // Out-of-band revocation (what `reset-admin` does): delete the file. No
        // restart, no new AppState — the same process must notice on the next call.
        std::fs::remove_file(&sessions).unwrap();
        let (revoked, _, _) =
            call(&state, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(
            revoked,
            StatusCode::UNAUTHORIZED,
            "the running server must reject the old cookie once the session file is cleared"
        );
        // A fresh login still works on the same running process, and persists only
        // the new session (no resurrected rows from before the delete).
        let (relogin, new_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(relogin, StatusCode::OK);
        let fresh = session_pair(&new_cookie.expect("re-login sets a cookie"));
        let (ok2, _, _) = call(&state, "GET", "/v1/relux/state", Some(&fresh), None).await;
        assert_eq!(ok2, StatusCode::OK);
    }

    #[tokio::test]
    async fn login_rejects_wrong_password_and_accepts_the_right_one() {
        let (state, _dir) = auth_state(false);
        // Configure the admin via setup first.
        let (_, _, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        // Wrong password → 401, no cookie.
        let (bad, bad_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"nope"}"#),
        )
        .await;
        assert_eq!(bad, StatusCode::UNAUTHORIZED);
        assert!(bad_cookie.is_none(), "a failed login must not set a session");
        // Right password → 200 + a fresh session that unlocks the API.
        let (good, good_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(good, StatusCode::OK);
        let cookie = session_pair(&good_cookie.expect("login sets a cookie"));
        let (ok, _, _) = call(&state, "GET", "/v1/relux/tools", Some(&cookie), None).await;
        assert_eq!(ok, StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_disabled_bypass_opens_the_api_for_dev_test() {
        let (state, _dir) = auth_state(true);
        // No session, yet a protected route succeeds because the bypass is on.
        let (ok, _, _) = call(&state, "GET", "/v1/relux/state", None, None).await;
        assert_eq!(ok, StatusCode::OK);
        // status advertises the disabled state so the SPA renders the app.
        let (_, _, body) = call(&state, "GET", "/v1/auth/status", None, None).await;
        assert!(body.contains("\"auth_disabled\":true"), "got: {body}");
        assert!(body.contains("\"authenticated\":true"), "got: {body}");
        // The change-password route refuses while the bypass is on (it would
        // rewrite a credential the bypass ignores). 400, not a silent success.
        let (cp, _, _) = call(
            &state,
            "POST",
            "/v1/auth/change-password",
            None,
            Some(r#"{"current_password":"x","new_password":"newpassword1"}"#),
        )
        .await;
        assert_eq!(cp, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn change_password_requires_a_session() {
        let (state, _dir) = auth_state(false);
        // Configure the admin via setup, but present NO session cookie.
        let (_, _, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        // The route is protected: without a session it is 401, never a change.
        let (no_sess, _, _) = call(
            &state,
            "POST",
            "/v1/auth/change-password",
            None,
            Some(r#"{"current_password":"hunter2pass","new_password":"newpassword1"}"#),
        )
        .await;
        assert_eq!(no_sess, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn change_password_over_http_swaps_creds_and_old_login_fails() {
        let (state, _dir) = auth_state(false);
        // Setup mints a session cookie we ride for the authenticated change.
        let (_, set_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        let cookie = session_pair(&set_cookie.expect("setup sets a cookie"));

        // Wrong current password → 401, nothing changed, and the secret is not
        // echoed back in the error body.
        let (wrong, _, wbody) = call(
            &state,
            "POST",
            "/v1/auth/change-password",
            Some(&cookie),
            Some(r#"{"current_password":"nope","new_password":"newpassword1"}"#),
        )
        .await;
        assert_eq!(wrong, StatusCode::UNAUTHORIZED);
        assert!(!wbody.contains("newpassword1"), "must not echo the new password");

        // Too-short new password → 400.
        let (short, _, _) = call(
            &state,
            "POST",
            "/v1/auth/change-password",
            Some(&cookie),
            Some(r#"{"current_password":"hunter2pass","new_password":"short"}"#),
        )
        .await;
        assert_eq!(short, StatusCode::BAD_REQUEST);

        // A correct change → 200; the current session still works afterward.
        let (ok, _, obody) = call(
            &state,
            "POST",
            "/v1/auth/change-password",
            Some(&cookie),
            Some(r#"{"current_password":"hunter2pass","new_password":"newpassword1"}"#),
        )
        .await;
        assert_eq!(ok, StatusCode::OK);
        assert!(!obody.contains("newpassword1") && !obody.contains("argon2"), "got: {obody}");
        let (still_ok, _, _) =
            call(&state, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(still_ok, StatusCode::OK);

        // The old password no longer logs in; the new one does.
        let (old, _, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(old, StatusCode::UNAUTHORIZED);
        let (new, new_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"newpassword1"}"#),
        )
        .await;
        assert_eq!(new, StatusCode::OK);
        assert!(new_cookie.is_some(), "new password must mint a session");
    }

    #[tokio::test]
    async fn successful_protected_request_refreshes_the_session_cookie() {
        let (state, _dir) = auth_state(false);
        // Setup mints the first session cookie.
        let (_, set_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        let cookie = session_pair(&set_cookie.expect("setup sets a cookie"));

        // A successful protected request re-emits the cookie (rolling session).
        let (ok, refreshed, _) =
            call(&state, "GET", "/v1/relux/state", Some(&cookie), None).await;
        assert_eq!(ok, StatusCode::OK);
        let refreshed = refreshed.expect("a successful protected request refreshes the cookie");
        // Same opaque session id (the window slides; the id is not rotated)...
        assert_eq!(session_pair(&refreshed), cookie, "the session id must be stable");
        // ...still HttpOnly with a positive idle Max-Age.
        assert!(refreshed.contains("HttpOnly"), "got: {refreshed}");
        assert!(
            refreshed.contains(&format!("Max-Age={}", relux_kernel::SESSION_TTL_SECS)),
            "the refreshed cookie carries the full idle window; got: {refreshed}"
        );
    }

    #[tokio::test]
    async fn unauthenticated_or_expired_protected_request_sets_no_cookie() {
        let (state, _dir) = auth_state(false);
        // Configure the admin so the 401 path is "needs login", not "needs setup".
        let (_, _, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        // No cookie → 401 and crucially NO Set-Cookie (a failed auth never mints
        // or refreshes a session).
        let (no_sess, no_cookie, _) =
            call(&state, "GET", "/v1/relux/state", None, None).await;
        assert_eq!(no_sess, StatusCode::UNAUTHORIZED);
        assert!(no_cookie.is_none(), "a rejected request must not set a cookie");
        // A bogus/expired session id → 401, still no Set-Cookie.
        let (bad, bad_cookie, _) = call(
            &state,
            "GET",
            "/v1/relux/state",
            Some("relux_session=deadbeef"),
            None,
        )
        .await;
        assert_eq!(bad, StatusCode::UNAUTHORIZED);
        assert!(bad_cookie.is_none(), "an invalid session must not set a cookie");
    }

    #[tokio::test]
    async fn auth_me_exposes_safe_session_expiry_metadata() {
        let (state, _dir) = auth_state(false);
        // Setup mints a session cookie.
        let (_, set_cookie, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        let cookie = session_pair(&set_cookie.expect("setup sets a cookie"));

        let (ok, me_cookie, body) = call(&state, "GET", "/v1/auth/me", Some(&cookie), None).await;
        assert_eq!(ok, StatusCode::OK);
        // /v1/auth/me is PUBLIC (outside the sliding middleware): reading it must
        // NOT refresh the session, so no Set-Cookie rides the response.
        assert!(
            me_cookie.is_none(),
            "reading /v1/auth/me must not slide/refresh the session"
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("me body is json");
        assert_eq!(v["username"], "ops");
        // The policy windows are surfaced verbatim from the kernel constants.
        assert_eq!(v["idle_timeout_secs"].as_i64(), Some(relux_kernel::SESSION_TTL_SECS));
        assert_eq!(
            v["absolute_max_secs"].as_i64(),
            Some(relux_kernel::SESSION_ABSOLUTE_MAX_SECS)
        );
        // A fresh session's remaining idle window is ~the full idle timeout, and
        // the absolute remaining is ~the full absolute cap (allow a couple secs of
        // test execution slack).
        let idle_left = v["idle_expires_in_secs"].as_i64().expect("idle remaining");
        assert!(
            (idle_left - relux_kernel::SESSION_TTL_SECS).abs() <= 3,
            "idle remaining should be ~the full window; got {idle_left}"
        );
        let abs_left = v["absolute_expires_in_secs"].as_i64().expect("absolute remaining");
        assert!(
            (abs_left - relux_kernel::SESSION_ABSOLUTE_MAX_SECS).abs() <= 3,
            "absolute remaining should be ~the full cap; got {abs_left}"
        );
        // Absolute instants are present and ordered, and the server clock is shown.
        let idle_at = v["idle_expires_at"].as_i64().expect("idle_expires_at");
        let abs_at = v["absolute_expires_at"].as_i64().expect("absolute_expires_at");
        let now = v["server_now"].as_i64().expect("server_now");
        assert!(idle_at <= abs_at, "idle deadline must be within the absolute ceiling");
        assert!(now <= idle_at, "server_now must be before the idle deadline");
        // CRUCIAL: no secret ever rides this body — not the session id/cookie, not
        // the admin hash.
        let sid = cookie.trim_start_matches("relux_session=");
        assert!(!body.contains(sid), "the session id must never appear in /v1/auth/me");
        assert!(!body.contains("argon2"), "no password hash may appear in /v1/auth/me");

        // Unauthenticated → 401, no metadata leaked.
        let (no_sess, _, _) = call(&state, "GET", "/v1/auth/me", None, None).await;
        assert_eq!(no_sess, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn reauth_logout_then_login_resets_the_absolute_window_and_kills_the_old_session() {
        // End-to-end proof of the Account "Sign out and sign back in" path (the one
        // way to clear the hard absolute ceiling): a logout invalidates the old
        // session server-side, and the subsequent login mints a FRESH session whose
        // absolute window is reset (>= the first session's, and ~the full cap), while
        // the old cookie stays dead. Mirrors the live-kernel e2e smoke deterministically.
        let (state, _dir) = auth_state(false);
        // First-run setup mints session S1.
        let (s, set1, _) = call(
            &state,
            "POST",
            "/v1/auth/setup",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let s1 = session_pair(&set1.expect("setup sets a cookie"));

        // S1's absolute window is fresh (~the full cap) and admits the API.
        let (ok1, _, me1) = call(&state, "GET", "/v1/auth/me", Some(&s1), None).await;
        assert_eq!(ok1, StatusCode::OK);
        let v1: serde_json::Value = serde_json::from_str(&me1).expect("me1 json");
        let abs_at_1 = v1["absolute_expires_at"].as_i64().expect("abs_at_1");
        let abs_left_1 = v1["absolute_expires_in_secs"].as_i64().expect("abs_left_1");
        assert!(
            (abs_left_1 - relux_kernel::SESSION_ABSOLUTE_MAX_SECS).abs() <= 3,
            "S1 should open a full absolute window; got {abs_left_1}"
        );
        let (gate1, _, _) = call(&state, "GET", "/v1/relux/state", Some(&s1), None).await;
        assert_eq!(gate1, StatusCode::OK);

        // Account "Sign out": logout drops S1.
        let (lo, _, _) = call(&state, "POST", "/v1/auth/logout", Some(&s1), None).await;
        assert_eq!(lo, StatusCode::OK);
        // The OLD cookie is now dead — both the protected API and the status read
        // reject it (server-side invalidation, not just a cleared browser jar).
        let (gate_dead, _, _) = call(&state, "GET", "/v1/relux/state", Some(&s1), None).await;
        assert_eq!(gate_dead, StatusCode::UNAUTHORIZED);
        let (me_dead, _, _) = call(&state, "GET", "/v1/auth/me", Some(&s1), None).await;
        assert_eq!(me_dead, StatusCode::UNAUTHORIZED);

        // Re-login mints a DISTINCT session S2.
        let (li, set2, _) = call(
            &state,
            "POST",
            "/v1/auth/login",
            None,
            Some(r#"{"username":"ops","password":"hunter2pass"}"#),
        )
        .await;
        assert_eq!(li, StatusCode::OK);
        let s2 = session_pair(&set2.expect("login sets a cookie"));
        assert_ne!(s2, s1, "re-login must mint a new opaque session id, not reuse the old one");

        // S2's absolute window is reset: at least as far out as S1's (never inheriting
        // a shrunk remainder) and ~the full cap again.
        let (ok2, _, me2) = call(&state, "GET", "/v1/auth/me", Some(&s2), None).await;
        assert_eq!(ok2, StatusCode::OK);
        let v2: serde_json::Value = serde_json::from_str(&me2).expect("me2 json");
        let abs_at_2 = v2["absolute_expires_at"].as_i64().expect("abs_at_2");
        let abs_left_2 = v2["absolute_expires_in_secs"].as_i64().expect("abs_left_2");
        assert!(
            abs_at_2 >= abs_at_1,
            "the re-auth must push the absolute ceiling forward (or equal), never backward: {abs_at_2} < {abs_at_1}"
        );
        assert!(
            (abs_left_2 - relux_kernel::SESSION_ABSOLUTE_MAX_SECS).abs() <= 3,
            "S2 should re-open a full absolute window; got {abs_left_2}"
        );
        // S2 admits the API; the old S1 cookie is STILL dead (re-auth did not revive it).
        let (gate2, _, _) = call(&state, "GET", "/v1/relux/state", Some(&s2), None).await;
        assert_eq!(gate2, StatusCode::OK);
        let (still_dead, _, _) = call(&state, "GET", "/v1/relux/state", Some(&s1), None).await;
        assert_eq!(still_dead, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_me_reports_dev_identity_under_the_bypass() {
        let (state, _dir) = auth_state(true);
        let (ok, _, body) = call(&state, "GET", "/v1/auth/me", None, None).await;
        assert_eq!(ok, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).expect("me body is json");
        assert_eq!(v["username"], "dev");
        assert_eq!(v["auth_disabled"], true);
    }
}
