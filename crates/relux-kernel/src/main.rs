//! `relux-kernel` demo binary - drives the first local control-plane loop.
//!
//! Running `cargo run -p relux-kernel` walks the MVP loop from
//! `docs/RELUX_MASTER_PLAN.md` section 14 / section 16 end to end against an in-memory
//! [`KernelState`], using only the two static example plugin manifests under
//! `examples/relux-plugins/`. It is fully deterministic: no network, no wall
//! clock, no real API calls, so the printed output is identical every run.

use std::path::PathBuf;
use std::process::ExitCode;

use relux_core::namespace::NamespaceKind;
use relux_core::{Permission, PluginId, PrimeContext};
use relux_kernel::{load_plugin_manifests, KernelError, KernelState};

fn main() -> ExitCode {
    // CLI: `relux-kernel prime <message...>` runs exactly one grounded Prime
    // turn on the message the user actually typed (the whole tail is one
    // message). With no arguments, `relux-kernel` walks the full deterministic
    // demo loop. The `prime` path must never fall through to the canned demo
    // script - it answers the one message and stops.
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
        _ => run_demo(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("relux-kernel demo failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Bootstrap a minimal control plane (plugins + workspace + Prime agent) and run
/// exactly one Prime turn on `message`, printing only that turn.
///
/// This is the CLI seam for `relux-kernel prime <message>`: it honors the
/// message the user provided instead of replaying a fixed script, so a greeting
/// stays a greeting and "create a task to X" creates exactly that task
/// (`docs/RELUX_MASTER_PLAN.md` section 10, section 16).
fn run_prime_message(message: &str) -> Result<(), KernelError> {
    let mut kernel = KernelState::new();

    let dir = examples_dir();
    for manifest in load_plugin_manifests(&dir)? {
        kernel.register_plugin(manifest);
    }

    let prime_adapter = PluginId::new("relux-adapter-local-prime");
    let echo_permission = Permission::new("tool:relux-tools-echo:say")
        .expect("static echo permission is well-formed");

    let workspace = kernel.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
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
        vec![echo_permission],
    )?;

    let prime_ctx = PrimeContext {
        namespace: workspace,
        agent: prime,
        actor: "founder".to_string(),
    };

    let turn = kernel.prime_turn(&prime_ctx, message)?;
    println!("   you   > {message}");
    println!(
        "   prime [{:?}/{:?}] {}",
        turn.intent, turn.disposition, turn.reply
    );

    Ok(())
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
