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
    AgentId, NamespaceId, OrchestrationId, Permission, PluginId, PluginSourceKind, PrimeContext,
    PrimeTurn, TaskId, ToolExecutability,
};
use relux_kernel::{
    install_from_dir, install_from_github, install_from_zip, load_plugin_manifests,
    refresh_bundled_plugins, remove_plugin, KernelError, KernelState, SqliteStore,
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
    //   relux-kernel reset-admin [user] [pw] -> local operator password recovery
    //                                           (rewrites dashboard-admin.json;
    //                                           generates a password if omitted)
    //   relux-kernel plugins                 -> list installed plugins
    //   relux-kernel plugin install-dir <p>  -> install a plugin from a folder
    //   relux-kernel plugin install-zip <p>  -> install a plugin from a .zip
    //   relux-kernel plugin install-github <url> -> install from a GitHub URL
    //   relux-kernel plugin remove <id>      -> remove an installed plugin
    //   relux-kernel plugin runtime <id>     -> show a plugin's HTTP loopback runtime
    //   relux-kernel plugin runtime set <id> <url> [--timeout-ms N] -> configure+enable
    //   relux-kernel plugin runtime disable <id> -> disable a plugin's runtime
    //   relux-kernel tools                   -> list installed tools + executable status
    //   relux-kernel tool invoke <plugin> <tool> [json] -> invoke a built-in tool
    //   relux-kernel adapters                -> list adapter plugins + runtime status
    //   relux-kernel adapter runtime <id>    -> show one adapter's CLI runtime
    //   relux-kernel adapter runtime enable <id> [--timeout-seconds N] [--max-output-bytes N] [--command C] [--working-dir D]
    //   relux-kernel adapter runtime disable <id> -> disable an adapter's CLI runtime
    //
    // The persistent paths share one durable SQLite store so state survives
    // across invocations (`docs/RELUX_MASTER_PLAN.md` section 15 Phase 1, section 17.8). The
    // no-arg demo stays fully in-memory and deterministic.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((cmd, rest)) if cmd == "prime" => match rest.split_first() {
            Some((prime_sub, prime_rest)) if prime_sub == "autonomy" => run_prime_autonomy(prime_rest),
            Some((prime_sub, prime_rest)) if prime_sub == "orchestrate" => {
                run_prime_orchestrate(prime_rest)
            }
            Some((prime_sub, prime_rest)) if prime_sub == "orchestration" => {
                run_prime_orchestration(prime_rest)
            }
            _ => {
                let message = rest.join(" ");
                if message.trim().is_empty() {
                    eprintln!("usage: relux-kernel prime <message>");
                    return ExitCode::FAILURE;
                }
                run_prime_message(&message)
            }
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
        Some((cmd, rest)) if cmd == "reset-admin" => run_reset_admin(rest),
        Some((cmd, _)) if cmd == "plugins" => run_plugins_list(),
        Some((cmd, rest)) if cmd == "plugin" => run_plugin_subcommand(rest),
        Some((cmd, _)) if cmd == "tools" => run_tools_list(),
        Some((cmd, rest)) if cmd == "tool" => run_tool_subcommand(rest),
        Some((cmd, _)) if cmd == "adapters" => run_adapters_list(),
        Some((cmd, rest)) if cmd == "adapter" => run_adapter_subcommand(rest),
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
        Some((sub, rest)) if sub == "retry-run" => {
            let run_id_str = first_arg(rest, "task retry-run <run_id>")?;
            retry_run_cli(&run_id_str)
        }
        _ => Err(KernelError::Storage(
            "usage: relux-kernel task <run-assigned <task_id> | retry-run <run_id>>".to_string(),
        )),
    }
}

/// Dispatches `relux-kernel prime autonomy <subcommand> ...`.
fn run_prime_autonomy(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        Some((sub, _)) if sub == "status" => run_autonomy_status(),
        Some((sub, _)) if sub == "enable" => run_autonomy_enable(),
        Some((sub, _)) if sub == "disable" => run_autonomy_disable(),
        Some((sub, _)) if sub == "tick" => run_autonomy_tick(),
        Some((sub, rest)) if sub == "configure" => run_autonomy_configure(rest),
        _ => Err(KernelError::Storage(
            "usage: relux-kernel prime autonomy <status|enable|disable|tick|configure>".to_string(),
        )),
    }
}

fn run_autonomy_status() -> Result<(), KernelError> {
    with_persistent_kernel(|kernel| {
        let config = &kernel.prime_autonomy_config;
        let mut output = "Prime Autonomy Status:\n".to_string();
        output.push_str(&format!("  Enabled: {}\n", config.enabled));
        output.push_str(&format!("  Interval: {} seconds\n", config.interval_seconds));
        output.push_str(&format!("  Max Tasks per Tick: {}\n", config.max_tasks_per_tick));
        output.push_str(&format!("  Auto-assign Unassigned Tasks: {}\n", config.auto_assign_unassigned));
        if let Some(last_tick_at) = &config.last_tick_at {
            output.push_str(&format!("  Last Tick At: {}\n", last_tick_at));
        } else {
            output.push_str("  Last Tick At: Never\n");
        }
        if let Some(ref last_tick_summary) = config.last_tick_summary {
            output.push_str(&format!("  Last Tick Summary: {}\n", last_tick_summary));
        } else {
            output.push_str("  Last Tick Summary: (empty)\n");
        }
        Ok(output)
    })
}

fn run_autonomy_enable() -> Result<(), KernelError> {
    with_persistent_kernel(|kernel| {
        kernel.prime_autonomy_config.enabled = true;
        Ok("Prime autonomy enabled.".to_string())
    })
}

fn run_autonomy_disable() -> Result<(), KernelError> {
    with_persistent_kernel(|kernel| {
        kernel.prime_autonomy_config.enabled = false;
        Ok("Prime autonomy disabled.".to_string())
    })
}

fn run_autonomy_tick() -> Result<(), KernelError> {
    with_persistent_kernel(|kernel| {
        let result = kernel.one_autonomy_tick();
        let mut output = "Prime Autonomy Manual Tick Result:\n".to_string();
        output.push_str(&format!("  Summary: {}\n", result.summary));
        output.push_str(&format!("  Tasks Run: {}\n", result.tasks_run));
        output.push_str(&format!("  Tasks Assigned: {}\n", result.tasks_assigned));
        if !result.skipped_reasons.is_empty() {
            output.push_str("  Skipped Reasons:\n");
            for reason in result.skipped_reasons {
                output.push_str(&format!("    - {}\n", reason));
            }
        }
        Ok(output)
    })
}

fn run_autonomy_configure(args: &[String]) -> Result<(), KernelError> {
    let mut interval: Option<u64> = None;
    let mut max_tasks: Option<u32> = None;
    let mut auto_assign: Option<bool> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--interval" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| KernelError::Storage("Missing value for --interval".to_string()))?;
                interval = Some(val.parse().map_err(|_| KernelError::Storage(format!("Invalid interval: {}", val)))?);
            },
            "--max-tasks" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| KernelError::Storage("Missing value for --max-tasks".to_string()))?;
                max_tasks = Some(val.parse().map_err(|_| KernelError::Storage(format!("Invalid max-tasks: {}", val)))?);
            },
            "--auto-assign" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| KernelError::Storage("Missing value for --auto-assign".to_string()))?;
                auto_assign = Some(val.parse().map_err(|_| KernelError::Storage(format!("Invalid auto-assign: {}", val)))?);
            },
            _ => return Err(KernelError::Storage(format!("Unknown argument: {}", args[i]))),
        }
        i += 1;
    }

    with_persistent_kernel(|kernel| {
        let mut config_changed = false;
        if let Some(val) = interval {
            kernel.prime_autonomy_config.interval_seconds = val.clamp(5, 3600);
            config_changed = true;
        }
        if let Some(val) = max_tasks {
            kernel.prime_autonomy_config.max_tasks_per_tick = val.clamp(1, 25);
            config_changed = true;
        }
        if let Some(val) = auto_assign {
            kernel.prime_autonomy_config.auto_assign_unassigned = val;
            config_changed = true;
        }

        if config_changed {
            Ok("Prime autonomy configuration updated.".to_string())
        } else {
            Ok("No changes applied to Prime autonomy configuration.".to_string())
        }
    })
}

/// `relux-kernel prime orchestrate "<goal>"` - decompose a goal into multiple
/// role-typed briefs assigned to fitting agents, then print the plan. Creates work
/// but does not run it; use `prime orchestration run <id>` to execute.
fn run_prime_orchestrate(args: &[String]) -> Result<(), KernelError> {
    let goal = args.join(" ");
    let goal = goal.trim().to_string();
    if goal.is_empty() {
        return Err(KernelError::Storage(
            "usage: relux-kernel prime orchestrate \"<goal with multiple steps>\"".to_string(),
        ));
    }
    with_persistent_kernel(|kernel| {
        let ctx = ensure_bootstrapped(kernel)?;
        match kernel.prime_orchestrate(&ctx, &goal) {
            Ok(record) => {
                let mut out = format!(
                    "Created orchestration {} for \"{}\" with {} briefs:\n",
                    record.id,
                    record.goal,
                    record.steps.len()
                );
                for step in &record.steps {
                    out.push_str(&format!(
                        "  - {} \"{}\" -> {} ({}) [{}]\n",
                        step.task_id,
                        step.title,
                        step.agent_id,
                        step.role.label(),
                        step.outcome.label()
                    ));
                }
                for note in &record.notes {
                    out.push_str(&format!("  note: {note}\n"));
                }
                out.push_str(&format!(
                    "Nothing has run yet. Start it with: relux-kernel prime orchestration run {}",
                    record.id
                ));
                Ok(out)
            }
            Err(KernelError::OrchestrationNotMultiAgent) => Ok(
                "That goal reads as a single piece of work. Give it distinct steps (e.g. \"research X, implement Y, and document Z\") or create one task instead."
                    .to_string(),
            ),
            Err(e) => Err(e),
        }
    })
}

/// Dispatches `relux-kernel prime orchestration <subcommand> ...`.
fn run_prime_orchestration(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        Some((sub, _)) if sub == "list" => run_orchestration_list(),
        Some((sub, rest)) if sub == "show" => run_orchestration_show(rest),
        Some((sub, rest)) if sub == "run" => run_orchestration_run(rest),
        _ => Err(KernelError::Storage(
            "usage: relux-kernel prime orchestration <list|show <id>|run <id> [--max N] [--concurrency N]>"
                .to_string(),
        )),
    }
}

fn run_orchestration_list() -> Result<(), KernelError> {
    with_persistent_kernel(|kernel| {
        let list = kernel.orchestrations();
        if list.is_empty() {
            return Ok("No orchestrations yet. Create one with `prime orchestrate \"<goal>\"`.".to_string());
        }
        let mut out = format!("Orchestrations ({}):\n", list.len());
        for o in list {
            out.push_str(&format!(
                "  {} [{}] \"{}\" - {} briefs\n",
                o.id,
                o.status.label(),
                o.goal,
                o.steps.len()
            ));
        }
        Ok(out.trim_end().to_string())
    })
}

fn run_orchestration_show(args: &[String]) -> Result<(), KernelError> {
    let id = args.first().ok_or_else(|| {
        KernelError::Storage("usage: relux-kernel prime orchestration show <id>".to_string())
    })?;
    let oid = OrchestrationId::new(id.clone());
    with_persistent_kernel(|kernel| {
        let o = kernel
            .orchestration(&oid)
            .ok_or_else(|| KernelError::UnknownOrchestration(id.clone()))?;
        let mut out = format!(
            "Orchestration {} [{}]\n  goal: {}\n  briefs:\n",
            o.id,
            o.status.label(),
            o.goal
        );
        for (i, step) in o.steps.iter().enumerate() {
            let deps = if step.depends_on.is_empty() {
                String::new()
            } else {
                let names: Vec<String> = step
                    .depends_on
                    .iter()
                    .filter_map(|&j| o.steps.get(j).map(|d| d.task_id.to_string()))
                    .collect();
                format!(" depends-on [{}]", names.join(", "))
            };
            out.push_str(&format!(
                "    {}. {} \"{}\" -> {} ({}) [{}]{}{}{}\n",
                i,
                step.task_id,
                step.title,
                step.agent_id,
                step.role.label(),
                step.outcome.label(),
                deps,
                step.round.map(|r| format!(" round {r}")).unwrap_or_default(),
                step.run_id
                    .as_ref()
                    .map(|r| format!(" run {r}"))
                    .unwrap_or_default()
            ));
            if let Some(note) = &step.note {
                out.push_str(&format!("      note: {note}\n"));
            }
        }
        if let Some(summary) = &o.last_batch_summary {
            out.push_str(&format!("  last batch: {summary}\n"));
        }
        Ok(out.trim_end().to_string())
    })
}

/// Run a multi-agent orchestration batch to completion from the CLI.
///
/// `--concurrency N` (default 2, clamp 1..=4) is the round size: the independent
/// briefs ready in one round run as real concurrent OS adapter processes via the
/// shared [`relux_kernel::KernelState::run_orchestration`] engine — the same true
/// parallelism the dashboard's background job path uses. This command blocks until
/// the whole batch finishes and then prints the result; it owns the store
/// exclusively for the run (load -> run -> save), so there is no interleaving.
fn run_orchestration_run(args: &[String]) -> Result<(), KernelError> {
    let mut id: Option<String> = None;
    let mut max: usize = 25;
    let mut concurrency: usize = 2;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--max" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| {
                    KernelError::Storage("Missing value for --max".to_string())
                })?;
                max = val
                    .parse()
                    .map_err(|_| KernelError::Storage(format!("Invalid --max: {val}")))?;
            }
            "--concurrency" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| {
                    KernelError::Storage("Missing value for --concurrency".to_string())
                })?;
                concurrency = val
                    .parse()
                    .map_err(|_| KernelError::Storage(format!("Invalid --concurrency: {val}")))?;
            }
            other => {
                if id.is_none() {
                    id = Some(other.to_string());
                } else {
                    return Err(KernelError::Storage(format!("Unexpected argument: {other}")));
                }
            }
        }
        i += 1;
    }
    let id = id.ok_or_else(|| {
        KernelError::Storage(
            "usage: relux-kernel prime orchestration run <id> [--max N] [--concurrency N]"
                .to_string(),
        )
    })?;
    let oid = OrchestrationId::new(id);

    // Persist even on partial failure so blocked/failed step records survive.
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    let result = kernel.run_orchestration(&oid, max, concurrency);
    store.save(&kernel)?;
    let result = result?;

    let mut out = format!(
        "Orchestration batch [{}]: {}\n",
        result.status.label(),
        result.summary
    );
    for line in &result.per_agent {
        out.push_str(&format!("  {line}\n"));
    }
    for reason in &result.skipped_reasons {
        out.push_str(&format!("  skipped: {reason}\n"));
    }
    out.push_str(&format!("Next: {}", result.next_action));
    println!("{out}");
    Ok(())
}

fn run_assigned_task(task_id_str: &str) -> Result<(), KernelError> {
    let task_id = TaskId::new(task_id_str);
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;

    // Dispatch on the assigned agent's adapter: local Prime echoes; an enabled
    // CLI adapter spawns its local binary; anything else fails honestly. The run
    // is persisted either way so the transcript/audit survive.
    let result = kernel.execute_assigned_run(&task_id);
    store.save(&kernel)?;
    let run_id = result?;

    println!("Successfully executed task {} as assigned agent. New run: {}", task_id, run_id);
    Ok(())
}

/// Retry a failed run as a fresh run on the same task (master plan section 10.2
/// `prime.retry_run`). Persists the new run either way so its transcript survives.
fn retry_run_cli(run_id_str: &str) -> Result<(), KernelError> {
    let run_id = relux_core::RunId::new(run_id_str);
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;

    let result = kernel.retry_run(&run_id);
    store.save(&kernel)?;
    let new_run_id = result?;

    println!("Retried run {run_id} as a fresh run on the same task. New run: {new_run_id}");
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
        Ok(mut store) => {
            println!("PASS: DB path: {}", db_path.display());
            let kernel_result = store.load();
            match kernel_result {
                Ok(mut kernel) => {
                    // Idempotently refresh the bundled plugins so an older DB
                    // picks up newly shipped capabilities, then persist. A refresh
                    // failure is a warning (e.g. examples dir absent), never a
                    // hard fail of the health check.
                    match ensure_bootstrapped(&mut kernel).and_then(|_| store.save(&kernel)) {
                        Ok(()) => {}
                        Err(e) => {
                            warnings.push(format!(
                                "WARN: bundled plugin refresh/save skipped: {e}"
                            ));
                            if exit_code == ExitCode::SUCCESS {
                                exit_code = ExitCode::from(1);
                            }
                        }
                    }
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

    // Local operator login status. A configured admin means the dashboard/API
    // require sign-in; no admin yet means the first dashboard load shows the
    // one-time setup form. Neither is a failure — this is informational so the
    // operator knows what to expect (and the dev/test bypass is flagged loudly).
    if auth_disabled_env() {
        warnings.push(
            "WARN: RELUX_AUTH_DISABLED is set — `serve` will leave the dashboard/API OPEN (dev/test only)."
                .to_string(),
        );
        if exit_code == ExitCode::SUCCESS {
            exit_code = ExitCode::from(1);
        }
    } else {
        match relux_kernel::read_admin_username(&admin_path()) {
            Some(user) => println!("PASS: Local login: admin '{user}' configured (dashboard requires sign-in)."),
            None => println!("INFO: Local login: no admin yet — first dashboard load shows the setup form."),
        }
    }

    // AI status: resolve from the dashboard-written secrets file (when present)
    // with environment fallback, so a key configured from the dashboard is
    // honored here too.
    let ai_config = relux_kernel::AiConfig::resolve(Some(&ai_config_path()));
    let ai_status = ai_config.status();
    match ai_status.mode {
        relux_kernel::AiMode::Openrouter => {
            if ai_status.configured {
                println!("PASS: AI mode: OpenRouter (configured)");
            } else {
                warnings.push("WARN: AI mode: OpenRouter (not configured; set a key in the dashboard AI settings or RELUX_OPENROUTER_API_KEY)".to_string());
                if exit_code == ExitCode::SUCCESS {
                    exit_code = ExitCode::from(1);
                }
            }
        }
        relux_kernel::AiMode::Deterministic => {
            println!("INFO: AI mode: Deterministic (no OpenRouter key configured)");
        }
        relux_kernel::AiMode::DeterministicForAction => {
            println!("INFO: AI mode: Deterministic (for action)");
        }
        relux_kernel::AiMode::ClaudeCli => {
            println!("INFO: AI mode: Claude CLI brain (conversational replies via the local `claude` CLI)");
        }
        relux_kernel::AiMode::CodexCli => {
            println!("INFO: AI mode: Codex CLI brain (conversational replies via the local `codex` CLI)");
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

/// The dashboard-written AI provider secrets file: `ai-config.json` next to the
/// local dev database (`dev-data/relux/ai-config.json` by default, already
/// gitignored). It lets an operator configure Prime's OpenRouter key from the
/// dashboard without environment variables; the key is never returned over the
/// API. See `docs/RELUX_MASTER_PLAN.md` "Optional LLM-backed Prime".
fn ai_config_path() -> PathBuf {
    let db = db_path();
    match db.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("ai-config.json"),
        _ => PathBuf::from("dev-data/relux/ai-config.json"),
    }
}

/// The dashboard admin credential file: `dashboard-admin.json` next to the local
/// dev database (`dev-data/relux/dashboard-admin.json` by default, gitignored).
/// `RELUX_ADMIN_FILE` overrides it. Holds the Argon2id password hash for local
/// operator login — never plaintext, never returned by the API. See
/// `docs/RELUX_MASTER_PLAN.md` "Local operator login v1".
fn admin_path() -> PathBuf {
    match std::env::var("RELUX_ADMIN_FILE") {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => relux_kernel::admin_path_for_db(&db_path()),
    }
}

/// Whether the dev/test auth bypass (`RELUX_AUTH_DISABLED`) is requested. Mirrors
/// the same parse the `serve` middleware uses so `doctor` reports it honestly.
fn auth_disabled_env() -> bool {
    matches!(
        std::env::var("RELUX_AUTH_DISABLED")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
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
/// act with.
///
/// The bundled plugin manifests are refreshed idempotently on EVERY load
/// (`docs/RELUX_MASTER_PLAN.md` section 9.4, section 7.4): a fresh store gets the
/// shipped plugins, and a long-lived store picks up newly shipped capabilities
/// (new adapters/tools) without a reset - while never duplicating records,
/// downgrading the protected `Bundled` source, overwriting a user-installed
/// plugin, or touching per-plugin runtime config. Creating the workspace
/// namespace and the Prime agent stays keyed on Prime's absence, so it runs
/// exactly once on a fresh store.
fn ensure_bootstrapped(kernel: &mut KernelState) -> Result<PrimeContext, KernelError> {
    let ns_id = NamespaceId::new(WORKSPACE_NS);
    let prime_id = AgentId::new(PRIME_AGENT);

    // Idempotently reconcile the shipped bundled plugins into the store. Safe to
    // run on every load: it adds missing bundled plugins, updates changed ones in
    // place, and is a no-op (no audit noise) when everything is already current.
    refresh_bundled_plugins(kernel, &examples_dir())?;

    if kernel.agent(&prime_id).is_none() {
        kernel.create_namespace(WORKSPACE_NS, "Workspace", NamespaceKind::Personal);
        // Prime is granted exactly the two safe, built-in tool permissions so it
        // can invoke the bundled echo and status tools through the kernel - least
        // privilege (`docs/RELUX_MASTER_PLAN.md` section 17.5).
        let echo_permission = Permission::new("tool:relux-tools-echo:say")
            .expect("static echo permission is well-formed");
        let status_permission = Permission::new("tool:relux-tools-status:summary")
            .expect("static status permission is well-formed");
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
            vec![echo_permission, status_permission],
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
    print_tool_result(&turn);

    Ok(())
}

/// Print the tool a Prime turn invoked (and its JSON output) or the honest
/// reason a requested tool did not run, when either is present. Nothing prints
/// for a turn that touched no tool.
fn print_tool_result(turn: &PrimeTurn) {
    if let Some(tool) = &turn.invoked_tool {
        println!("   tool  > {tool}");
        if let Some(output) = &turn.tool_output {
            let pretty =
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
            for line in pretty.lines() {
                println!("           {line}");
            }
        }
    } else if let Some(err) = &turn.tool_error {
        println!("   tool  > (not run) {err}");
    }
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

/// **Local operator password recovery.** Rewrite the dashboard admin credential
/// with a fresh Argon2id hash, using the SAME storage as first-run setup
/// (`docs/RELUX_MASTER_PLAN.md` "Local operator login v1").
///
/// Usage: `relux-kernel reset-admin [username] [password]`.
/// - If `username` is omitted, the existing admin username is kept (or `admin`
///   when no admin exists yet).
/// - If `password` is omitted, a strong random password is generated and PRINTED
///   once (the only time it is ever shown). The existing secret is never read or
///   printed.
///
/// This is a local CLI/filesystem operation only — there is NO network or
/// unauthenticated reset path. It does not weaken session auth or touch any other
/// state. In-memory sessions are not invalidated here; **restart
/// `relux-kernel serve`** to drop live sessions (a restart also reloads this new
/// credential).
fn run_reset_admin(args: &[String]) -> Result<(), KernelError> {
    let path = admin_path();

    // Resolve the username: explicit arg > existing admin username > "admin".
    let username = match args.first() {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => relux_kernel::read_admin_username(&path).unwrap_or_else(|| "admin".to_string()),
    };

    // Resolve the password: explicit arg > a freshly generated strong one.
    let (password, generated) = match args.get(1) {
        Some(p) if !p.trim().is_empty() => (p.clone(), false),
        _ => (generate_password(), true),
    };

    relux_kernel::reset_admin_credential(&path, &username, &password)
        .map_err(|e| KernelError::Storage(format!("reset-admin failed: {e}")))?;

    println!("reset-admin: local operator credential rewritten.");
    println!("   file:     {}", path.display());
    println!("   username: {username}");
    if generated {
        println!("   password: {password}");
        println!("   ^ this generated password is shown ONCE. Save it now.");
    } else {
        println!("   password: (set from the value you provided)");
    }
    println!();
    println!("Restart `relux-kernel serve` to drop any live sessions and load this");
    println!("credential, then sign in to the dashboard with the new password.");
    Ok(())
}

/// Generate a strong, copy-pasteable random password (no ambiguous characters).
/// Used by `reset-admin` when the operator does not supply one.
fn generate_password() -> String {
    use rand::RngCore;
    // Avoid 0/O/1/l/I to keep the printed password unambiguous to retype.
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
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
/// Ensures the store is bootstrapped first, which idempotently refreshes the
/// bundled plugin manifests, so a fresh DB shows the bundled example plugins and
/// an older DB picks up any newly shipped bundled plugins. The result is saved so
/// the install records stay durable.
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
        Some((sub, rest)) if sub == "runtime" => run_plugin_runtime(rest),
        _ => Err(KernelError::PluginInstall(
            "usage: relux-kernel plugin <install-dir|install-zip|install-github|remove|runtime> <arg>"
                .to_string(),
        )),
    }
}

/// Dispatch `relux-kernel plugin runtime <show|set|disable> ...`.
///
/// The HTTP loopback ToolSet runtime (`docs/RELUX_MASTER_PLAN.md` section 8.2,
/// section 18). Relux never auto-runs downloaded plugin code; an operator opts a
/// plugin into execution by pointing it at a loopback server they run themselves.
fn run_plugin_runtime(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        // `runtime set <id> <url> [--timeout-ms N]`
        Some((sub, rest)) if sub == "set" => {
            let (id, tail) = rest
                .split_first()
                .ok_or_else(|| usage_err("plugin runtime set <plugin-id> <base-url> [--timeout-ms N]"))?;
            let (url, flags) = tail
                .split_first()
                .ok_or_else(|| usage_err("plugin runtime set <plugin-id> <base-url> [--timeout-ms N]"))?;
            let timeout_ms = parse_timeout_flag(flags)?;
            run_plugin_runtime_set(id, url, timeout_ms)
        }
        // `runtime disable <id>`
        Some((sub, rest)) if sub == "disable" => {
            let id = first_arg(rest, "plugin runtime disable <plugin-id>")?;
            run_plugin_runtime_disable(&id)
        }
        // `runtime <id>` (bare) shows the config/status.
        Some((id, _)) if !id.trim().is_empty() => run_plugin_runtime_show(id),
        _ => Err(usage_err(
            "plugin runtime <plugin-id> | plugin runtime set <plugin-id> <base-url> [--timeout-ms N] | plugin runtime disable <plugin-id>",
        )),
    }
}

fn usage_err(usage: &str) -> KernelError {
    KernelError::PluginInstall(format!("usage: relux-kernel {usage}"))
}

/// Parse an optional trailing `--timeout-ms N` flag.
fn parse_timeout_flag(flags: &[String]) -> Result<Option<u64>, KernelError> {
    match flags.split_first() {
        None => Ok(None),
        Some((flag, rest)) if flag == "--timeout-ms" => {
            let val = rest
                .first()
                .ok_or_else(|| KernelError::PluginInstall("missing value for --timeout-ms".to_string()))?;
            let parsed = val
                .parse::<u64>()
                .map_err(|_| KernelError::PluginInstall(format!("invalid --timeout-ms: {val}")))?;
            Ok(Some(parsed))
        }
        Some((other, _)) => Err(KernelError::PluginInstall(format!("unknown flag: {other}"))),
    }
}

fn run_plugin_runtime_show(plugin_id: &str) -> Result<(), KernelError> {
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    store.save(&kernel)?;

    let id = PluginId::new(plugin_id);
    if kernel.installed_plugin(&id).is_none() {
        return Err(KernelError::PluginNotInstalled(plugin_id.to_string()));
    }
    println!("== Tool runtime: {plugin_id} ==");
    match kernel.tool_runtime_config(&id) {
        Some(cfg) => {
            println!("   kind:      {}", cfg.kind.as_str());
            println!("   base_url:  {}", cfg.base_url);
            println!("   enabled:   {}", cfg.enabled);
            println!("   timeout:   {} ms", cfg.timeout_ms);
        }
        None => {
            println!("   (no runtime configured)");
            println!("   Configure one with: relux-kernel plugin runtime set {plugin_id} http://127.0.0.1:<port>");
        }
    }
    Ok(())
}

fn run_plugin_runtime_set(plugin_id: &str, base_url: &str, timeout_ms: Option<u64>) -> Result<(), KernelError> {
    let id = PluginId::new(plugin_id);
    with_persistent_kernel(|kernel| {
        let cfg = kernel.configure_tool_runtime(&id, base_url, true, timeout_ms)?;
        Ok(format!(
            "configured {} runtime for {} -> {} (enabled, timeout {} ms)",
            cfg.kind.as_str(),
            cfg.plugin_id,
            cfg.base_url,
            cfg.timeout_ms
        ))
    })
}

fn run_plugin_runtime_disable(plugin_id: &str) -> Result<(), KernelError> {
    let id = PluginId::new(plugin_id);
    with_persistent_kernel(|kernel| {
        kernel.disable_tool_runtime(&id)?;
        Ok(format!("disabled runtime for {plugin_id}"))
    })
}

// --- Adapter runtime CLI (local coding-agent CLIs) -------------------------

/// List every installed Adapter plugin with its honest runtime status: whether
/// it is the local deterministic adapter, configured/enabled, and whether its
/// binary is on PATH (`docs/RELUX_MASTER_PLAN.md` section 8.1, Adapter Runtime v1).
fn run_adapters_list() -> Result<(), KernelError> {
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    store.save(&kernel)?;

    let adapters = kernel.adapter_runtime_status();
    println!("== Relux adapters ({}) ==", adapters.len());
    if adapters.is_empty() {
        println!("   (none)");
    }
    for a in &adapters {
        let onpath = if a.available_on_path { "on-path" } else { "no-binary" };
        println!(
            "   {:<28} {:<14} enabled={:<5} {:<10} {}",
            a.plugin_id,
            a.state.as_str(),
            a.enabled,
            onpath,
            a.command.clone().unwrap_or_else(|| "-".to_string()),
        );
    }
    println!();
    println!("CLI adapters are DISABLED by default. Enable one with:");
    println!("   relux-kernel adapter runtime enable <adapter-id>");
    println!("Relux runs the local CLI in a non-interactive, non-bypass mode and never");
    println!("passes --dangerously-skip-permissions.");
    Ok(())
}

/// Dispatch `relux-kernel adapter runtime <show|enable|disable> ...`.
fn run_adapter_subcommand(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        Some((sub, rest)) if sub == "runtime" => run_adapter_runtime(rest),
        _ => Err(usage_err(
            "adapter runtime <adapter-id> | adapter runtime enable <adapter-id> [flags] | adapter runtime disable <adapter-id>",
        )),
    }
}

fn run_adapter_runtime(args: &[String]) -> Result<(), KernelError> {
    match args.split_first() {
        Some((sub, rest)) if sub == "enable" => {
            let (id, flags) = rest.split_first().ok_or_else(|| {
                usage_err("adapter runtime enable <adapter-id> [--timeout-seconds N] [--max-output-bytes N] [--command C] [--working-dir D]")
            })?;
            let opts = parse_adapter_flags(flags)?;
            run_adapter_runtime_enable(id, opts)
        }
        Some((sub, rest)) if sub == "disable" => {
            let id = first_arg(rest, "adapter runtime disable <adapter-id>")?;
            run_adapter_runtime_disable(&id)
        }
        Some((id, _)) if !id.trim().is_empty() => run_adapter_runtime_show(id),
        _ => Err(usage_err(
            "adapter runtime <adapter-id> | adapter runtime enable <adapter-id> [flags] | adapter runtime disable <adapter-id>",
        )),
    }
}

/// Flags accepted by `adapter runtime enable`.
#[derive(Default)]
struct AdapterFlags {
    timeout_seconds: Option<u64>,
    max_output_bytes: Option<u64>,
    command: Option<String>,
    working_dir: Option<String>,
}

/// Parse `--timeout-seconds N`, `--max-output-bytes N`, `--command C`,
/// `--working-dir D` in any order.
fn parse_adapter_flags(flags: &[String]) -> Result<AdapterFlags, KernelError> {
    let mut out = AdapterFlags::default();
    let mut i = 0;
    while i < flags.len() {
        let flag = &flags[i];
        let value = flags.get(i + 1).cloned().ok_or_else(|| {
            KernelError::PluginInstall(format!("missing value for {flag}"))
        })?;
        match flag.as_str() {
            "--timeout-seconds" => {
                out.timeout_seconds = Some(value.parse::<u64>().map_err(|_| {
                    KernelError::PluginInstall(format!("invalid --timeout-seconds: {value}"))
                })?);
            }
            "--max-output-bytes" => {
                out.max_output_bytes = Some(value.parse::<u64>().map_err(|_| {
                    KernelError::PluginInstall(format!("invalid --max-output-bytes: {value}"))
                })?);
            }
            "--command" => out.command = Some(value),
            "--working-dir" => out.working_dir = Some(value),
            other => {
                return Err(KernelError::PluginInstall(format!("unknown flag: {other}")))
            }
        }
        i += 2;
    }
    Ok(out)
}

fn run_adapter_runtime_show(plugin_id: &str) -> Result<(), KernelError> {
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    store.save(&kernel)?;

    let id = PluginId::new(plugin_id);
    let status = kernel
        .adapter_runtime_status()
        .into_iter()
        .find(|a| a.plugin_id == plugin_id)
        .ok_or_else(|| KernelError::NotAnAdapter {
            plugin: plugin_id.to_string(),
        })?;
    println!("== Adapter runtime: {plugin_id} ==");
    println!("   state:        {}", status.state.as_str());
    println!("   kind:         {}", status.kind.clone().unwrap_or_else(|| "-".to_string()));
    println!("   configured:   {}", status.configured);
    println!("   enabled:      {}", status.enabled);
    println!("   command:      {}", status.command.clone().unwrap_or_else(|| "-".to_string()));
    println!("   on PATH:      {}", status.available_on_path);
    if let Some(p) = &status.resolved_path {
        println!("   resolved:     {p}");
    }
    if let Some(t) = status.timeout_seconds {
        println!("   timeout:      {t} s");
    }
    if let Some(m) = status.max_output_bytes {
        println!("   max output:   {m} bytes");
    }
    if let Some(w) = &status.working_dir {
        println!("   working dir:  {w}");
    }
    println!("   {}", status.detail);
    let _ = id;
    Ok(())
}

fn run_adapter_runtime_enable(plugin_id: &str, opts: AdapterFlags) -> Result<(), KernelError> {
    let id = PluginId::new(plugin_id);
    with_persistent_kernel(|kernel| {
        let cfg = kernel.configure_adapter_runtime(
            &id,
            Some(true),
            opts.command,
            opts.timeout_seconds,
            opts.max_output_bytes,
            opts.working_dir,
        )?;
        let binary = cfg.resolved_command().unwrap_or_default();
        let on_path = relux_kernel::find_on_path(&binary).is_some();
        let note = if on_path {
            format!("'{binary}' is on PATH")
        } else {
            format!("WARNING: '{binary}' was NOT found on PATH; install it before running tasks")
        };
        Ok(format!(
            "enabled {} adapter runtime for {} (timeout {}s, max output {} bytes). {}",
            cfg.kind.as_str(),
            cfg.plugin_id,
            cfg.timeout_seconds,
            cfg.max_output_bytes,
            note,
        ))
    })
}

fn run_adapter_runtime_disable(plugin_id: &str) -> Result<(), KernelError> {
    let id = PluginId::new(plugin_id);
    with_persistent_kernel(|kernel| {
        kernel.disable_adapter_runtime(&id)?;
        Ok(format!("disabled adapter runtime for {plugin_id}"))
    })
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

/// Render a tool's executable status as a short, honest label for the CLI.
fn executability_label(e: &ToolExecutability) -> &'static str {
    match e {
        ToolExecutability::Ready => "ready",
        ToolExecutability::RuntimeNotConfigured => "runtime_not_configured",
        ToolExecutability::RuntimeDisabled => "runtime_disabled",
        ToolExecutability::NotImplemented => "not_implemented",
        ToolExecutability::MissingPermission => "missing_permission",
    }
}

/// List installed plugin tools and whether the kernel can actually run each one.
///
/// Bootstraps first (so a fresh DB shows the bundled tools), then prints each
/// tool with its plugin, permission, risk, and honest executable status. Tools
/// whose runtime is not implemented are listed - never hidden, never faked.
fn run_tools_list() -> Result<(), KernelError> {
    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;
    store.save(&kernel)?;

    let tools = kernel.discover_tools(None);
    println!("== Relux installed tools ({}) ==", tools.len());
    if tools.is_empty() {
        println!("   (none)");
    }
    for t in &tools {
        println!(
            "   {:<24} {:<16} {:<8} {:<16} {}",
            t.plugin_id,
            t.tool_name,
            format!("{:?}", t.risk).to_lowercase(),
            executability_label(&t.executable),
            t.permission,
        );
    }
    println!();
    println!("Built-in deterministic tools (echo/status) are always executable. A");
    println!("'runtime_not_configured' tool becomes 'ready' once you point it at an");
    println!("operator-run loopback server: relux-kernel plugin runtime set <id>");
    println!("http://127.0.0.1:<port>. Relux never auto-runs downloaded plugin code.");
    Ok(())
}

/// Dispatch `relux-kernel tool <subcommand> ...`.
fn run_tool_subcommand(args: &[String]) -> Result<(), KernelError> {
    let usage = "usage: relux-kernel tool invoke <plugin-id> <tool-name> [json-input]";
    match args.split_first() {
        Some((sub, [plugin_id, tool_name, tail @ ..])) if sub == "invoke" => {
            let input = tail.first().map(String::as_str);
            run_tool_invoke(plugin_id, tool_name, input)
        }
        _ => Err(KernelError::Storage(usage.to_string())),
    }
}

/// Invoke a built-in tool through the kernel as the Prime agent, then pretty-print
/// the structured result. Routes through the same permission/audit path as the
/// API; an unsupported tool returns a clear `ToolRuntimeUnavailable` error.
fn run_tool_invoke(plugin_id: &str, tool_name: &str, input: Option<&str>) -> Result<(), KernelError> {
    let input_value: serde_json::Value = match input {
        Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw)
            .map_err(|e| KernelError::Storage(format!("invalid JSON input: {e}")))?,
        _ => serde_json::json!({}),
    };

    let path = db_path();
    let mut store = SqliteStore::open(&path)?;
    let mut kernel = store.load()?;
    ensure_bootstrapped(&mut kernel)?;

    let prime = kernel
        .prime_agent_id()
        .ok_or_else(|| KernelError::UnknownAgent("prime".to_string()))?;
    let result = kernel.invoke_tool(
        &prime,
        &PluginId::new(plugin_id),
        tool_name,
        input_value,
    )?;
    store.save(&kernel)?;

    println!("invoked {}/{} as {}", result.plugin_id, result.tool_name, result.agent_id);
    println!("permission: {}", result.permission);
    println!("output:");
    let pretty = serde_json::to_string_pretty(&result.output)
        .unwrap_or_else(|_| result.output.to_string());
    for line in pretty.lines() {
        println!("   {line}");
    }
    Ok(())
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
        let install_dir = dir.join(manifest.id.as_str()).display().to_string();
        kernel.install_plugin(
            manifest,
            PluginSourceKind::Bundled,
            "bundled example".to_string(),
            install_dir,
            true,
        );
    }
    println!();


    let prime_adapter = PluginId::new("relux-adapter-local-prime");
    let echo_permission = Permission::new("tool:relux-tools-echo:say")
        .expect("static echo permission is well-formed");
    let status_permission = Permission::new("tool:relux-tools-status:summary")
        .expect("static status permission is well-formed");

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
        vec![echo_permission.clone(), status_permission],
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
        "what tools can you use?",
        "what is going on?",
        "echo hello",
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
        print_tool_result(&turn);
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
