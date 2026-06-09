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

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use relux_core::{
    InstalledPlugin, PluginManifest, PluginSourceKind, PrimeAutonomyConfig, PrimeAutonomyTickResult,
    PrimeTurn, ToolInvocationResult,
};
use relux_kernel::{
    install_from_dir, install_from_github, install_from_zip, remove_plugin, AiConfig, AiMode,
    AiStatus, KernelError, KernelState, SqliteStore,
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
    ai_config: AiConfig,
    lock: Arc<Mutex<()>>,
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
        ai_config: AiConfig::from_env(),
        lock: Arc::new(Mutex::new(())),
    };

    // Bootstrap + persist once so a fresh store already lists the bundled
    // example plugins before the first request arrives.
    locked_save(&state, |_kernel| Ok(()))
        .map_err(|e| KernelError::Storage(format!("bootstrap failed: {}", e.message)))?;

    let addr = bind_addr()?;
    let dashboard_missing = state.dashboard_dir.is_none();
    let app = router(state.clone()); // Clone state for the background task

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| KernelError::Storage(format!("failed to bind {addr}: {e}")))?;
    let bound = listener
        .local_addr()
        .map_err(|e| KernelError::Storage(format!("failed to read bound address: {e}")))?;

    println!("relux-kernel serve: Relux local control plane is up.");
    println!();
    println!("   Relux dashboard: http://{bound}/dashboard");
    println!("   Relux API:       http://{bound}/v1/relux/state");
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
    println!("   GET    /v1/relux/state");
    println!("   GET    /v1/relux/ai/status");
    println!("   GET    /v1/relux/tasks");
    println!("   GET    /v1/relux/tasks/:id");
    println!("   GET    /v1/relux/runs");
    println!("   GET    /v1/relux/runs/:id");
    println!("   GET    /v1/relux/runs/:id/events");
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
    println!("   DELETE /v1/relux/plugins/:id");

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

/// Resolve the bind address from `RELUX_HTTP_ADDR`, falling back to loopback.
fn bind_addr() -> Result<SocketAddr, KernelError> {
    let raw = match std::env::var("RELUX_HTTP_ADDR") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_ADDR.to_string(),
    };
    raw.parse::<SocketAddr>()
        .map_err(|e| KernelError::Storage(format!("invalid RELUX_HTTP_ADDR {raw:?}: {e}")))
}

/// Assemble the `/v1/relux` router with the shared state.
fn router(state: AppState) -> Router {
    Router::new()
        // Standalone Relux dashboard shell, served by the kernel itself.
        .route("/", get(root_redirect))
        .route("/dashboard", get(dashboard_index))
        .route("/dashboard/", get(dashboard_index))
        .route("/dashboard/*path", get(dashboard_path))
        // The /v1/relux control-plane API the dashboard calls on the same origin.
        .route("/v1/relux/state", get(get_state))
        .route("/v1/relux/ai/status", get(get_ai_status))
        .route("/v1/relux/agents", get(list_agents).post(create_agent))
        .route("/v1/relux/prime", post(run_prime))
        .route("/v1/relux/prime/autonomy", get(get_autonomy_config).put(update_autonomy_config).patch(update_autonomy_config))
        .route("/v1/relux/prime/autonomy/tick", post(run_autonomy_tick))
        .route("/v1/relux/tasks", get(list_tasks).post(create_task))
        .route("/v1/relux/tasks/:id", get(get_task))
        .route("/v1/relux/runs", get(list_runs))
        .route("/v1/relux/runs/:id", get(get_run))
        .route("/v1/relux/runs/:id/events", get(get_run_events))
        .route("/v1/relux/audit", get(list_audit_events))
        .route("/v1/relux/health", get(get_health))
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
        .route("/v1/relux/plugins/:id", delete(remove))
        // Relux Approvals and Permissions
        .route("/v1/relux/approvals", get(list_approvals))
        .route("/v1/relux/approvals/:id/decide", post(decide_approval))
        .route("/v1/relux/permissions", get(list_permissions))
        .route("/v1/relux/agents/:id/permissions", post(grant_agent_permission))
        // Bound the request body so a large zip upload is refused cleanly.
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(state)
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
    Json(state.ai_config.status())
}

async fn list_agents(State(state): State<AppState>) -> Result<Json<Vec<AgentRecord>>, ApiError> {
    let records = locked_read(&state, |kernel| {
        Ok(kernel.agents().into_iter().map(agent_record).collect())
    })?;
    Ok(Json(records))
}

async fn list_plugins(
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginRecord>>, ApiError> {
    let records = locked_read(&state, |kernel| Ok(plugin_records(kernel)))?;
    Ok(Json(records))
}

/// Optional `?agent=<id>` scoping for tool discovery: when supplied, each tool's
/// executable status reflects whether THAT agent holds the permission
/// (`ready`/`missing_permission`); when absent, discovery is permission-agnostic.
#[derive(Debug, Deserialize)]
struct ToolsQuery {
    agent: Option<String>,
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
    let tools = locked_read(&state, |kernel| {
        Ok(kernel.discover_tools(agent_id.as_ref()))
    })?;
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

        let task_title = kernel
            .task(&run.task_id)
            .map(|t| t.title.clone());

        Ok(RunRecord { run, task_title })
    })?;
    Ok(Json(record))
}

async fn get_run_events(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Vec<relux_kernel::RunEvent>>, ApiError> {
    let run_id = relux_core::RunId::new(id);
    let events = locked_read(&state, |kernel| {
        // Check if the run exists to return 404 if not.
        kernel
            .run(&run_id)
            .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
        Ok(kernel.run_events(&run_id).into_iter().cloned().collect())
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
    let run_id = locked_save(&state, |kernel| {
        let status = kernel
            .task(&task_id)
            .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?
            .status
            .clone();
        if matches!(
            status,
            relux_core::TaskStatus::Created | relux_core::TaskStatus::Queued
        ) {
            kernel.start_run(&task_id)?;
        }
        kernel.execute_local_run(&task_id)
    })?;
    Ok(Json(ExecuteAssignedTaskResponse { run_id }))
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

    // 1. Run the deterministic kernel turn (must happen under the lock).
    let (turn, summary) = {
        let _guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = SqliteStore::open(&state.db_path)?;
        let mut kernel = store.load()?;
        let ctx = crate::ensure_bootstrapped(&mut kernel)?;
        let turn = kernel.prime_turn(&ctx, &message)?;
        let summary = state_response(&kernel, &state.db_path);
        store.save(&kernel)?;
        (turn, summary)
    };

    // 2. Shape the reply (LLM or deterministic fallback). This happens OUTSIDE
    // the lock because it might involve a slow network call.
    let outcome = relux_kernel::shape_reply(&state.ai_config, &message, &turn).await;

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
    let ai_status = state.ai_config.status();

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

/// One run, flattened for the dashboard table.
#[derive(Debug, Serialize)]
struct RunRecord {
    #[serde(flatten)]
    run: relux_core::Run,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_title: Option<String>,
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
        | KernelError::UnknownAgent(_) => StatusCode::NOT_FOUND,
        KernelError::BundledPluginProtected(_) => StatusCode::CONFLICT,
        KernelError::RuntimeNotConfigured { .. } => StatusCode::NOT_FOUND,
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
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
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
    }
}
