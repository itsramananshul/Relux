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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use relux_core::{InstalledPlugin, PluginManifest, PluginSourceKind};
use relux_kernel::{
    install_from_dir, install_from_github, install_from_zip, remove_plugin, KernelError,
    KernelState, SqliteStore,
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
        lock: Arc::new(Mutex::new(())),
    };

    // Bootstrap + persist once so a fresh store already lists the bundled
    // example plugins before the first request arrives.
    locked_save(&state, |_kernel| Ok(()))
        .map_err(|e| KernelError::Storage(format!("bootstrap failed: {}", e.message)))?;

    let addr = bind_addr()?;
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| KernelError::Storage(format!("failed to bind {addr}: {e}")))?;
    let bound = listener
        .local_addr()
        .map_err(|e| KernelError::Storage(format!("failed to read bound address: {e}")))?;

    println!("relux-kernel serve: listening on http://{bound}");
    println!("   db      {}", crate::db_path().display());
    println!("   plugins {}", crate::plugins_root().display());
    println!("   GET    /v1/relux/state");
    println!("   GET    /v1/relux/plugins");
    println!("   POST   /v1/relux/plugins/install-dir     {{ \"path\": \"...\" }}");
    println!("   POST   /v1/relux/plugins/install-github   {{ \"url\": \"https://github.com/...\" }}");
    println!("   POST   /v1/relux/plugins/install-zip      (multipart field: file)");
    println!("   DELETE /v1/relux/plugins/:id");

    axum::serve(listener, app)
        .await
        .map_err(|e| KernelError::Storage(format!("server error: {e}")))?;
    Ok(())
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
        .route("/v1/relux/state", get(get_state))
        .route("/v1/relux/plugins", get(list_plugins))
        .route("/v1/relux/plugins/install-dir", post(install_dir))
        .route("/v1/relux/plugins/install-github", post(install_github))
        .route("/v1/relux/plugins/install-zip", post(install_zip))
        .route("/v1/relux/plugins/:id", delete(remove))
        // Bound the request body so a large zip upload is refused cleanly.
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(state)
}

// --- Handlers --------------------------------------------------------------

async fn get_state(State(state): State<AppState>) -> Result<Json<StateResponse>, ApiError> {
    let resp = locked_read(&state, |kernel| Ok(state_response(kernel, &state.db_path)))?;
    Ok(Json(resp))
}

async fn list_plugins(
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginRecord>>, ApiError> {
    let records = locked_read(&state, |kernel| Ok(plugin_records(kernel)))?;
    Ok(Json(records))
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
        KernelError::PluginNotInstalled(_) => StatusCode::NOT_FOUND,
        KernelError::BundledPluginProtected(_) => StatusCode::CONFLICT,
        KernelError::UnsafePluginPath(_)
        | KernelError::PluginInstall(_)
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
    }
}
