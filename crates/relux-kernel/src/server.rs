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
    /// Dev/test-only escape hatch: when `RELUX_AUTH_DISABLED` is truthy the auth
    /// middleware passes every request through (a loud warning is printed at
    /// startup). OFF by default — production/normal use always enforces login.
    auth_disabled: bool,
    lock: Arc<Mutex<()>>,
    /// In-process registry of non-blocking orchestration jobs. Lives only for the
    /// life of the server process: a restart honestly loses in-flight job records
    /// (the durable orchestration record still carries the real per-brief progress
    /// recorded round-by-round). See [`JobRegistry`].
    jobs: JobRegistry,
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
        auth_disabled: auth_disabled_from_env(),
        lock: Arc::new(Mutex::new(())),
        jobs: JobRegistry::default(),
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
    println!("   GET    /v1/relux/ai/status");
    println!("   PUT    /v1/relux/ai/config                 {{ \"provider\":\"openrouter\", \"api_key\":\"...\", \"model\"?, \"disabled\"? }}");
    println!("   DELETE /v1/relux/ai/config                 (clear the stored AI key/config)");
    println!("   GET    /v1/relux/tasks");
    println!("   GET    /v1/relux/tasks/:id");
    println!("   GET    /v1/relux/runs");
    println!("   GET    /v1/relux/runs/:id");
    println!("   GET    /v1/relux/runs/:id/events            (optional ?since=<event_id> tail)");
    println!("   POST   /v1/relux/runs/:id/proposed-changes/:index/review {{ \"decision\": \"approve|reject\" }}");
    println!("   POST   /v1/relux/runs/:id/proposed-changes/:index/apply");
    println!("   GET    /v1/relux/audit");
    println!("   GET    /v1/relux/health");
    println!("   POST   /v1/relux/prime                     {{ \"message\": \"...\" }}");
    println!("   POST   /v1/relux/tasks                     {{ \"title\": \"...\" }}");
    println!("   POST   /v1/relux/tasks/:id/start");
    println!("   POST   /v1/relux/tasks/:id/execute-assigned");
    println!("   GET    /v1/relux/tools                      (installed tools + executable status)");
    println!("   POST   /v1/relux/tools/invoke              {{ \"plugin_id\":\"...\", \"tool_name\":\"...\", \"input\":{{}} }}");
    println!("   GET    /v1/relux/plugins");
    println!("   POST   /v1/relux/plugins/install-github   {{ \"url\": \"https://github.com/...\" }}");
    println!("   POST   /v1/relux/plugins/install-zip      (multipart field: file)");
    println!("   GET    /v1/relux/plugins/:id/runtime      (HTTP loopback runtime status)");
    println!("   PUT    /v1/relux/plugins/:id/runtime      {{ \"base_url\":\"http://127.0.0.1:<port>\", \"enabled\"?, \"timeout_ms\"? }}");
    println!("   DELETE /v1/relux/plugins/:id/runtime      (clear runtime config)");
    println!("   GET    /v1/relux/plugins/:id/manifest-template  (starter relux-plugin.json)");
    println!("   DELETE /v1/relux/plugins/:id");
    println!("   GET    /v1/relux/adapters                  (adapter plugins + CLI runtime status)");
    println!("   POST   /v1/relux/prime/orchestrations/:id/run-async  (start a background job; returns job + status_url)");
    println!("   GET    /v1/relux/prime/orchestrations/:id/job         (latest job for this orchestration)");
    println!("   GET    /v1/relux/orchestration-jobs/:job_id           (poll one job's status)");
    println!("   POST   /v1/relux/orchestration-jobs/:job_id/cancel    (request cancellation; stops before the next round)");
    println!("   GET    /v1/relux/adapters/:id/runtime");
    println!("   PUT    /v1/relux/adapters/:id/runtime     {{ \"enabled\":true, \"command\"?, \"timeout_seconds\"?, \"max_output_bytes\"? }}");
    println!("   DELETE /v1/relux/adapters/:id/runtime     (clear adapter runtime config)");

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
    public_router()
        .merge(protected)
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
        .route("/v1/relux/ai/status", get(get_ai_status))
        .route(
            "/v1/relux/ai/config",
            put(set_ai_config).patch(set_ai_config).delete(clear_ai_config),
        )
        .route("/v1/relux/agents", get(list_agents).post(create_agent))
        .route("/v1/relux/prime", post(run_prime))
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
        .route("/v1/relux/runs/:id/retry", post(retry_run))
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
        // Relux Approvals and Permissions
        .route("/v1/relux/approvals", get(list_approvals))
        .route("/v1/relux/approvals/:id/decide", post(decide_approval))
        .route("/v1/relux/permissions", get(list_permissions))
        .route("/v1/relux/agents/:id/permissions", post(grant_agent_permission))
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
        Ok(kernel.agents().into_iter().map(agent_record).collect())
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

async fn list_approvals(
    State(state): State<AppState>,
) -> Result<Json<Vec<relux_core::Approval>>, ApiError> {
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
        Ok(all_approvals)
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
) -> Result<Json<relux_core::Approval>, ApiError> {
    let approval_id = relux_core::ApprovalId::new(id);
    let approve = match req.decision.as_str() {
        "approved" => true,
        "rejected" => false,
        _ => return Err(ApiError::bad_request("decision must be 'approved' or 'rejected'")),
    };

    let approval = locked_save(&state, |kernel| {
        // TODO: Pass actual user or Prime agent id for approver
        kernel.resolve_approval(&approval_id, approve, "dashboard_user", req.note)?;
        Ok(kernel.approval(&approval_id).cloned().unwrap())
    })?;
    Ok(Json(approval))
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

#[derive(Debug, Deserialize)]
struct CreateTaskReq {
    title: String,
}

#[derive(Debug, Deserialize)]
struct CreateAgentReq {
    id: Option<String>,
    name: String,
    role: Option<String>,
    adapter_plugin: Option<String>,
}

async fn create_agent(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentReq>,
) -> Result<Json<AgentRecord>, ApiError> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("name is required"));
    }

    let agent_id_str = match req.id {
        Some(id) if !id.trim().is_empty() => id.trim().to_string(),
        _ => name.to_lowercase().replace(' ', "-"), // Sanitize/derive id if omitted
    };

    let description = req.role.unwrap_or_default();
    let adapter_plugin = req
        .adapter_plugin
        .unwrap_or_else(|| "relux-adapter-local-prime".to_string());
    let adapter_plugin_id = relux_core::PluginId::new(adapter_plugin);

    let agent = locked_save(&state, |kernel| {
        let ctx = crate::ensure_bootstrapped(kernel)?;
        if kernel.agent(&relux_core::AgentId::new(&agent_id_str)).is_some() {
            return Err(KernelError::AgentExists(agent_id_str.clone()));
        }

        // Grant minimal safe permissions for MVP
        let permissions = vec![relux_core::Permission::new("tool:relux-tools-echo:say").unwrap()];

        let id = kernel.create_agent(
            &agent_id_str,
            &name,
            &description,
            &adapter_plugin_id,
            &ctx.namespace,
            None, // persona
            permissions,
        )?;
        Ok(agent_record(kernel.agent(&id).unwrap()))
    })?;
    Ok(Json(agent))
}

async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskReq>,
) -> Result<Json<relux_core::Task>, ApiError> {
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::bad_request("title is required"));
    }
    let task = locked_save(&state, |kernel| {
        let ctx = crate::ensure_bootstrapped(kernel)?;
        let id = kernel.create_task(
            &title,
            serde_json::json!({}),
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

    // 1. Run the deterministic kernel turn (must happen under the lock). While we
    // hold the lock, also snapshot the runtime status of the brain's CLI adapter
    // (if any), so the spawn below can happen outside the lock.
    let (turn, summary, cli_status) = {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = SqliteStore::open(&state.db_path)?;
        let mut kernel = store.load()?;
        let ctx = crate::ensure_bootstrapped(&mut kernel)?;
        let turn = kernel.prime_turn(&ctx, &message)?;
        let summary = state_response(&kernel, &state.db_path);
        let cli_status = cli_adapter_id.and_then(|id| {
            kernel
                .adapter_runtime_status()
                .into_iter()
                .find(|a| a.plugin_id == id)
        });
        store.save(&kernel)?;
        (turn, summary, cli_status)
    };

    // 2. Produce the conversational reply through the selected brain. Actions are
    // never delegated: an actionful turn (a real state change / approval / tool
    // result) always keeps the grounded deterministic reply. Conversational turns
    // route to the chosen brain. This happens OUTSIDE the lock because it can
    // involve a slow network/process call.
    let outcome = if !relux_kernel::is_actionful(&turn)
        && matches!(
            brain,
            relux_kernel::PrimeBrain::ClaudeCli | relux_kernel::PrimeBrain::CodexCli
        ) {
        run_cli_brain(brain, cli_status, &message, &turn).await
    } else {
        // Local / OpenRouter (and actionful turns) go through shape_reply, which
        // keeps actionful turns deterministic and only augments via OpenRouter
        // when that brain is selected and a key is configured.
        relux_kernel::shape_reply(&ai_config, &message, &turn).await
    };

    // 3. Merge the outcome into the response.
    let mut final_turn = turn;
    final_turn.reply = outcome.reply;

    Ok(Json(PrimeResponse {
        turn: final_turn,
        state: summary,
        ai_mode: outcome.mode,
        ai_model: outcome.model,
        ai_note: outcome.note,
    }))
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

    let prompt = relux_kernel::compose_chat_prompt(message, &turn.reply);
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

    // Phase 2: run the prepared adapter processes in parallel with the lock RELEASED,
    // through the SAME shared spawn primitive the synchronous kernel driver uses.
    let finished = relux_kernel::run_briefs_in_parallel(prepared);

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
    /// Whether the dashboard should offer a Retry action: a failed run whose task
    /// still exists and is still assigned.
    retryable: bool,
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
    let retryable = run.status == relux_core::RunStatus::Failed
        && task.map(|t| t.assigned_agent.is_some()).unwrap_or(false);
    RunRecord {
        run,
        task_title,
        phase,
        output_excerpt,
        failure_reason,
        retryable,
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
    created_at: String,
}

/// Build a [`AgentRecord`] from an `Agent`.
fn agent_record(agent: &relux_core::Agent) -> AgentRecord {
    AgentRecord {
        id: agent.id.as_str().to_string(),
        name: agent.name.clone(),
        description: agent.description.clone(),
        adapter_plugin: agent.adapter_plugin.as_str().to_string(),
        namespace: agent.namespace_id.as_str().to_string(),
        status: format!("{:?}", agent.status),
        permissions_summary: format!("{} permissions", agent.permissions.len()),
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
        | KernelError::UnknownPlugin(_)
        | KernelError::UnknownAgent(_)
        | KernelError::UnknownOrchestration(_) => StatusCode::NOT_FOUND,
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
            artifacts,
            proposed_changes: Vec::new(),
        }
    }

    fn record_of(run: relux_core::Run) -> RunRecord {
        RunRecord {
            run,
            task_title: Some("a task".into()),
            phase: Some("run_completed".into()),
            output_excerpt: None,
            failure_reason: None,
            retryable: false,
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
        }
    }

    #[tokio::test]
    async fn cli_brain_not_installed_falls_back_with_actionable_note() {
        // No adapter status at all -> keep the grounded reply, tell the operator
        // exactly what to do. Never blank, never a fabricated Claude answer.
        let turn = conversational_turn("There is 1 active run.");
        let outcome =
            run_cli_brain(relux_kernel::PrimeBrain::ClaudeCli, None, "hey", &turn).await;
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
            auth_disabled: false,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
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
            auth_disabled: false,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
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
            auth_disabled,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
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
            auth_disabled: false,
            lock: Arc::new(Mutex::new(())),
            jobs: JobRegistry::default(),
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
