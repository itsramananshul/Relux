//! SQLite-backed local persistence for the Relux kernel.
//!
//! This is the first concrete `PrimaryStorage` slice from the plugin model
//! (`docs/RELUX_MASTER_PLAN.md` section 8.3 ServiceProvider plugins, section 15 Phase 1:
//! "SQLite provider as default storage"). It lets repeated CLI invocations share
//! one durable control plane instead of each booting a throwaway in-memory
//! [`KernelState`] that forgets everything (section 17.8).
//!
//! The schema is deliberately minimal and robust for the MVP: typed entities are
//! stored as JSON blobs in id-keyed tables, the run transcript and audit log are
//! stored in emission order, and the deterministic counters live in a small
//! key/value table. A save rewrites the whole snapshot inside one transaction, so
//! the on-disk state is always a consistent export of the live kernel.

use std::path::Path;

use rusqlite::{params, Connection};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::event::RunEvent;
use crate::state::{KernelCounters, KernelSnapshot, KernelState};
use crate::KernelError;

/// The current on-disk schema version, recorded in `meta`.
const SCHEMA_VERSION: i64 = 1;

/// The schema for the local store. `IF NOT EXISTS` everywhere so `open` is
/// idempotent against an existing database file.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS meta              (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS plugins           (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS installed_plugins (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS namespaces        (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS agents       (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks        (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS runs         (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS approvals    (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS run_events   (id TEXT PRIMARY KEY, run_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit_events (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS counters     (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
";

/// The id-keyed JSON tables, cleared in order on every save.
const JSON_TABLES: &[&str] = &[
    "plugins",
    "installed_plugins",
    "namespaces",
    "agents",
    "tasks",
    "runs",
    "approvals",
    "run_events",
    "audit_events",
    "counters",
];

/// A durable, local SQLite store for one [`KernelState`].
pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Open (creating if needed) a store at `path`, ensuring the parent
    /// directory and schema exist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, KernelError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    KernelError::Storage(format!("create dir {}: {e}", parent.display()))
                })?;
            }
        }
        let conn = Connection::open(path).map_err(storage_err)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), KernelError> {
        self.conn.execute_batch(SCHEMA).map_err(storage_err)?;
        // Record the schema version once; leave an existing value untouched.
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES('schema_version', ?1) \
                 ON CONFLICT(key) DO NOTHING",
                params![SCHEMA_VERSION.to_string()],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Load the persisted [`KernelSnapshot`]. Returns an empty snapshot (all
    /// counters zero) for a freshly created store.
    pub fn load_snapshot(&self) -> Result<KernelSnapshot, KernelError> {
        Ok(KernelSnapshot {
            plugins: self.load_json("plugins")?,
            installed_plugins: self.load_json("installed_plugins")?,
            namespaces: self.load_json("namespaces")?,
            agents: self.load_json("agents")?,
            tasks: self.load_json("tasks")?,
            runs: self.load_json("runs")?,
            approvals: self.load_json("approvals")?,
            run_events: self.load_json("run_events")?,
            run_logs: self.load_meta_json("run_logs")?.unwrap_or_default(),
            audit_events: self.load_json("audit_events")?,
            prime_autonomy_config: self
                .load_meta_json("prime_autonomy_config")?
                .unwrap_or_default(),
            tool_runtime_configs: self
                .load_meta_json("tool_runtime_configs")?
                .unwrap_or_default(),
            adapter_runtime_configs: self
                .load_meta_json("adapter_runtime_configs")?
                .unwrap_or_default(),
            orchestrations: self.load_meta_json("orchestrations")?.unwrap_or_default(),
            pending_clarifications: self
                .load_meta_json("pending_clarifications")?
                .unwrap_or_default(),
            conversation_histories: self
                .load_meta_json("conversation_histories")?
                .unwrap_or_default(),
            conversation_summaries: self
                .load_meta_json("conversation_summaries")?
                .unwrap_or_default(),
            pending_tool_invocations: self
                .load_meta_json("pending_tool_invocations")?
                .unwrap_or_default(),
            persistent_grants: self.load_meta_json("persistent_grants")?.unwrap_or_default(),
            counters: self.load_counters()?,
        })
    }

    /// Load and rehydrate the full [`KernelState`].
    pub fn load(&self) -> Result<KernelState, KernelError> {
        Ok(KernelState::from_snapshot(self.load_snapshot()?))
    }

    /// Persist `snapshot`, replacing the whole on-disk state in one transaction.
    pub fn save_snapshot(&mut self, snapshot: &KernelSnapshot) -> Result<(), KernelError> {
        let tx = self.conn.transaction().map_err(storage_err)?;

        for table in JSON_TABLES {
            tx.execute(&format!("DELETE FROM {table}"), [])
                .map_err(storage_err)?;
        }

        for plugin in &snapshot.plugins {
            put_json(&tx, "plugins", plugin.id.as_str(), plugin)?;
        }
        for installed in &snapshot.installed_plugins {
            put_json(&tx, "installed_plugins", installed.id.as_str(), installed)?;
        }
        for namespace in &snapshot.namespaces {
            put_json(&tx, "namespaces", namespace.id.as_str(), namespace)?;
        }
        for agent in &snapshot.agents {
            put_json(&tx, "agents", agent.id.as_str(), agent)?;
        }
        for task in &snapshot.tasks {
            put_json(&tx, "tasks", task.id.as_str(), task)?;
        }
        for run in &snapshot.runs {
            put_json(&tx, "runs", run.id.as_str(), run)?;
        }
        for approval in &snapshot.approvals {
            put_json(&tx, "approvals", approval.id.as_str(), approval)?;
        }
        for event in &snapshot.run_events {
            put_run_event(&tx, event)?;
        }
        for event in &snapshot.audit_events {
            put_json(&tx, "audit_events", &event.id, event)?;
        }

        let c = &snapshot.counters;
        put_counter(&tx, "clock_secs", c.clock_secs)?;
        put_counter(&tx, "next_task", c.next_task)?;
        put_counter(&tx, "next_run", c.next_run)?;
        put_counter(&tx, "next_approval", c.next_approval)?;
        put_counter(&tx, "next_audit", c.next_audit)?;
        put_counter(&tx, "next_event", c.next_event)?;
        put_counter(&tx, "next_orchestration", c.next_orchestration)?;
        put_counter(&tx, "next_grant", c.next_grant)?;
        put_meta_json(&tx, "prime_autonomy_config", &snapshot.prime_autonomy_config)?;
        put_meta_json(&tx, "orchestrations", &snapshot.orchestrations)?;
        put_meta_json(
            &tx,
            "pending_clarifications",
            &snapshot.pending_clarifications,
        )?;
        put_meta_json(
            &tx,
            "conversation_histories",
            &snapshot.conversation_histories,
        )?;
        put_meta_json(
            &tx,
            "conversation_summaries",
            &snapshot.conversation_summaries,
        )?;
        put_meta_json(
            &tx,
            "pending_tool_invocations",
            &snapshot.pending_tool_invocations,
        )?;
        put_meta_json(&tx, "persistent_grants", &snapshot.persistent_grants)?;
        put_meta_json(&tx, "run_logs", &snapshot.run_logs)?;
        put_meta_json(&tx, "tool_runtime_configs", &snapshot.tool_runtime_configs)?;
        put_meta_json(
            &tx,
            "adapter_runtime_configs",
            &snapshot.adapter_runtime_configs,
        )?;

        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    /// Persist the live [`KernelState`].
    pub fn save(&mut self, state: &KernelState) -> Result<(), KernelError> {
        self.save_snapshot(&state.snapshot())
    }

    /// Read the `json` column from an id-keyed table, in stored (rowid) order,
    /// and deserialize each row. For the transcript and audit tables rowid order
    /// is the emission order the snapshot was saved in; for entity tables the
    /// order is irrelevant once rehydrated into maps.
    fn load_json<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>, KernelError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT json FROM {table} ORDER BY rowid"))
            .map_err(storage_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(storage_err)?;
            out.push(serde_json::from_str(&json).map_err(json_err)?);
        }
        Ok(out)
    }

    fn load_counters(&self) -> Result<KernelCounters, KernelError> {
        let mut counters = KernelCounters::default();
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM counters")
            .map_err(storage_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(storage_err)?;
        for row in rows {
            let (key, value) = row.map_err(storage_err)?;
            let value = value as u64;
            match key.as_str() {
                "clock_secs" => counters.clock_secs = value,
                "next_task" => counters.next_task = value,
                "next_run" => counters.next_run = value,
                "next_approval" => counters.next_approval = value,
                "next_audit" => counters.next_audit = value,
                "next_event" => counters.next_event = value,
                "next_orchestration" => counters.next_orchestration = value,
                "next_grant" => counters.next_grant = value,
                _ => {}
            }
        }
        Ok(counters)
    }

    fn load_meta_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, KernelError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM meta WHERE key = ?1")
            .map_err(storage_err)?;
        let mut rows = stmt.query(params![key]).map_err(storage_err)?;
        let Some(row) = rows.next().map_err(storage_err)? else {
            return Ok(None);
        };
        let json: String = row.get(0).map_err(storage_err)?;
        Ok(Some(serde_json::from_str(&json).map_err(json_err)?))
    }
}

fn put_json<T: Serialize>(
    conn: &Connection,
    table: &str,
    id: &str,
    value: &T,
) -> Result<(), KernelError> {
    let json = serde_json::to_string(value).map_err(json_err)?;
    conn.execute(
        &format!("INSERT INTO {table}(id, json) VALUES(?1, ?2)"),
        params![id, json],
    )
    .map_err(storage_err)?;
    Ok(())
}

fn put_run_event(conn: &Connection, event: &RunEvent) -> Result<(), KernelError> {
    let json = serde_json::to_string(event).map_err(json_err)?;
    conn.execute(
        "INSERT INTO run_events(id, run_id, json) VALUES(?1, ?2, ?3)",
        params![event.id, event.run_id.as_str(), json],
    )
    .map_err(storage_err)?;
    Ok(())
}

fn put_meta_json<T: Serialize>(
    conn: &Connection,
    key: &str,
    value: &T,
) -> Result<(), KernelError> {
    let json = serde_json::to_string(value).map_err(json_err)?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, json],
    )
    .map_err(storage_err)?;
    Ok(())
}

fn put_counter(conn: &Connection, key: &str, value: u64) -> Result<(), KernelError> {
    conn.execute(
        "INSERT INTO counters(key, value) VALUES(?1, ?2)",
        params![key, value as i64],
    )
    .map_err(storage_err)?;
    Ok(())
}

fn storage_err(e: rusqlite::Error) -> KernelError {
    KernelError::Storage(e.to_string())
}

fn json_err(e: serde_json::Error) -> KernelError {
    KernelError::Storage(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::namespace::NamespaceKind;
    use relux_core::permission::{ApprovalRequirement, RiskLevel, ToolDefinition};
    use relux_core::{
        Permission, PluginCapability, PluginHealth, PluginId, PluginKind, PluginManifest,
        PrimeContext, TrustLevel,
    };

    fn echo_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-echo"),
            name: "Echo".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "echo".to_string(),
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

    fn adapter_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-adapter-local-prime"),
            name: "Local Prime".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Adapter,
            description: "adapter".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![],
                permissions: vec![
                    Permission::new("adapter:relux-adapter-local-prime:run").unwrap(),
                ],
            },
            health: PluginHealth::Unknown,
        }
    }

    /// A kernel bootstrapped with plugins, a workspace namespace, and Prime.
    fn bootstrapped() -> (KernelState, PrimeContext) {
        let mut k = KernelState::new();
        k.register_plugin(echo_manifest());
        k.register_plugin(adapter_manifest());
        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        let adapter = PluginId::new("relux-adapter-local-prime");
        let prime = k
            .create_agent(
                "prime",
                "Prime",
                "The control-plane operator.",
                &adapter,
                &ns,
                None,
                vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
            )
            .unwrap();
        let ctx = PrimeContext {
            namespace: ns,
            agent: prime,
            actor: "founder".to_string(),
        };
        (k, ctx)
    }

    #[test]
    fn save_then_reopen_restores_state() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("local.db");

        // First process: bootstrap, do work, save.
        let task_id;
        {
            let (mut kernel, ctx) = bootstrapped();
            let turn = kernel
                .prime_turn(&ctx, "create a task to echo hello and run it")
                .unwrap();
            task_id = turn.created_task.expect("a task was created");

            let mut store = SqliteStore::open(&path).unwrap();
            store.save(&kernel).unwrap();
        }

        // Second process: a brand-new store handle over the same file sees the work.
        {
            let store = SqliteStore::open(&path).unwrap();
            let kernel = store.load().unwrap();
            assert_eq!(kernel.plugin_count(), 2);
            assert_eq!(kernel.agent_count(), 1);
            assert_eq!(kernel.task_count(), 1);
            assert_eq!(kernel.run_count(), 1);
            assert!(kernel.task(&task_id).is_some(), "task survived restart");
            assert!(!kernel.audit_log().is_empty(), "audit log persisted");
        }
    }

    #[test]
    fn fresh_store_loads_empty_state() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("local.db");
        let store = SqliteStore::open(&path).unwrap();
        let kernel = store.load().unwrap();
        assert_eq!(kernel.task_count(), 0);
        assert_eq!(kernel.agent_count(), 0);
        assert_eq!(kernel.plugin_count(), 0);
    }

    #[test]
    fn counters_resume_across_reopen_without_id_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("local.db");

        let first_task;
        {
            let (mut kernel, ctx) = bootstrapped();
            first_task = kernel
                .prime_turn(&ctx, "create a task to echo hello and run it")
                .unwrap()
                .created_task
                .unwrap();
            let mut store = SqliteStore::open(&path).unwrap();
            store.save(&kernel).unwrap();
        }

        // Reopen, act again: the next task id must advance, not collide.
        let store = SqliteStore::open(&path).unwrap();
        let mut kernel = store.load().unwrap();
        let ctx = PrimeContext {
            namespace: relux_core::NamespaceId::new("workspace"),
            agent: relux_core::AgentId::new("prime"),
            actor: "founder".to_string(),
        };
        let second_task = kernel
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap()
            .created_task
            .unwrap();
        assert_ne!(first_task, second_task);
    }
}
