use serde::{Deserialize, Serialize};

/// What Prime understood the user to intend before taking any action.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §10.1 (Intent Layer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimeIntent {
    Greeting,
    StatusQuestion,
    TaskCreation,
    TaskUpdate,
    RunStart,
    RunRetry,
    AgentCreation,
    PluginInstallation,
    PermissionChange,
    ApprovalResponse,
    ExplanationRequest,
    DashboardNavigation,
    Brainstorming,
    DirectAnswer,
}

/// A concrete kernel action that Prime is authorized to invoke.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §10.2 (Action Layer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PrimeAction {
    InspectState,
    CreateTask {
        title: String,
    },
    UpdateTask {
        task_id: String,
        patch: String,
    },
    AssignTask {
        task_id: String,
        agent_id: String,
    },
    StartRun {
        task_id: String,
    },
    RetryRun {
        run_id: String,
    },
    CreateAgent {
        name: String,
        adapter_plugin: String,
    },
    InstallPlugin {
        plugin_id: String,
    },
    ConfigurePlugin {
        plugin_id: String,
    },
    GrantPermission {
        subject_id: String,
        permission: String,
    },
    RequestApproval {
        action: String,
        reason: String,
    },
    SummarizeRun {
        run_id: String,
    },
    ExplainBlocker {
        task_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prime_intent_serializes_cleanly() {
        let intent = PrimeIntent::TaskCreation;
        let json = serde_json::to_string(&intent).unwrap();
        assert_eq!(json, "\"task_creation\"");
        let back: PrimeIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PrimeIntent::TaskCreation);
    }

    #[test]
    fn prime_action_serializes_cleanly() {
        let action = PrimeAction::CreateTask {
            title: "Fix failing tests".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let back: PrimeAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }

    #[test]
    fn all_prime_intents_round_trip() {
        let intents = [
            PrimeIntent::Greeting,
            PrimeIntent::StatusQuestion,
            PrimeIntent::TaskCreation,
            PrimeIntent::TaskUpdate,
            PrimeIntent::RunStart,
            PrimeIntent::RunRetry,
            PrimeIntent::AgentCreation,
            PrimeIntent::PluginInstallation,
            PrimeIntent::PermissionChange,
            PrimeIntent::ApprovalResponse,
            PrimeIntent::ExplanationRequest,
            PrimeIntent::DashboardNavigation,
            PrimeIntent::Brainstorming,
            PrimeIntent::DirectAnswer,
        ];
        for intent in intents {
            let json = serde_json::to_string(&intent).unwrap();
            let back: PrimeIntent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, intent);
        }
    }
}
