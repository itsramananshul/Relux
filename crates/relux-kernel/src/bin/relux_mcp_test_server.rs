//! A tiny, real **MCP stdio server** used ONLY as a deterministic test fixture for
//! the managed-stdio client (`crates/relux-kernel/src/mcp_stdio.rs` and
//! `crates/relux-kernel/tests/mcp_stdio.rs`). It is NOT a product surface.
//!
//! It speaks the same JSON-RPC-over-stdio subset Relux's managed-stdio client uses —
//! `initialize`, `notifications/initialized`, `tools/list`, `tools/call` — so the
//! integration test exercises a genuine subprocess (real spawn → handshake → list →
//! call → reap), not the kernel's built-in echo tool. Pure Rust + serde_json: no
//! node/python/network dependency, so it runs identically on every platform/CI.
//!
//! Tools it advertises:
//! - `status.summary` — returns a small computed text result.
//! - `boom` — returns a `tools/call` result flagged `isError` (an honest failure).
//! - `noisy` — writes a line to stderr (so the client's bounded stderr tail is
//!   exercised) and returns an ok result.

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        // A notification (no id) gets no response.
        let Some(id) = req.get("id").cloned() else {
            continue;
        };

        let response = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": { "name": "relux-mcp-test-server", "version": "0" }
                }
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "tools": [
                    { "name": "status.summary", "description": "Return a small status summary." },
                    { "name": "boom", "description": "Always returns an error result." },
                    { "name": "noisy", "description": "Writes to stderr, then returns ok." }
                ]}
            }),
            "tools/call" => {
                let name = req
                    .get("params")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let args = req
                    .get("params")
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                match name {
                    "boom" => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "content": [ { "type": "text", "text": "intentional failure" } ],
                            "isError": true
                        }
                    }),
                    "noisy" => {
                        let _ = writeln!(stderr, "relux-mcp-test-server: noisy diagnostic line");
                        let _ = stderr.flush();
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "content": [ { "type": "text", "text": "noisy ok" } ],
                                "isError": false
                            }
                        })
                    }
                    "status.summary" => {
                        let q = args.get("q").and_then(|q| q.as_str()).unwrap_or("none");
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "content": [ { "type": "text", "text": format!("status ok; q={q}") } ],
                                "structuredContent": { "ok": true },
                                "isError": false
                            }
                        })
                    }
                    other => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("no such tool: {other}") }
                    }),
                }
            }
            other => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }),
        };

        if writeln!(stdout, "{response}").is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }
}
