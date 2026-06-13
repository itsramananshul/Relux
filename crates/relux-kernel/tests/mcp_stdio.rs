//! Integration tests for the **managed-stdio MCP client** against a REAL subprocess
//! (`src/bin/relux_mcp_test_server.rs`, a tiny pure-Rust MCP stdio server) — not the
//! kernel's built-in echo. They prove the real lifecycle end to end: spawn →
//! `initialize` → `tools/list` / `tools/call` → reap, plus honest failures and the
//! kernel-level dispatch through `KernelState::discover_mcp_tools`.

use relux_kernel::mcp::McpClientError;
use relux_kernel::mcp_stdio;
use relux_kernel::state::KernelState;

/// Path to the test-fixture MCP stdio server binary (built by Cargo for this test).
fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_relux_mcp_test_server")
}

/// The disabled sampling context (capability never advertised, every server-initiated
/// sampling request cleanly refused) — the default for a session that does not exercise
/// gated sampling.
fn no_sampling() -> relux_kernel::mcp_sampling::SamplingContext {
    relux_kernel::mcp_sampling::SamplingContext::disabled()
}

#[test]
fn discovers_tools_from_a_real_stdio_server() {
    let tools = mcp_stdio::discover_tools(fixture(), &[], &[], None, 5_000).expect("discovery ok");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"status.summary"), "tools: {names:?}");
    assert!(names.contains(&"boom"), "tools: {names:?}");
    assert!(names.contains(&"noisy"), "tools: {names:?}");
    // The description is carried + sanitized.
    let status = tools.iter().find(|t| t.name == "status.summary").unwrap();
    assert!(status.description.contains("status summary"), "desc: {}", status.description);
}

#[test]
fn calls_a_tool_and_gets_a_shaped_result() {
    let out = mcp_stdio::call_tool(
        fixture(),
        &[],
        &[],
        None,
        "status.summary",
        &serde_json::json!({ "q": "hi" }),
        5_000,
    )
    .expect("call ok");
    // Shaped result — NEVER the raw { jsonrpc, id, result } envelope.
    assert_eq!(out["result"], "status ok; q=hi");
    assert_eq!(out["structuredContent"], serde_json::json!({ "ok": true }));
    assert!(out.get("jsonrpc").is_none(), "raw envelope leaked: {out}");
}

#[test]
fn a_tool_iserror_is_an_honest_failure_not_a_fake_success() {
    let err = mcp_stdio::call_tool(fixture(), &[], &[], None, "boom", &serde_json::json!({}), 5_000)
        .unwrap_err();
    assert!(
        matches!(err, McpClientError::ToolCallError(ref m) if m.contains("intentional failure")),
        "got {err:?}"
    );
}

#[test]
fn a_noisy_tool_still_returns_ok() {
    // The server writes to stderr then returns ok; the client drains stderr (bounded)
    // and still surfaces the shaped ok result.
    let out = mcp_stdio::call_tool(fixture(), &[], &[], None, "noisy", &serde_json::json!({}), 5_000)
        .expect("call ok");
    assert_eq!(out["result"], "noisy ok");
}

#[test]
fn an_unknown_tool_fails_cleanly() {
    let err = mcp_stdio::call_tool(fixture(), &[], &[], None, "does.not.exist", &serde_json::json!({}), 5_000)
        .unwrap_err();
    // A JSON-RPC error from the server, surfaced honestly (never a fabricated result).
    assert!(matches!(err, McpClientError::ServerError { .. }), "got {err:?}");
}

#[test]
fn spawn_failure_is_honest() {
    let err = mcp_stdio::discover_tools("relux-mcp-no-such-binary-xyzzy", &[], &[], None, 1_000)
        .unwrap_err();
    assert!(matches!(err, McpClientError::Spawn(_)), "got {err:?}");
}

#[test]
fn kernel_discovers_a_managed_stdio_server_through_the_registry() {
    let mut k = KernelState::new();
    // Registration is explicit + validated; it does NOT spawn the command.
    k.register_mcp_stdio_server(
        "local-fs",
        fixture(),
        &[],
        Default::default(),
        None,
        "test stdio server",
        true,
        Some(5_000),
    )
    .expect("register ok");

    // Discover runs the live tools/list by SPAWNING the command (operator-controlled).
    let tools = k.discover_mcp_tools("local-fs").expect("discovery ok");
    let status = tools
        .iter()
        .find(|t| t.tool_name == "status.summary")
        .expect("status.summary discovered");
    // Mapped into the standard ToolDescriptor shape, under the mcp:<server> namespace.
    assert_eq!(status.plugin_id, "mcp:local-fs");
    assert_eq!(status.permission, "tool:mcp-local-fs:summary");
    assert_eq!(status.source_kind, "Mcp");
    // Unclassified ⇒ gated (needs approval), never auto-runnable.
    assert_eq!(status.executable, relux_core::ToolExecutability::NeedsApproval);
}

#[test]
fn off_lock_discover_and_classify_probes_a_real_stdio_server() {
    // The post-activation discovery primitive: probe + classify straight from a config,
    // off the kernel lock (this is what Prime's configure-candidate route runs after it
    // registers a candidate). It spawns the command, lists tools, and classifies each —
    // identical to `discover_mcp_tools`, but with no `&KernelState`.
    let cfg = relux_core::McpServerConfig {
        id: "fixture".to_string(),
        transport: relux_core::McpTransport::ManagedStdio,
        endpoint: String::new(),
        command: Some(fixture().to_string()),
        args: Vec::new(),
        env: Default::default(),
        cwd: None,
        description: "post-activation probe".to_string(),
        enabled: true,
        timeout_ms: 5_000,
        tool_overrides: Default::default(),
        sampling: Default::default(),
    };
    let tools = relux_kernel::discover_and_classify_mcp_tools(&cfg).expect("probe ok");
    assert!(!tools.is_empty(), "the fixture advertises tools");
    let status = tools
        .iter()
        .find(|t| t.tool_name == "status.summary")
        .expect("status.summary discovered");
    assert_eq!(status.plugin_id, "mcp:fixture");
    assert_eq!(status.source_kind, "Mcp");
    // Every freshly-discovered, unclassified tool is gated — discovery NEVER silently
    // marks a tool low-risk / directly runnable.
    assert!(
        tools.iter().all(|t| t.executable == relux_core::ToolExecutability::NeedsApproval),
        "all discovered tools stay gated until classified"
    );
}

#[test]
fn off_lock_discover_and_classify_fails_cleanly_on_a_missing_secret() {
    // A managed-stdio server whose env references an absent secret cannot be probed —
    // the spawn-per-op resolver fails BEFORE spawning, naming the secret KEY + env var
    // (never a value). The configure-candidate route turns this into "map its secrets,
    // then Discover" guidance.
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "NEEDS_TOKEN".to_string(),
        relux_core::McpEnvRef {
            secret: "definitely_absent_secret_probe_xyz".to_string(),
        },
    );
    let cfg = relux_core::McpServerConfig {
        id: "needs-secret".to_string(),
        transport: relux_core::McpTransport::ManagedStdio,
        endpoint: String::new(),
        command: Some(fixture().to_string()),
        args: Vec::new(),
        env,
        cwd: None,
        description: "needs a secret".to_string(),
        enabled: true,
        timeout_ms: 5_000,
        tool_overrides: Default::default(),
        sampling: Default::default(),
    };
    let err = relux_kernel::discover_and_classify_mcp_tools(&cfg).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("definitely_absent_secret_probe_xyz"), "names the secret: {msg}");
    assert!(msg.contains("NEEDS_TOKEN"), "names the env var: {msg}");
}

#[test]
fn kernel_disabled_stdio_server_refuses_discovery() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "off",
        fixture(),
        &[],
        Default::default(),
        None,
        "disabled",
        false,
        Some(5_000),
    )
    .expect("register ok");
    assert!(k.discover_mcp_tools("off").is_err(), "a disabled server must refuse discovery");
}

// --- Managed-stdio process pool lifecycle (start/stop/restart/reuse/crash) ---

use relux_core::ManagedStdioState;
use relux_kernel::mcp_stdio::pool;

/// Parse the per-process `calls` counter the fixture's `whoami` tool returns in its
/// structuredContent, proving a SINGLE process is reused across calls.
fn calls_from_whoami(out: &serde_json::Value) -> u64 {
    out["structuredContent"]["calls"].as_u64().expect("calls counter")
}
fn pid_from_whoami(out: &serde_json::Value) -> u64 {
    out["structuredContent"]["pid"].as_u64().expect("pid")
}

#[test]
fn pool_start_status_list_call_stop_lifecycle() {
    let id = "pool-lifecycle";
    let status = pool().start(id, fixture(), &[], &[], None, 5_000, no_sampling());
    assert_eq!(status.state, ManagedStdioState::Running, "start → running: {status:?}");
    assert!(status.pid.is_some(), "a running process has a pid");
    assert!(status.started_at_ms.is_some(), "a running process has a start time");

    // tools/list reuses the running process.
    let tools = pool().list_tools(id, 5_000).expect("list ok");
    assert!(tools.iter().any(|t| t.name == "whoami"), "tools: {tools:?}");
    // tools_count is now recorded on the status.
    assert_eq!(pool().status(id).tools_count, Some(tools.len()));

    // tools/call reuses the same process and returns a shaped result.
    let out = pool()
        .call_tool(id, "status.summary", &serde_json::json!({ "q": "hi" }), 5_000)
        .expect("call ok");
    assert_eq!(out["result"], "status ok; q=hi");
    assert!(out.get("jsonrpc").is_none(), "raw envelope leaked: {out}");

    let stopped = pool().stop(id);
    assert_eq!(stopped.state, ManagedStdioState::Stopped);
    assert!(stopped.pid.is_none(), "a stopped process has no pid");
    assert!(!pool().is_running(id), "stopped server is not running");
}

#[test]
fn pool_reuses_one_process_across_calls() {
    let id = "pool-reuse";
    let start = pool().start(id, fixture(), &[], &[], None, 5_000, no_sampling());
    let started_pid = start.pid.expect("running pid");

    let first = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    let second = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    // Same OS process (same pid as the status) and a monotonically increasing
    // per-process counter prove ONE long-lived process served both calls.
    assert_eq!(pid_from_whoami(&first), started_pid as u64);
    assert_eq!(pid_from_whoami(&second), started_pid as u64);
    assert_eq!(calls_from_whoami(&first), 1, "first call on a fresh process");
    assert_eq!(calls_from_whoami(&second), 2, "second call on the SAME process");

    pool().stop(id);
}

#[test]
fn pool_reuse_requires_an_explicit_start() {
    let id = "pool-not-started";
    // No start → no running process → reuse fails cleanly (the caller falls back to
    // spawn-per-operation). Nothing is auto-started.
    assert!(!pool().is_running(id));
    let err = pool().call_tool(id, "whoami", &serde_json::json!({}), 2_000).unwrap_err();
    assert!(matches!(err, McpClientError::ProcessExited), "got {err:?}");
}

#[test]
fn pool_restart_replaces_the_process() {
    let id = "pool-restart";
    let first = pool().start(id, fixture(), &[], &[], None, 5_000, no_sampling());
    let pid1 = first.pid.expect("pid1");
    // whoami once on the first process.
    let a = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    assert_eq!(calls_from_whoami(&a), 1);

    let second = pool().restart(id, fixture(), &[], &[], None, 5_000, no_sampling());
    let pid2 = second.pid.expect("pid2");
    assert_ne!(pid1, pid2, "restart spawns a NEW process (different pid)");
    // The fresh process's counter starts over at 1 (it is genuinely a new process).
    let b = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    assert_eq!(calls_from_whoami(&b), 1, "the restarted process is fresh");

    pool().stop(id);
}

#[test]
fn pool_process_crash_marks_failed_and_records_error() {
    let id = "pool-crash";
    let start = pool().start(id, fixture(), &[], &[], None, 5_000, no_sampling());
    assert_eq!(start.state, ManagedStdioState::Running);

    // The `crash` tool exits the process without responding; the call sees EOF and
    // fails honestly (never a fabricated success).
    let err = pool().call_tool(id, "crash", &serde_json::json!({}), 5_000).unwrap_err();
    assert!(
        matches!(err, McpClientError::ProcessExited | McpClientError::Stdio(_)),
        "got {err:?}"
    );
    // The status now reports the process as failed, with an honest reason and no pid.
    let status = pool().status(id);
    assert_eq!(status.state, ManagedStdioState::Failed, "crash → failed: {status:?}");
    assert!(status.pid.is_none(), "a dead process has no pid");
    assert!(status.last_error.is_some(), "a crash records an honest reason");
    assert!(!pool().is_running(id));

    pool().stop(id);
}

#[test]
fn pool_start_failure_is_an_honest_failed_status() {
    let id = "pool-bad-binary";
    // Nothing by this name is on PATH; the start fails → a `failed` status with a
    // redacted reason (never a fabricated `running`).
    let status = pool().start(id, "relux-mcp-no-such-binary-xyzzy", &[], &[], None, 1_000, no_sampling());
    assert_eq!(status.state, ManagedStdioState::Failed, "bad binary → failed: {status:?}");
    assert!(status.last_error.is_some(), "a failed start records why");
    assert!(status.pid.is_none());
}

#[test]
fn kernel_managed_stdio_lifecycle_through_the_registry() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "life-fs",
        fixture(),
        &[],
        Default::default(),
        None,
        "test stdio server",
        true,
        Some(5_000),
    )
    .expect("register ok");

    // Start through the kernel (audited); the process comes up.
    let status = k.start_mcp_stdio_server("life-fs", None).expect("start ok");
    assert_eq!(status.state, ManagedStdioState::Running);

    // Discovery now REUSES the running process (still mapped into ToolDescriptors).
    let tools = k.discover_mcp_tools("life-fs").expect("discover ok");
    assert!(tools.iter().any(|t| t.tool_name == "whoami"), "tools: {tools:?}");

    // The status surface lists it as running.
    let listed = k.mcp_stdio_statuses();
    assert!(listed.iter().any(|s| s.id == "life-fs" && s.state == ManagedStdioState::Running));

    // Stop through the kernel.
    let stopped = k.stop_mcp_stdio_server("life-fs").expect("stop ok");
    assert_eq!(stopped.state, ManagedStdioState::Stopped);
}

#[test]
fn kernel_lifecycle_refuses_http_and_disabled_servers() {
    let mut k = KernelState::new();
    // An HTTP-loopback server has no process lifecycle.
    k.register_mcp_server("http-one", "http://127.0.0.1:8000/mcp", "http", true, Some(5_000))
        .expect("register http ok");
    assert!(matches!(
        k.start_mcp_stdio_server("http-one", None),
        Err(relux_kernel::KernelError::NotAManagedStdioServer(_))
    ));

    // A disabled managed-stdio server refuses to start (enable it first).
    k.register_mcp_stdio_server(
        "off-stdio",
        fixture(),
        &[],
        Default::default(),
        None,
        "disabled",
        false,
        Some(5_000),
    )
    .expect("register ok");
    assert!(matches!(
        k.start_mcp_stdio_server("off-stdio", None),
        Err(relux_kernel::KernelError::McpServerDisabled(_))
    ));

    // An unknown server is a clean not-found.
    assert!(matches!(
        k.start_mcp_stdio_server("nope", None),
        Err(relux_kernel::KernelError::UnknownMcpServer(_))
    ));
}

// --- Local secrets → managed-stdio env injection (end to end) ---------------

use relux_kernel::secret_store::secret_store;

/// The same FNV-1a hash the fixture uses, so the test can attest the injected value
/// matches WITHOUT the raw value ever appearing in the test or the transport.
fn fnv1a_hex(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[test]
fn managed_stdio_child_receives_a_resolved_env_secret_end_to_end() {
    // A unique secret name so the process-global store does not collide with other
    // tests in the same process.
    let secret_name = "relux_e2e_managed_env_secret_777";
    let secret_value = "tok-abcdef-0987654321-xyz";
    secret_store().set(secret_name, secret_value).expect("set secret");

    let mut k = KernelState::new();
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "RELUX_FIXTURE_TOKEN".to_string(),
        relux_core::McpEnvRef {
            secret: secret_name.to_string(),
        },
    );
    k.register_mcp_stdio_server(
        "env-e2e",
        fixture(),
        &[],
        env,
        None,
        "env injection test",
        true,
        Some(5_000),
    )
    .expect("register ok");

    // Start RESOLVES the secret ref and injects the plaintext into the child env.
    let status = k.start_mcp_stdio_server("env-e2e", None).expect("start ok");
    assert_eq!(status.state, ManagedStdioState::Running, "start → running: {status:?}");
    // The status surface carries NO secret value (defense in depth).
    let status_json = serde_json::to_string(&status).unwrap();
    assert!(!status_json.contains(secret_value), "secret leaked into status: {status_json}");

    // The child reports the env var is PRESENT with a matching value hash — never the
    // value itself.
    let out = pool()
        .call_tool(
            "env-e2e",
            "env_probe",
            &serde_json::json!({ "var": "RELUX_FIXTURE_TOKEN" }),
            5_000,
        )
        .expect("probe ok");
    assert_eq!(out["structuredContent"]["present"], serde_json::json!(true));
    assert_eq!(
        out["structuredContent"]["fnv1a"],
        serde_json::json!(fnv1a_hex(secret_value)),
        "the child received a DIFFERENT value than the stored secret"
    );
    // The raw secret value never appears in the shaped result.
    let out_json = serde_json::to_string(&out).unwrap();
    assert!(!out_json.contains(secret_value), "secret value leaked in result: {out_json}");

    k.stop_mcp_stdio_server("env-e2e").expect("stop ok");
    secret_store().delete(secret_name);
}

#[test]
fn managed_stdio_start_fails_cleanly_when_a_referenced_secret_is_missing() {
    let mut k = KernelState::new();
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "NEEDS_TOKEN".to_string(),
        relux_core::McpEnvRef {
            secret: "definitely_absent_secret_e2e_xyz".to_string(),
        },
    );
    k.register_mcp_stdio_server(
        "env-missing",
        fixture(),
        &[],
        env,
        None,
        "missing secret",
        true,
        Some(5_000),
    )
    .expect("register ok");

    // Start does NOT spawn — it fails cleanly, naming the missing secret KEY (never a
    // value), as a `failed` status rather than a fabricated `running`.
    let status = k.start_mcp_stdio_server("env-missing", None).expect("start returns a status");
    assert_eq!(status.state, ManagedStdioState::Failed, "missing secret → failed: {status:?}");
    let reason = status.last_error.clone().unwrap_or_default();
    assert!(reason.contains("definitely_absent_secret_e2e_xyz"), "names the secret: {reason}");
    assert!(reason.contains("NEEDS_TOKEN"), "names the env var: {reason}");
    assert!(status.pid.is_none(), "a failed start has no pid");

    pool().stop("env-missing");
}

// --- Managed-stdio MCP resources (resources/list + resources/read) ----------

#[test]
fn lists_resources_from_a_real_stdio_server() {
    // Spawn-per-operation listing (no managed process running).
    let resources =
        mcp_stdio::list_resources(fixture(), &[], &[], None, 5_000).expect("list ok");
    let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();
    assert!(uris.contains(&"mem://notes"), "uris: {uris:?}");
    assert!(uris.contains(&"mem://image"), "uris: {uris:?}");
    let notes = resources.iter().find(|r| r.uri == "mem://notes").unwrap();
    assert_eq!(notes.mime_type.as_deref(), Some("text/plain"));
    assert!(notes.description.contains("notes"), "desc: {}", notes.description);
}

#[test]
fn reads_a_text_resource_and_redacts_secrets() {
    let content = mcp_stdio::read_resource(fixture(), &[], &[], None, "mem://notes", 5_000)
        .expect("read ok");
    assert_eq!(content.uri, "mem://notes");
    assert_eq!(content.mime_type.as_deref(), Some("text/plain"));
    assert!(!content.binary, "a text resource is not binary");
    // The legit prose survives; the embedded secret is redacted (never verbatim).
    assert!(content.text.contains("notes line one"), "text: {}", content.text);
    assert!(
        !content.text.contains("sk-fixturesupersecret1234567890"),
        "secret must be redacted: {}",
        content.text
    );
}

#[test]
fn reads_a_binary_resource_without_surfacing_bytes() {
    let content = mcp_stdio::read_resource(fixture(), &[], &[], None, "mem://image", 5_000)
        .expect("read ok");
    assert!(content.binary, "a blob resource sets the binary flag");
    // The raw base64 bytes are NEVER surfaced; an honest marker is.
    assert!(content.text.contains("[binary content omitted"), "text: {}", content.text);
    assert!(!content.text.contains("aGVsbG8td29ybGQ="), "raw bytes leaked: {}", content.text);
}

#[test]
fn an_unknown_resource_uri_fails_cleanly() {
    let err = mcp_stdio::read_resource(fixture(), &[], &[], None, "mem://nope", 5_000)
        .unwrap_err();
    // A JSON-RPC error from the server, surfaced honestly (never a fabricated body).
    assert!(matches!(err, McpClientError::ServerError { .. }), "got {err:?}");
}

#[test]
fn pool_lists_and_reads_resources_reusing_one_process() {
    let id = "pool-resources";
    let start = pool().start(id, fixture(), &[], &[], None, 5_000, no_sampling());
    assert_eq!(start.state, ManagedStdioState::Running, "start → running: {start:?}");

    // resources/list reuses the running process.
    let resources = pool().list_resources(id, 5_000).expect("list ok");
    assert!(resources.iter().any(|r| r.uri == "mem://notes"), "resources: {resources:?}");

    // resources/read reuses the SAME process and returns shaped, redacted content.
    let content = pool().read_resource(id, "mem://notes", 5_000).expect("read ok");
    assert!(content.text.contains("notes line one"), "text: {}", content.text);
    assert!(
        !content.text.contains("sk-fixturesupersecret1234567890"),
        "secret must be redacted: {}",
        content.text
    );

    pool().stop(id);
}

#[test]
fn pool_resource_reuse_requires_an_explicit_start() {
    let id = "pool-resources-not-started";
    // No start → no running process → reuse fails cleanly (the caller then falls back
    // to spawn-per-operation). Nothing is auto-started.
    assert!(!pool().is_running(id));
    let err = pool().list_resources(id, 2_000).unwrap_err();
    assert!(matches!(err, McpClientError::ProcessExited), "got {err:?}");
}

#[test]
fn kernel_lists_and_reads_resources_over_managed_stdio() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "res-fs",
        fixture(),
        &[],
        Default::default(),
        None,
        "resources stdio server",
        true,
        Some(5_000),
    )
    .expect("register ok");

    // The kernel surface lists resources over the stdio transport (spawn-per-op here).
    let resources = k.list_mcp_resources("res-fs").expect("list ok");
    assert!(resources.iter().any(|r| r.uri == "mem://notes"), "resources: {resources:?}");

    // And reads ONE, with the URI validated and the body shaped + redacted.
    let content = k.read_mcp_resource("res-fs", "mem://notes").expect("read ok");
    assert!(content.text.contains("notes line one"), "text: {}", content.text);
    assert!(
        !content.text.contains("sk-fixturesupersecret1234567890"),
        "secret leaked: {}",
        content.text
    );

    // An invalid URI is fail-closed (never dialed), even over stdio.
    assert!(matches!(
        k.read_mcp_resource("res-fs", "bad\r\nuri"),
        Err(relux_kernel::KernelError::InvalidMcpResourceUri { .. })
    ));
}

#[test]
fn kernel_disabled_stdio_server_refuses_resource_list() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "res-off",
        fixture(),
        &[],
        Default::default(),
        None,
        "disabled",
        false,
        Some(5_000),
    )
    .expect("register ok");
    assert!(matches!(
        k.list_mcp_resources("res-off"),
        Err(relux_kernel::KernelError::McpServerDisabled(_))
    ));
}

#[test]
fn kernel_remove_stops_the_managed_process() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "rm-fs",
        fixture(),
        &[],
        Default::default(),
        None,
        "to remove",
        true,
        Some(5_000),
    )
    .expect("register ok");
    let status = k.start_mcp_stdio_server("rm-fs", None).expect("start ok");
    assert_eq!(status.state, ManagedStdioState::Running);
    // Removing the registration stops + reaps the process so no daemon lingers.
    k.remove_mcp_server("rm-fs").expect("remove ok");
    assert!(!pool().is_running("rm-fs"), "removed server's process is stopped");
}

// --- Managed-stdio MCP prompts (prompts/list + prompts/get) -----------------

#[test]
fn lists_prompts_from_a_real_stdio_server() {
    // Spawn-per-operation listing (no managed process running).
    let prompts = mcp_stdio::list_prompts(fixture(), &[], &[], None, 5_000).expect("list ok");
    let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"greet"), "names: {names:?}");
    assert!(names.contains(&"leaky"), "names: {names:?}");
    let greet = prompts.iter().find(|p| p.name == "greet").unwrap();
    assert_eq!(greet.arguments.len(), 1);
    assert_eq!(greet.arguments[0].name, "who");
    assert!(greet.arguments[0].required);
}

#[test]
fn gets_a_prompt_forwarding_arguments() {
    let result = mcp_stdio::get_prompt(
        fixture(),
        &[],
        &[],
        None,
        "greet",
        &serde_json::json!({ "who": "Ada" }),
        5_000,
    )
    .expect("get ok");
    assert_eq!(result.name, "greet");
    assert_eq!(result.description.as_deref(), Some("A greeting."));
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].role, "user");
    // The forwarded argument was materialized into the template message.
    assert!(result.messages[0].content.contains("Hello, Ada!"), "msg: {}", result.messages[0].content);
}

#[test]
fn gets_a_prompt_and_redacts_secrets() {
    let result = mcp_stdio::get_prompt(fixture(), &[], &[], None, "leaky", &serde_json::json!({}), 5_000)
        .expect("get ok");
    let content = &result.messages[0].content;
    assert!(content.contains("remember"), "content: {content}");
    // The embedded fake secret is redacted, never returned verbatim.
    assert!(
        !content.contains("sk-fixturepromptsecret1234567890"),
        "secret must be redacted: {content}"
    );
}

#[test]
fn an_unknown_prompt_name_fails_cleanly() {
    let err = mcp_stdio::get_prompt(fixture(), &[], &[], None, "nope", &serde_json::json!({}), 5_000)
        .unwrap_err();
    // A JSON-RPC error from the server, surfaced honestly (never a fabricated body).
    assert!(matches!(err, McpClientError::ServerError { .. }), "got {err:?}");
}

#[test]
fn pool_lists_and_gets_prompts_reusing_one_process() {
    let id = "pool-prompts";
    let start = pool().start(id, fixture(), &[], &[], None, 5_000, no_sampling());
    assert_eq!(start.state, ManagedStdioState::Running, "start → running: {start:?}");

    // prompts/list reuses the running process.
    let prompts = pool().list_prompts(id, 5_000).expect("list ok");
    assert!(prompts.iter().any(|p| p.name == "greet"), "prompts: {prompts:?}");

    // prompts/get reuses the SAME process and returns shaped, redacted content.
    let result = pool()
        .get_prompt(id, "leaky", &serde_json::json!({}), 5_000)
        .expect("get ok");
    assert!(
        !result.messages[0].content.contains("sk-fixturepromptsecret1234567890"),
        "secret must be redacted: {}",
        result.messages[0].content
    );

    pool().stop(id);
}

#[test]
fn pool_prompt_reuse_requires_an_explicit_start() {
    let id = "pool-prompts-not-started";
    assert!(!pool().is_running(id));
    let err = pool().list_prompts(id, 2_000).unwrap_err();
    assert!(matches!(err, McpClientError::ProcessExited), "got {err:?}");
}

#[test]
fn kernel_lists_and_gets_prompts_over_managed_stdio() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "prm-fs",
        fixture(),
        &[],
        Default::default(),
        None,
        "prompts stdio server",
        true,
        Some(5_000),
    )
    .expect("register ok");

    // The kernel surface lists prompts over the stdio transport (spawn-per-op here).
    let prompts = k.list_mcp_prompts("prm-fs").expect("list ok");
    assert!(prompts.iter().any(|p| p.name == "greet"), "prompts: {prompts:?}");

    // And materializes ONE, with the name validated and the body shaped + redacted.
    let result = k
        .get_mcp_prompt("prm-fs", "greet", &serde_json::json!({ "who": "Bee" }))
        .expect("get ok");
    assert!(result.messages[0].content.contains("Hello, Bee!"), "msg: {}", result.messages[0].content);

    // An invalid name is fail-closed (never dialed), even over stdio.
    assert!(matches!(
        k.get_mcp_prompt("prm-fs", "bad\r\nname", &serde_json::json!({})),
        Err(relux_kernel::KernelError::InvalidMcpPromptName { .. })
    ));
}

#[test]
fn kernel_disabled_stdio_server_refuses_prompt_list() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "prm-off",
        fixture(),
        &[],
        Default::default(),
        None,
        "disabled",
        false,
        Some(5_000),
    )
    .expect("register ok");
    assert!(matches!(
        k.list_mcp_prompts("prm-off"),
        Err(relux_kernel::KernelError::McpServerDisabled(_))
    ));
}

// --- Gated MCP sampling (server-initiated sampling/createMessage) ---------------
//
// These drive the `sample_probe` fixture tool, which (during its tools/call) sends a
// SERVER→client `sampling/createMessage` request back to the managed-stdio session and
// returns whatever the client answered. They prove the gated round trip end to end:
// default-deny, allowed-with-a-(test)-provider, missing-provider, output redaction, the
// secret-free audit tail, and that no provider key ever reaches the server.

use relux_kernel::mcp_sampling::{
    self, Sampler, SamplingCompletion, SamplingContext, SAMPLING_ERR_DENIED_POLICY,
    SAMPLING_ERR_NO_PROVIDER,
};

/// A deterministic test provider whose completion embeds an obvious fake secret AND
/// overflows the output cap — so a test can PROVE the handler redacts + clamps before the
/// text ever reaches the (possibly hostile) server.
fn leaky_test_sampler() -> Sampler {
    std::sync::Arc::new(|_req| {
        Ok(SamplingCompletion {
            text: format!(
                "ANSWER api_key=sk-providerleaksecret1234567890 {}",
                "z".repeat(relux_core::MAX_MCP_SAMPLING_OUTPUT_CHARS + 200)
            ),
            model: "test/sampling-model".to_string(),
        })
    })
}

#[test]
fn sampling_is_denied_by_default() {
    let id = "samp-denied";
    // Disabled policy (the default): capability not advertised; a non-compliant server
    // that asks anyway is cleanly REFUSED, never run, never hung.
    let ctx = SamplingContext {
        enabled: false,
        server_id: id.to_string(),
        sampler: Some(leaky_test_sampler()),
    };
    pool().start(id, fixture(), &[], &[], None, 5_000, ctx);
    let out = pool()
        .call_tool(id, "sample_probe", &serde_json::json!({ "prompt": "hi" }), 5_000)
        .expect("call ok");
    let sc = &out["structuredContent"];
    assert_eq!(sc["kind"], "error", "denied → JSON-RPC error: {out}");
    assert_eq!(sc["code"], SAMPLING_ERR_DENIED_POLICY);
    let rec = mcp_sampling::audit_tail()
        .into_iter()
        .rev()
        .find(|r| r.server_id == id)
        .expect("an audit record");
    assert_eq!(rec.decision, relux_core::SAMPLING_DECISION_DENIED_POLICY);
    assert_eq!(rec.output_chars, 0);
    pool().stop(id);
}

#[test]
fn sampling_is_allowed_with_a_provider_and_is_redacted_and_audited() {
    let id = "samp-allowed";
    let ctx = SamplingContext {
        enabled: true,
        server_id: id.to_string(),
        sampler: Some(leaky_test_sampler()),
    };
    pool().start(id, fixture(), &[], &[], None, 5_000, ctx);
    let out = pool()
        .call_tool(id, "sample_probe", &serde_json::json!({ "prompt": "summarize" }), 5_000)
        .expect("call ok");
    let sc = &out["structuredContent"];
    assert_eq!(sc["kind"], "result", "allowed → a completion: {out}");
    let text = sc["text"].as_str().expect("completion text");
    // The fake provider secret was redacted BEFORE it reached the server, and the output
    // was clamped to the cap.
    assert!(!text.contains("sk-providerleaksecret"), "secret must be redacted: {text}");
    assert!(
        text.chars().count() <= relux_core::MAX_MCP_SAMPLING_OUTPUT_CHARS,
        "output clamped"
    );
    assert_eq!(sc["model"], "test/sampling-model");

    // Audit recorded the decision + metadata, never any plaintext.
    let rec = mcp_sampling::audit_tail()
        .into_iter()
        .rev()
        .find(|r| r.server_id == id)
        .expect("an audit record");
    assert_eq!(rec.decision, relux_core::SAMPLING_DECISION_ALLOWED);
    assert!(rec.input_chars > 0 && rec.output_chars > 0);
    assert_eq!(rec.model.as_deref(), Some("test/sampling-model"));
    assert!(!rec.reason.contains("sk-"), "audit reason carries no secret");
    pool().stop(id);
}

#[test]
fn sampling_enabled_without_provider_is_a_clean_no_provider_refusal() {
    let id = "samp-noprov";
    let ctx = SamplingContext {
        enabled: true,
        server_id: id.to_string(),
        sampler: None,
    };
    pool().start(id, fixture(), &[], &[], None, 5_000, ctx);
    let out = pool()
        .call_tool(id, "sample_probe", &serde_json::json!({ "prompt": "hi" }), 5_000)
        .expect("call ok");
    let sc = &out["structuredContent"];
    assert_eq!(sc["kind"], "error");
    assert_eq!(sc["code"], SAMPLING_ERR_NO_PROVIDER);
    let rec = mcp_sampling::audit_tail()
        .into_iter()
        .rev()
        .find(|r| r.server_id == id)
        .expect("an audit record");
    assert_eq!(rec.decision, relux_core::SAMPLING_DECISION_DENIED_NO_PROVIDER);
    pool().stop(id);
}

#[test]
fn kernel_set_sampling_policy_is_rejected_on_an_http_server() {
    let mut k = KernelState::new();
    k.register_mcp_server("http-samp", "http://127.0.0.1:8765/mcp", "http", true, Some(5_000))
        .expect("register ok");
    // Sampling needs a persistent session; an HTTP-loopback server has none → fail-closed.
    assert!(matches!(
        k.set_mcp_sampling_policy("http-samp", true),
        Err(relux_kernel::KernelError::InvalidMcpConfig { .. })
    ));
}

#[test]
fn kernel_set_sampling_policy_persists_on_a_stdio_server() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server(
        "stdio-samp",
        fixture(),
        &[],
        Default::default(),
        None,
        "stdio",
        true,
        Some(5_000),
    )
    .expect("register ok");
    let updated = k.set_mcp_sampling_policy("stdio-samp", true).expect("enable ok");
    assert!(updated.sampling.enabled);
    // Re-registering preserves the operator's sampling policy (not silently reset).
    k.register_mcp_stdio_server(
        "stdio-samp",
        fixture(),
        &[],
        Default::default(),
        None,
        "stdio re-registered",
        true,
        Some(5_000),
    )
    .expect("re-register ok");
    assert!(
        k.mcp_servers()
            .into_iter()
            .find(|s| s.id == "stdio-samp")
            .expect("server present")
            .sampling
            .enabled,
        "sampling policy preserved across re-registration"
    );
}
