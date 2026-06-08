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
use relux_core::{AgentId, NamespaceId, Permission, PluginId, PrimeContext};
use relux_kernel::{load_plugin_manifests, KernelError, KernelState, SqliteStore};

/// The stable ids the local control plane is bootstrapped with.
const WORKSPACE_NS: &str = "workspace";
const PRIME_AGENT: &str = "prime";
const PRIME_ADAPTER: &str = "relux-adapter-local-prime";

fn main() -> ExitCode {
    // CLI:
    //   relux-kernel                     -> deterministic in-memory demo loop
    //   relux-kernel prime <message...>  -> one Prime turn against PERSISTENT state
    //   relux-kernel state               -> summarize the persistent store
    //   relux-kernel reset-local         -> wipe + reinit the local dev DB
    //
    // The `prime`/`state`/`reset-local` paths share one durable SQLite store so
    // state survives across invocations (`docs/RELUX_MASTER_PLAN.md` section 15 Phase 1,
    // section 17.8). The no-arg demo stays fully in-memory and deterministic.
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
        Some((cmd, _)) if cmd == "reset-local" => run_reset_local(),
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

/// Resolve the local dev database path: `$RELUX_DB` if set and non-empty,
/// otherwise `dev-data/relux/local.db` (already gitignored).
fn db_path() -> PathBuf {
    match std::env::var("RELUX_DB") {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => PathBuf::from("dev-data/relux/local.db"),
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
        for manifest in load_plugin_manifests(&examples_dir())? {
            kernel.register_plugin(manifest);
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
        "plugins={} namespaces={} agents={} tasks={} runs={} approvals={}",
        kernel.plugin_count(),
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

fn run_demo() -> Result<(), KernelError> {
    let mut kernel = KernelState::new();

    println!("== Relux kernel: first local control-plane loop ==\n");

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

    let echo_plugin = PluginId::new("relux-tools-echo");
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

    // 6. Prime calls the echo tool through the kernel. The kernel checks the
    //    permission, routes to the ToolSet plugin, and the tool echoes the input.
    let input = serde_json::json!({ "message": "hello relux" });
    let output = kernel.call_tool(&run, &prime, &echo_plugin, "echo.say", input)?;
    println!("[6] Prime called echo.say -> {output}");

    // 7. Complete the run and the task.
    kernel.complete_run(&run, "echo.say returned the input unchanged")?;
    kernel.complete_task(&task)?;
    println!("[7] Completed run {run} and task {task}\n");

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
        "-- State summary --\n   plugins={} namespaces={} agents={} tasks={} runs={}",
        kernel.plugin_count(),
        kernel.namespace_count(),
        kernel.agent_count(),
        kernel.task_count(),
        kernel.run_count()
    );

    Ok(())
}
