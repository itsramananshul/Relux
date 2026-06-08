//! Run transcript events.
//!
//! `relux-core` defines the durable entities (Task, Run, Agent, ...) but does not
//! yet model the per-run transcript described in `docs/RELUX_MASTER_PLAN.md`
//! section 9.7 (Run Event). The kernel owns that timeline locally so the demo loop can
//! show how a run unfolds: started -> tool call -> completed.

use relux_core::RunId;
use serde::{Deserialize, Serialize};

/// A single timeline entry inside a run.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.7 (Run Event).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub id: String,
    pub run_id: RunId,
    pub ts: String,
    /// A coarse event kind, e.g. `run_started`, `tool_call`, `run_completed`.
    pub kind: String,
    /// Who emitted the event: `kernel` or an agent id.
    pub source: String,
    pub message: String,
    pub payload: serde_json::Value,
}
