//! `relux-kernel` demo binary - drives the first local control-plane loop.
//!
//! Running `cargo run -p relux-kernel` walks the MVP loop from
//! `docs/RELUX_MASTER_PLAN.md` section 14 / section 16 end to end against an in-memory
//! [`KernelState`], using only the two static example plugin manifests under
//! `examples/relux-plugins/`. It is fully deterministic: no network, no wall
//! clock, no real API calls, so the printed output is identical every run.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use relux_core::namespace::NamespaceKind;
use relux_core::{
    AgentId, NamespaceId, Permission, PluginId, PluginSourceKind, PrimeContext, TaskId, TaskStatus,
};
use relux_kernel::{
    install_from_dir, install_from_github, install_from_zip, load_plugin_manifests, remove_plugin,
    KernelError, KernelState, SqliteStore,
};

mod dashboard;
mod server;

/// Returns the relux-kernel crate version.
fn get_kernel_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The stable ids the local control plane is bootstrapped with.
const WORKSPACE_NS: &str = "workspace";
const PRIME_AGENT: &str = "prime";
const PRIME_ADAPTER: &str = "relux-adapter-local-prime";

fn main() -> ExitCode {
    // CLI:
    //   relux-kernel                         -> deterministic in-memory demo loop
    //   relux-kernel prime <message...>      -> one Prime turn against PERSISTENT state
    //   relux-kernel state                   -> summarize the persistent store
    //   relux-kernel serve                   -> run the local /v1/relux HTTP API
    //   relux-kernel health|doctor          -> check local health, return zero on PASS
    //   relux-kernel reset-local             -> wipe + reinit the local dev DB
    //   relux-kernel plugins                 -> list installed plugins
    //   relux-kernel plugin install-dir <p>  -> install a plugin from a folder
    //   relux-kernel plugin install-zip <p>  -> install a plugin from a .zip
    //   relux-kernel plugin install-github <url> -> install from a GitHub URL
    //   relux-kernel plugin remove <id>      -> remove an installed plugin
    //
    // The persistent paths share one durable SQLite store so state survives
    // across invocations (`docs/RELUX_MASTER_PLAN.md` section 15 Phase 1, section 17.8). The
    // no-arg demo stays fully in-memory and deterministic.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((cmd, rest)) if cmd == "prime" => {
            let message = rest.join(" ");
            if message.trim().is_empty() {
                eprintln!("usage: relux-kernel prime <message>");
                return ExitCode::FAILURE;
            }
            run_prime_message(&message)
        }
        Some((cmd, _)) if cmd == "state" => run_state(),
        Some((cmd, _)) if cmd == "serve" => server::run(),
        Some((cmd, _)) if cmd == "health" || cmd == "doctor" => {
            let exit_code = run_health();
            if exit_code == ExitCode::SUCCESS {
                Ok(())
            } else {
                Err(KernelError::Storage(format!("Health check failed with code {:?}", exit_code)))
            }
        }
        Some((cmd, _)) if cmd == "reset-local" => run_reset_local(),
        Some((cmd, _)) if cmd == "plugins" => run_plugins_list(),
        Some((cmd, rest)) if cmd == "plugin" => run_plugin_subcommand(rest),
        Some((cmd, rest)) if cmd == "task" => run_task_subcommand(rest),
        _ => run_demo(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("relux-kernel failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_task_subcommand(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        Some((sub, rest)) if sub == "run-assigned" => {
            let task_id_str = first_arg(rest, "task run-assigned <task_id>")?;
            run_assigned_task(&task_id_str)
        }
        _ => Err(KernelError::Storage(
            "usage: relux-kernel task <run-assigned> <task_id>".to_string(),
        )),
    }
}

fn run_assigned_task(task_id_str: &str) -> Result<(), KernelError> {
    let task_id = TaskId::new(task_id_str);
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;

    let status = kernel
        .task(&task_id)
        .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?
        .status
        .clone();
    if matches!(status, TaskStatus::Created | TaskStatus::Queued) {
        kernel.start_run(&task_id)?;
    }
    let run_id = kernel.execute_local_run(&task_id)?;
    store.save(&kernel)?;

    println!("Successfully executed task {} as assigned agent. New run: {}", task_id, run_id);
    Ok(())
}

/// Print local Relux health. Exits 0 on PASS, 1 on WARN, 2 on FAIL.
fn run_health() -> ExitCode {
    let mut exit_code = ExitCode::SUCCESS;
    let mut warnings = vec![];
    let mut errors = vec![];

    println!("== Relux kernel health ({}) ==", get_kernel_version());

    // DB path and status
    let db_path = db_path();
    let store_result = SqliteStore::open(&db_path);
    match store_result {
        Ok(store) => {
            println!("PASS: DB path: {}", db_path.display());
            let kernel_result = store.load();
            match kernel_result {
                Ok(kernel) => {
                    println!("PASS: DB loaded successfully.");
                    println!("   Installed plugins: {}", kernel.installed_plugin_count());
                    println!("   Agents: {}", kernel.agent_count());
                    println!("   Tasks: {}", kernel.task_count());
                    println!("   Runs: {}", kernel.run_count());
                    println!("   Approvals: {}", kernel.approval_count());
                }
                Err(e) => {
                    errors.push(format!("FAIL: Failed to load kernel state from DB: {}", e));
                    exit_code = ExitCode::from(2);
                }
            }
        }
        Err(e) => {
            errors.push(format!("FAIL: Failed to open DB at {}: {}", db_path.display(), e));
            exit_code = ExitCode::from(2);
        }
    }

    // Dashboard bundle status
    let dashboard_dir = crate::dashboard::resolve_dist_dir();
    if let Some(path) = dashboard_dir {
        println!("PASS: Dashboard bundle present at {}", path.display());
    } else {
        warnings.push("WARN: Dashboard bundle not found. Run `npm run build` in `apps/dashboard`".to_string());
        if exit_code == ExitCode::SUCCESS {
            exit_code = ExitCode::from(1);
        }
    }

    // AI status
    let ai_config = relux_kernel::AiConfig::from_env();
    let ai_status = ai_config.status();
    match ai_status.mode {
        relux_kernel::AiMode::Openrouter => {
            if ai_status.configured {
                println!("PASS: AI mode: OpenRouter (configured)");
            } else {
                warnings.push("WARN: AI mode: OpenRouter (not configured, set RELUX_OPENROUTER_API_KEY)".to_string());
                if exit_code == ExitCode::SUCCESS {
                    exit_code = ExitCode::from(1);
                }
            }
        }
        relux_kernel::AiMode::Deterministic => {
            println!("INFO: AI mode: Deterministic (no OpenRouter config found)");
        }
        relux_kernel::AiMode::DeterministicForAction => {
            println!("INFO: AI mode: Deterministic (for action)");
        }
    }

    // Output warnings and errors
    if !warnings.is_empty() {
        println!("
--- Warnings ---");
        for warn in &warnings {
            println!("{}", warn);
        }
    }
    if !errors.is_empty() {
        println!("
--- Errors ---");
        for err in &errors {
            println!("{}", err);
        }

    }

    let label = if exit_code == ExitCode::SUCCESS {
        "PASS"
    } else if errors.is_empty() {
        "WARN"
    } else {
        "FAIL"
    };
    println!("
Health check complete. Status: {label}");
    exit_code
}

/// Resolve the local dev database path: `$RELUX_DB` if set and non-empty,
/// otherwise `dev-data/relux/local.db` (already gitignored).
fn db_path() -> PathBuf {
    match std::env::var("RELUX_DB") {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => PathBuf::from("dev-data/relux/local.db"),
    }
}

/// The durable root for installed plugin directories: a `plugins/` folder next
/// to the local dev database (`dev-data/relux/plugins/<plugin-id>` by default,
/// already gitignored). Spec ref: `docs/RELUX_MASTER_PLAN.md` section 7.4.
fn plugins_root() -> PathBuf {
    let db = db_path();
    match db.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("plugins"),
        _ => PathBuf::from("dev-data/relux/plugins"),
    }
}

/// The durable root for staged plugin uploads: an `uploads/` folder next to the
/// local dev database (`dev-data/relux/uploads` by default, already gitignored).
/// The HTTP `install-zip` route writes each upload here, installs it, then
/// removes the temp file.
fn uploads_root() -> PathBuf {
    let db = db_path();
    match db.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("uploads"),
        _ => PathBuf::from("dev-data/relux/uploads"),
    }
}

/// Ensure the loaded `kernel` has the baseline control plane (plugins, the
/// workspace namespace, and the Prime agent), and return the `PrimeContext` to
/// act with. Bootstrapping is keyed on Prime's existence, so it runs exactly
/// once on a fresh store and is skipped on every subsequent load - no duplicate
/// plugin registrations or audit noise.
fn ensure_bootstrapped(kernel: &mut KernelState) -> Result<PrimeContext, KernelError> {
    let ns_id = NamespaceId::new(WORKSPACE_NS);
    let prime_id = AgentId::new(PRIME_AGENT);

    if kernel.agent(&prime_id).is_none() {
        // Mark the shipped example plugins as installed/enabled with source kind
        // Bundled (`docs/RELUX_MASTER_PLAN.md` section 9.4). Keyed on Prime's absence,
        // so this runs exactly once on a fresh store - no duplicate installed
        // records or audit noise on later loads.
        let dir = examples_dir();
        for manifest in load_plugin_manifests(&dir)? {
            let install_dir = dir.join(manifest.id.as_str()).display().to_string();
            kernel.install_plugin(
                manifest,
                PluginSourceKind::Bundled,
                "bundled example".to_string(),
                install_dir,
                true,
            );
        }
        kernel.create_namespace(WORKSPACE_NS, "Workspace", NamespaceKind::Personal);
        let echo_permission = Permission::new("tool:relux-tools-echo:say")
            .expect("static echo permission is well-formed");
        kernel.create_agent(
            PRIME_AGENT,
            "Prime",
            "The Relux control-plane operator.",
            &PluginId::new(PRIME_ADAPTER),
            &ns_id,
            Some(
                "You are Prime: understand intent, act through the kernel, never bypass permissions."
                    .to_string(),
            ),
            vec![echo_permission],
        )?;
    }

    Ok(PrimeContext {
        namespace: ns_id,
        agent: prime_id,
        actor: "founder".to_string(),
    })
}

/// Load durable state, run exactly one Prime turn on `message`, and save.
///
/// This is the CLI seam for `relux-kernel prime <message>`: it honors the
/// message the user provided instead of replaying a fixed script, so a greeting
/// stays a greeting and "create a task to X" creates exactly that task
/// (`docs/RELUX_MASTER_PLAN.md` section 10, section 16) - and, unlike before, the result
/// persists so the next invocation sees it.
fn run_prime_message(message: &str) -> Result<(), KernelError> {
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    let prime_ctx = ensure_bootstrapped(&mut kernel)?;

    let turn = kernel.prime_turn(&prime_ctx, message)?;
    store.save(&kernel)?;

    println!("   db    > {}", path.display());
    println!("   you   > {message}");
    println!(
        "   prime [{:?}/{:?}] {}",
        turn.intent, turn.disposition, turn.reply
    );

    Ok(())
}

/// Print a concise summary of the persistent control plane.
fn run_state() -> Result<(), KernelError> {
    let path = db_path();
    let store = SqliteStore::open(&path)?;
    let kernel = store.load()?;
    let summary = kernel.inspect_state();

    println!("== Relux local state ({}) ==", path.display());
    println!(
        "plugins={} installed_plugins={} namespaces={} agents={} tasks={} runs={} approvals={}",
        kernel.plugin_count(),
        kernel.installed_plugin_count(),
        kernel.namespace_count(),
        kernel.agent_count(),
        kernel.task_count(),
        kernel.run_count(),
        kernel.approval_count(),
    );
    println!(
        "open_tasks={} active_runs={} waiting_approval={} blocked={} failed={} pending_approvals={}",
        summary.tasks_open,
        summary.runs_active,
        summary.tasks_waiting_approval,
        summary.tasks_blocked,
        summary.tasks_failed,
        summary.pending_approvals,
    );
    if !summary.queued.is_empty() {
        println!("-- queued --");
        for t in &summary.queued {
            println!("   {} [{:?}] {}", t.id, t.status, t.title);
        }
    }
    if !summary.recent.is_empty() {
        println!("-- recent --");
        for t in &summary.recent {
            println!("   {} [{:?}] {}", t.id, t.status, t.title);
        }
    }

    Ok(())
}

/// Delete and reinitialize the local dev database. LOCAL DEV CONVENIENCE ONLY -
/// this is a destructive wipe of `db_path()`, intended for resetting a scratch
/// store between experiments, never for any shared/production data.
fn run_reset_local() -> Result<(), KernelError> {
    let path = db_path();
    println!("reset-local: LOCAL DEV ONLY - wiping and reinitializing the local Relux dev store.");
    println!("   target db: {}", path.display());

    // Remove the database and any SQLite sidecar files (-wal / -shm).
    for suffix in ["", "-wal", "-shm"] {
        let p = sidecar(&path, suffix);
        if p.exists() {
            std::fs::remove_file(&p)
                .map_err(|e| KernelError::Storage(format!("remove {}: {e}", p.display())))?;
            println!("   removed {}", p.display());
        }
    }

    // Recreate a fresh, bootstrapped store so the next command finds it ready.
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    store.save(&kernel)?;
    println!("   reinitialized with plugins, the workspace namespace, and the Prime agent.");

    Ok(())
}

/// Build the path for a SQLite sidecar by appending `suffix` to the db filename.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return path.to_path_buf();
    }
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Resolve the example-plugins directory.
///
/// Prefer `./examples/relux-plugins` relative to the current working directory
/// (the documented path), but fall back to the location relative to this
/// crate's manifest so `cargo run -p relux-kernel` works from anywhere in the
/// workspace.
fn examples_dir() -> PathBuf {
    let cwd_path = PathBuf::from("examples/relux-plugins");
    if cwd_path.is_dir() {
        return cwd_path;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/relux-plugins")
}

/// List installed plugins from the persistent store.
///
/// Ensures the store is bootstrapped first so a fresh DB shows the bundled
/// example plugins; the (idempotent) bootstrap result is saved so the bundled
/// install records are durable.
fn run_plugins_list() -> Result<(), KernelError> {
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    store.save(&kernel)?;

    let installed = kernel.installed_plugins();
    println!("== Relux installed plugins ({}) ==", installed.len());
    if installed.is_empty() {
        println!("   (none)");
    }
    for p in installed {
        println!(
            "   {:<28} v{:<8} {:<10} {:<8} {}",
            p.id,
            p.version,
            format!("{:?}", p.source_kind),
            if p.enabled { "enabled" } else { "disabled" },
            p.source_label,
        );
    }
    Ok(())
}

/// Dispatch `relux-kernel plugin <subcommand> ...`.
fn run_plugin_subcommand(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        Some((sub, rest)) if sub == "install-dir" => {
            let path = first_arg(rest, "plugin install-dir <path>")?;
            run_plugin_install_dir(&path)
        }
        Some((sub, rest)) if sub == "install-zip" => {
            let path = first_arg(rest, "plugin install-zip <path>")?;
            run_plugin_install_zip(&path)
        }
        Some((sub, rest)) if sub == "install-github" => {
            let url = first_arg(rest, "plugin install-github <url>")?;
            run_plugin_install_github(&url)
        }
        Some((sub, rest)) if sub == "remove" => {
            let id = first_arg(rest, "plugin remove <plugin-id>")?;
            run_plugin_remove(&id)
        }
        _ => Err(KernelError::PluginInstall(
            "usage: relux-kernel plugin <install-dir|install-zip|install-github|remove> <arg>"
                .to_string(),
        )),
    }
}

/// Return the first argument, or a usage error naming the expected form.
fn first_arg(rest: &[String], usage: &str) -> Result<String, KernelError> {
    match rest.first() {
        Some(a) if !a.trim().is_empty() => Ok(a.clone()),
        _ => Err(KernelError::PluginInstall(format!("usage: relux-kernel {usage}"))),
    }
}

/// Open the persistent store, ensure bootstrap, run `action`, save, and report.
fn with_persistent_kernel<F>(action: F) -> Result<(), KernelError>
where
    F: FnOnce(&mut KernelState) -> Result<String, KernelError>,
{
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    let message = action(&mut kernel)?;
    store.save(&kernel)?;
    println!("{message}");
    Ok(())
}

fn run_plugin_install_dir(path: &str) -> Result<(), KernelError> {
    let root = plugins_root();
    with_persistent_kernel(|kernel| {
        let installed = install_from_dir(Path::new(path), &root, kernel)?;
        Ok(format!(
            "installed {} v{} ({:?}) from {} -> {}",
            installed.id,
            installed.version,
            installed.source_kind,
            installed.source_label,
            installed.install_dir
        ))
    })
}

fn run_plugin_install_zip(path: &str) -> Result<(), KernelError> {
    let root = plugins_root();
    with_persistent_kernel(|kernel| {
        let installed = install_from_zip(Path::new(path), &root, kernel)?;
        Ok(format!(
            "installed {} v{} ({:?}) from {} -> {}",
            installed.id,
            installed.version,
            installed.source_kind,
            installed.source_label,
            installed.install_dir
        ))
    })
}

fn run_plugin_install_github(url: &str) -> Result<(), KernelError> {
    let root = plugins_root();
    with_persistent_kernel(|kernel| {
        let installed = install_from_github(url, &root, kernel)?;
        Ok(format!(
            "installed {} v{} ({:?}) from {} -> {}",
            installed.id,
            installed.version,
            installed.source_kind,
            installed.source_label,
            installed.install_dir
        ))
    })
}

fn run_plugin_remove(plugin_id: &str) -> Result<(), KernelError> {
    let root = plugins_root();
    with_persistent_kernel(|kernel| {
        remove_plugin(plugin_id, &root, kernel)?;
        Ok(format!("removed plugin {plugin_id}"))
    })
}

fn run_demo() -> Result<(), KernelError> {
    let mut kernel = KernelState::new();

    println!("== Relux kernel: first local control-plane loop ==
");

    // 1. Load the static example plugin manifests and register them.
    let dir = examples_dir();
    let manifests = load_plugin_manifests(&dir)?;
    println!(
        "[1] Loaded {} plugin manifest(s) from {}:",
        manifests.len(),
        dir.display()
    );
    for manifest in &manifests {
        println!(
            "    - {} ({:?}, v{}) - {}",
            manifest.id, manifest.kind, manifest.version, manifest.description
        );
    }
    for manifest in manifests {
        kernel.register_plugin(manifest);
    }
    println!();


    let prime_adapter = PluginId::new("relux-adapter-local-prime");
    let echo_permission = Permission::new("tool:relux-tools-echo:say")
        .expect("static echo permission is well-formed");

    // 2. Create a namespace (a personal workspace scope).
    let workspace = kernel.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
    println!("[2] Created namespace: {workspace}");

    // 3. Create Prime as an agent backed by the local-prime adapter manifest.
    //    Prime is granted exactly the echo tool permission - least privilege.
    let prime = kernel.create_agent(
        "prime",
        "Prime",
        "The Relux control-plane operator.",
        &prime_adapter,
        &workspace,
        Some(
            "You are Prime: understand intent, act through the kernel, never bypass permissions."
                .to_string(),
        ),
        vec![echo_permission.clone()],
    )?;
    println!("[3] Created Prime agent: {prime} (adapter {prime_adapter})");

    // 4. Create a task and assign it to Prime.
    let task = kernel.create_task(
        "Check the echo tool responds",
        serde_json::json!({ "message": "hello relux" }),
        "founder",
        &workspace,
        vec![echo_permission],
    );
    kernel.assign_task(&task, &prime)?;
    println!("[4] Created task {task} and assigned it to {prime}");

    // 5. Start a run for the task (inherits Prime's adapter).
    let run = kernel.start_run(&task)?;
    println!("[5] Started run {run}");

    // 6. Execute the run locally using the new mechanism (echo tool, complete run/task).
    kernel.execute_local_run(&task)?; // This implicitly performs echo.say, completes run, and task
    println!("[6] Executed run {run} locally as assigned agent (echo.say, completed).");

    // No separate step 7 needed, as execute_local_run completes both run and task.
    println!("[7] Completed run {run} and task {task} (via local execution)
");

    // --- Prime chat: the first Prime Core slice (master plan section 10, section 16) ----
    //
    // The same kernel now drives Prime as a grounded, Codex-like operator: it
    // classifies intent, inspects state, acts within scope, and gates risky
    // actions behind approval - all deterministically, no LLM. Greetings stay
    // greetings (section 17.1); task creation and "start it" walk the loop; a permission
    // grant is only proposed, never silently performed (section 10.3).
    println!("-- Prime chat --");
    let prime_ctx = PrimeContext {
        namespace: workspace.clone(),
        agent: prime.clone(),
        actor: "founder".to_string(),
    };
    let script = [
        "hey",
        "what is going on?",
        "create a task to summarize the README",
        "start it",
        "give the code agent GitHub access",
        "why did it fail?",
    ];
    for message in script {
        let turn = kernel.prime_turn(&prime_ctx, message)?;
        println!("   you   > {message}");
        println!(
            "   prime [{:?}/{:?}] {}",
            turn.intent, turn.disposition, turn.reply
        );
    }
    println!();

    // --- Show the resulting control-plane state ---------------------------
    println!("-- Run transcript ({run}) --");
    for event in kernel.run_events(&run) {
        println!(
            "   {}  {:<18} {:<8} {}",
            event.ts, event.kind, event.source, event.message
        );
    }
    println!();

    println!("-- Audit log ({} events) --", kernel.audit_log().len());
    for event in kernel.audit_log() {
        let ns = event
            .namespace_id
            .as_ref()
            .map(|n| n.as_str())
            .unwrap_or("-");
        println!(
            "   {}  {:<10} {:<28} {:<8} ns={}",
            event.ts,
            event.actor_id,
            event.action,
            format!("{:?}", event.result),
            ns
        );
    }
    println!();

    println!(
        "-- State summary --
   plugins={} namespaces={} agents={} tasks={} runs={}",
        kernel.plugin_count(),
        kernel.namespace_count(),
        kernel.agent_count(),
        kernel.task_count(),
        kernel.run_count()
    );

    Ok(())
}
