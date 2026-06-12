//! Prime's deterministic brain: intent classification and grounded planning.
//!
//! `relux-core` defines Prime's domain types (`PrimeIntent`, `PrimeAction`,
//! `StateSummary`, `PrimePlan`); this module is the behavior that turns a user
//! message plus the current `StateSummary` into a `PrimePlan`. The kernel
//! ([`crate::KernelState::prime_turn`]) then executes that plan.
//!
//! Everything here is a deterministic, rule-based stand-in for the eventual
//! LLM-backed Prime. It is intentionally shaped like the real thing
//! (`docs/RELUX_MASTER_PLAN.md` section 10):
//!
//! - [`classify_intent`] is the Intent Layer (section 10.1): a message becomes one
//!   `PrimeIntent` before any action is considered.
//! - [`decide`] is the Action Layer (section 10.2) plus the Approval/Conversation rules
//!   (section 10.3, section 10.5): it grounds every reply in the `StateSummary`, executes only
//!   safe in-scope work, proposes risky actions behind approval, and never turns
//!   a greeting into a plan (section 17.1).
//!
//! No kernel access, no mutation, no network, no wall clock: pure functions of
//! `(message, StateSummary)`.

use relux_core::{
    plan_orchestration, OrchestrationPlan, PrimeAction, PrimeIntent, PrimePlan, RiskLevel,
    StateSummary, TaskBrief, TaskStatus,
};

/// Leading verbs that signal the user wants new work created.
///
/// Used in two places so they stay in lockstep: [`classify_intent`] treats a
/// bare imperative ("create", "fix") as task creation even with no object, and
/// [`task_title`] then refuses to mint a task whose whole title is just one of
/// these verbs - a verb with no object names no work, so Prime asks instead of
/// inventing a task titled after the verb (section 10.5).
const CREATION_VERBS: &[&str] = &[
    "create",
    "make",
    "add",
    "build",
    "fix",
    "implement",
    "investigate",
    "summarize",
    "review",
    "refactor",
    "write",
    "draft",
];

/// Classify a user message into a [`PrimeIntent`] (section 10.1, Intent Layer).
///
/// Deterministic and rule-based: the checks are ordered and the first match
/// wins, so more specific intents (status, explanation, run control) are tested
/// before the broad "this is work" task creation catch, and a bare greeting is
/// only matched once nothing more actionable has. This is the seam a real
/// classifier model will sit behind.
pub fn classify_intent(message: &str) -> PrimeIntent {
    let m = message.trim().to_lowercase();
    if m.is_empty() {
        return PrimeIntent::Greeting;
    }

    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    let starts = |p: &str| m.starts_with(p);
    // The leading run of alphanumerics, used so greetings match on a whole word
    // ("hey", "hi") and never on a substring ("this", "history").
    let first_word: String = m
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();

    if starts("approve") || starts("reject") || starts("deny") || starts("decline") {
        return PrimeIntent::ApprovalResponse;
    }
    if starts("why") || has(&["explain", "what went wrong", "what happened"]) {
        return PrimeIntent::ExplanationRequest;
    }
    // Tool discovery: the user wants to know which tools Prime can use. Checked
    // before task creation so "what tools can you use?" never mints a task.
    if has(&[
        "what tools",
        "which tools",
        "list tools",
        "list your tools",
        "available tools",
        "your tools",
        "tools do you have",
        "tools can you",
        "what can you run",
        "show me your tools",
        "show your tools",
    ]) {
        return PrimeIntent::ToolDiscovery;
    }
    // Explicit multi-tool plan: the user asked Prime to run SEVERAL named tools in
    // order ("run these tools in order: …", "use the status tool then the echo
    // tool", "chain these tools"). Requires BOTH an explicit plan/sequence cue AND
    // at least two segments that resolve to a real tool reference, so a single tool
    // request ("echo hello") stays `ToolInvocation` and casual chat that merely says
    // "then" never reaches here. Checked before single tool invocation so an ordered
    // multi-tool command previews an INERT plan rather than running as one tool. The
    // resulting turn is action-free: the kernel grounds + validates the plan and
    // attaches a reviewable card; only an explicit operator click creates a tool-run
    // task (`docs/mcp.md` "Run-driven multi-tool plan"; `docs/prime-processing-audit.md`
    // "Hermes-first general agent"; §10.5, §17.1).
    let plan_cue = has(&[
        "these tools",
        "the following tools",
        "tool plan",
        "tools in order",
        "multi-tool",
        "multi tool",
        "chain these",
        "chain the tools",
        "sequence of tools",
        "several tools",
        "multiple tools",
        "a few tools",
    ]);
    let seq_cue = has(&[
        " then ",
        " and then ",
        ", then ",
        " followed by ",
        ", followed by ",
        " after that ",
        ", after that ",
        " next, ",
    ]);
    if plan_cue || seq_cue {
        let resolved = split_tool_plan_segments(message)
            .iter()
            .filter(|s| parse_tool_request(s).is_some())
            .count();
        if resolved >= 2 {
            return PrimeIntent::ToolPlanRequest;
        }
    }
    // Tool invocation: the user wants Prime to RUN a specific tool. An explicit
    // `plugin/tool` reference, an echo/status request, or an "<verb> the X tool"
    // phrasing. Checked before task creation/run control so "echo hello" runs the
    // echo tool instead of being read as new work, while "run it"/"start it"
    // (no tool referenced) still fall through to run control.
    let invoke_verb = first_word == "echo"
        || starts("use ")
        || starts("run ")
        || starts("invoke ")
        || starts("call ")
        || starts("test ")
        || starts("execute ");
    // An explicit SINGLE MCP tool reference ("use mcp:loopback/status.summary",
    // "call mcp:fs/search with {…}", or a bare "mcp:loopback/echo.say"). Recognized via
    // the SAME `parse_tool_request` resolver the multi-tool plan path uses, so the
    // accepted ref form is exactly `mcp:<server>/<tool>` (the stable synthetic
    // `mcp:<server>` plugin id, mirroring openclaw's `mcp:<serverId>:<toolName>` ref).
    // The multi-tool plan block above already claimed a message that names ≥2 tools
    // behind a plan/sequence cue, so only a single MCP ref reaches here. It is gated so
    // a deliberative question or a guarded musing never invokes — an explicit invoke
    // verb forces it, otherwise the message must NOT be chat-guarded ([`is_chat_guarded`]:
    // a question / ideation / venting / chitchat without an explicit command). Grounding
    // + every gate happen later in the kernel (`prime_invoke_tool` resolves the ref against
    // the off-lock live MCP catalog); a server/tool that is not live fails closed with a
    // clean message, never a raw dump (`docs/mcp.md` "Invocation"; §10.5, §17.1).
    let references_mcp_tool = parse_tool_request(message)
        .is_some_and(|(plugin, tool, _)| plugin.starts_with("mcp:") && !tool.is_empty());
    if has(&["echo.say", "status.summary"])
        || (first_word == "echo")
        || (invoke_verb && has(&[" tool", "echo", "status tool"]))
        || (invoke_verb && m.contains("relux-tools-"))
        || has(&["use the echo", "use the status", "the echo tool", "the status tool"])
        || (references_mcp_tool && (invoke_verb || !is_chat_guarded(message)))
    {
        return PrimeIntent::ToolInvocation;
    }
    // Ideation / musing stays a conversation (section 10.5: "be natural, but not
    // reckless" - Prime does not create plans from casual chat). A lead-in like
    // "I was thinking...", "what if we...", "I have an idea..." means the user is
    // floating an idea, not commanding work, so even a sentence carrying a creation
    // verb ("create", "build", "make") classifies as Brainstorming. Only an
    // EXPLICIT command ("create a task to ...", "orchestrate", "assign", "run it")
    // overrides this and mints/runs work. Checked before orchestration and task
    // creation so the verb-based catches below cannot turn musing into a task.
    if is_ideation(&m) && !is_explicit_command(&m) {
        return PrimeIntent::Brainstorming;
    }
    // Orchestration: the user explicitly wants Prime to coordinate a goal across
    // multiple agents. Gated on explicit coordination phrasing (never a bare verb)
    // and checked before task creation/agent creation so "orchestrate X, Y and Z"
    // is decomposed into briefs instead of being read as one task. The pure planner
    // ([`relux_core::plan_orchestration`]) still decides whether the goal actually
    // splits; a non-splittable goal becomes a clarifying question, not a storm.
    if has(&[
        "orchestrate",
        "coordinate",
        "split this across",
        "split across agents",
        "split it across",
        "divide this",
        "divide it across",
        "fan out",
        "fan this out",
        "parallelize",
        "in parallel across",
        "multiple agents",
        "several agents",
        "across agents",
        "across the agents",
        "assign across",
        "set up a team",
        "spin up a team",
        "have the team",
        "have agents",
        "team of agents",
        "delegate across",
        "plan and assign",
    ]) {
        return PrimeIntent::Orchestration;
    }
    // Explicit plan request (section 10 planning layer, section 11.1): the user
    // wants Prime to lay an idea out as a REVIEWABLE plan before any work is
    // created - the first-class "idea -> plan -> tasks" rung. Checked AFTER
    // orchestration (so "plan and assign" still commits) and BEFORE task creation
    // (so "make a plan to build X" previews a plan instead of minting one task).
    // The decide() arm is action-free: nothing is created or run until the user
    // confirms with the one-click "Create these tasks" suggestion.
    if has(&[
        "plan this out",
        "plan it out",
        "make a plan",
        "draft a plan",
        "come up with a plan",
        "put together a plan",
        "give me a plan",
        "outline the steps",
        "outline a plan",
        "lay out the steps",
        "lay out a plan",
    ]) || starts("plan ")
    {
        return PrimeIntent::PlanRequest;
    }
    if has(&["and run it", "and start it", "and execute it"])
        && (has(&["create", "make", "add", "new task"])
            || CREATION_VERBS.contains(&first_word.as_str()))
    {
        return PrimeIntent::CreateAndRunTask;
    }
    if has(&[
        "hire",
        "spawn",
        "create an agent",
        "create agent",
        "new agent",
        "another agent",
        "add an agent",
        "crew member",
        "run claude on",
        "run codex on",
    ]) {
        return PrimeIntent::AgentCreation;
    }
    if starts("assign")
        || starts("delegate")
        || has(&["assign task", "delegate task", "assign ", "delegate "])
    {
        return PrimeIntent::AssignTask;
    }
    // Status question: grounded in live runs/tasks. Checked BEFORE the task-creation
    // catch and the conversation guard below, so "give me a status of the build"
    // reports state instead of being read as work (off "build") or swallowed by the
    // question guard as a thing to discuss.
    if m == "status"
        || has(&[
            "what is going on",
            "whats going on",
            "what's going on",
            "what is running",
            "whats running",
            "what's running",
            "active run",
            "show me active",
            "anything running",
            "what is happening",
            "whats happening",
            "what do we have",
            "give me a status",
        ])
    {
        return PrimeIntent::StatusQuestion;
    }
    // Conversation guard (section 10.5; section 17.1: "Prime must understand
    // conversational intent" and "must not blindly turn every message into a
    // plan"). A QUESTION - "how does the build work?", "what's the best way to fix
    // the tests?", "should we refactor this?" - is the user asking or deliberating,
    // not commanding work. So even when it carries a work verb it must NOT be minted
    // into a task by the catch below; with no explicit command it becomes a
    // Brainstorming conversation - Prime engages the idea and offers a one-click
    // "turn this into a task", creating nothing. An explicit command in the same
    // breath ("can you create a task to fix X") still acts, because
    // [`is_explicit_command`] (and the specific intents above) already claimed it.
    // Status/explanation/tool questions are classified above, so the guard never
    // swallows them.
    if is_question(&m) && !is_explicit_command(&m) {
        return PrimeIntent::Brainstorming;
    }
    // Orchestration RUN/continue: the user wants Prime to START (or continue) the
    // governed batch for an EXISTING orchestration — distinct from CREATING one above
    // (which is keyed on "orchestrate"/"coordinate"/…). Keyed on a run/continue verb
    // together with the orchestration noun, or an explicit `orch_` id reference. Checked
    // AFTER the conversation guard (so "should we run the orchestration?" stays a
    // conversation) and BEFORE the task-creation / run-start catches (so the batch verb
    // is never read as new work or a single-task run). The kernel validates the id
    // against the live records before running anything.
    let run_verb = has(&[
        "run ", "start ", "continue", "resume", "execute", "kick off", "go ahead",
    ]);
    if run_verb && (extract_orchestration_id(&m).is_some() || m.contains("orchestration")) {
        return PrimeIntent::OrchestrationRun;
    }
    // By-id task UPDATE: a message that names a SPECIFIC existing-looking task
    // (`task_…`) and a field to change is an edit, not new work — checked BEFORE the
    // broad task-creation catch so "rename task_0001 to Fix the login page" is not
    // misread as creating a task off the embedded verb "fix". Anchored on a real
    // `task_…` reference + an update-field word so it never swallows casual chat; a
    // question ("should I cancel task_0001?") was already routed to Brainstorming by the
    // conversation guard above.
    if extract_task_id(&m).is_some()
        && (m.contains("priority")
            || m.contains("rename")
            || m.contains("retitle")
            || m.contains("title")
            || m.contains("reassign")
            || m.contains("assignee")
            || m.contains("cancel")
            || m.contains("block")
            || m.contains("status")
            || m.contains("detail")
            || m.contains("description")
            || m.contains("notes"))
    {
        return PrimeIntent::TaskUpdate;
    }
    // Task creation: the broad "this is work" catch. A work verb counts only as a
    // WHOLE WORD ("please fix the login bug"), never a substring - so "the prefix
    // is wrong" (fix), "show me a preview" (review), "the building plan" (build),
    // and "it fixes the crash" (fix) are no longer misread as new work off an
    // embedded verb. Explicit task phrases still match directly.
    if has(&["create a task", "make a task", "new task", "add a task", "set up", "update the"])
        || CREATION_VERBS.iter().any(|v| has_word(&m, v))
    {
        return PrimeIntent::TaskCreation;
    }
    if has(&["retry", "try again", "rerun", "re-run"]) {
        return PrimeIntent::RunRetry;
    }
    if m == "start"
        || m == "run"
        || m == "go"
        || m == "continue"
        || starts("continue ")
        || has(&[
            "start it",
            "start the run",
            "start run",
            "run it",
            "kick off",
            "go ahead and run",
        ])
    {
        return PrimeIntent::RunStart;
    }
    if has(&[
        "install ",
        "add plugin",
        "enable plugin",
        "install the plugin",
    ]) {
        return PrimeIntent::PluginInstallation;
    }
    if has(&[
        "grant",
        "permission",
        "give it access",
        "give this agent",
        "access to",
        " access",
        "revoke",
        "scope it",
    ]) {
        return PrimeIntent::PermissionChange;
    }
    if has(&[
        "update task",
        "change task",
        "reassign",
        "set priority",
        "rename task",
        "mark ",
    ]) {
        return PrimeIntent::TaskUpdate;
    }
    if has(&[
        "show me the board",
        "open the",
        "go to",
        "navigate",
        "take me to",
        "show board",
        "show the board",
    ]) {
        return PrimeIntent::DashboardNavigation;
    }
    if has(&[
        "brainstorm",
        "ideas",
        "what should we",
        "how should we",
        "think about",
        "what could we",
    ]) {
        return PrimeIntent::Brainstorming;
    }
    if matches!(
        first_word.as_str(),
        "hey" | "hi" | "hello" | "yo" | "sup" | "gm" | "howdy" | "hiya"
    ) || has(&["good morning", "good evening", "good afternoon"])
    {
        return PrimeIntent::Greeting;
    }
    // Emotional / casual conversation — the Hermes-first non-action categories.
    // Checked LAST, after every action / status / explanation / question /
    // brainstorm-keyword / greeting rail, so an explicit command or a real question
    // always wins and only genuine chitchat or venting reaches here. Venting /
    // insults / frustration are `EmotionalSupport`; throwaway affirmations and light
    // chitchat are `SmallTalk`. Neither ever mints or runs work — they are deliberate
    // conversational intents, not pseudo-brainstorming (`docs/prime-processing-audit.md`
    // "Hermes-first general agent"; §10.5, §17.1).
    if is_emotional_distress(&m) {
        return PrimeIntent::EmotionalSupport;
    }
    if is_casual_chat(&m) {
        return PrimeIntent::SmallTalk;
    }

    PrimeIntent::DirectAnswer
}

/// Decide what Prime should do, grounded in the current [`StateSummary`].
///
/// Pure: no kernel access and no mutation. The kernel turns the returned
/// [`PrimePlan`] into real state changes. The mapping enforces the Prime rules:
///
/// - greetings/status/explanations are grounded replies, never plans (section 10.5, section 17.1);
/// - task creation and a single ready run are safe `Act`s the kernel executes;
/// - granting permissions, installing plugins, and spawning agents are `Propose`d
///   behind a human approval and never done silently (section 10.3);
/// - ambiguous run control becomes a `Clarify` instead of a guess.
pub fn decide(message: &str, intent: &PrimeIntent, summary: &StateSummary) -> PrimePlan {
    match intent {
        // A greeting is just conversation. Prime answers like a general agent
        // (Hermes-first), NOT a work-board bot: it does not volunteer the
        // board/queue/crew state or ask "what do you want to set up" — that belongs
        // to a status question the user explicitly asks. It stays warm and
        // open-ended so casual chat is never dragged into company setup
        // (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §17.1).
        PrimeIntent::Greeting => PrimePlan::Reply {
            text: greeting_text().to_string(),
        },
        // A status question is grounded by actually consulting the read-only
        // `status.summary` tool through the kernel (§11.1 plugin/action results).
        // The prose answer rides along as `text` so the reply stays readable and
        // falls back cleanly to prose if the tool cannot run.
        PrimeIntent::StatusQuestion => PrimePlan::Act {
            action: PrimeAction::InvokeTool {
                plugin_id: "relux-tools-status".to_string(),
                tool_name: "status.summary".to_string(),
                input_json: "{}".to_string(),
            },
            text: status_text(summary),
        },
        PrimeIntent::ExplanationRequest => PrimePlan::Reply {
            text: explanation_text(summary),
        },
        PrimeIntent::TaskCreation => match task_title(message) {
            Some(title) => PrimePlan::Act {
                action: PrimeAction::CreateTask {
                    title: title.clone(),
                },
                text: format!("Creating a task: \"{title}\"."),
            },
            // A bare verb with no object ("create", "fix") is not actionable
            // work, so Prime asks rather than minting a junk task (section 10.5).
            None => PrimePlan::Clarify {
                text: "What should I create? Tell me the task - for example \"create a task to summarize the README\"."
                    .to_string(),
            },
        },
        PrimeIntent::CreateAndRunTask => match task_title(message) {
            Some(title) => {
                let title = title.replace(" and run it", "");
                let title = title.replace(" and start it", "");
                let title = title.replace(" and execute it", "");
                PrimePlan::Act {
                    action: PrimeAction::CreateAndRunTask {
                        title: title.clone(),
                    },
                    text: format!("Creating and running task: \"{title}\"."),
                }
            }
            None => PrimePlan::Clarify {
                text: "What should I create and run?".to_string(),
            },
        },
        PrimeIntent::RunStart => {
            // Honor an explicit task id when the user (or a continued clarification) named
            // one: it must exist AND be ready to start (queued). An existing-but-not-ready
            // id is reported honestly; an unknown id fails closed. Only when no id is named
            // do we fall back to the ready-queue heuristic.
            match extract_task_id(message) {
                Some(id) => {
                    if let Some(b) = summary.queued.iter().find(|b| b.id.0 == id) {
                        PrimePlan::Act {
                            action: PrimeAction::StartRun { task_id: id.clone() },
                            text: format!("Starting \"{}\" ({}).", b.title, b.id),
                        }
                    } else if summary.all_task_ids.contains(&id) {
                        PrimePlan::Reply {
                            text: format!(
                                "Task {id} is not ready to start. Assign it to an agent first, or it may already be running or finished."
                            ),
                        }
                    } else {
                        PrimePlan::Reply {
                            text: format!("Task with ID '{id}' does not exist. Please provide a valid task ID."),
                        }
                    }
                }
                None => match summary.queued.as_slice() {
                    [] => PrimePlan::Clarify {
                        text: "There is no task ready to start. Create a task first, or assign an existing one to an agent."
                            .to_string(),
                    },
                    [one] => PrimePlan::Act {
                        action: PrimeAction::StartRun {
                            task_id: one.id.0.clone(),
                        },
                        text: format!("Starting \"{}\" ({}).", one.title, one.id),
                    },
                    many => PrimePlan::Clarify {
                        text: format!(
                            "More than one task is ready. Which should I start? {}",
                            list_briefs(many)
                        ),
                    },
                },
            }
        }
        PrimeIntent::RunRetry => {
            if summary.tasks_failed == 0 {
                PrimePlan::Reply {
                    text: "Nothing has failed, so there is no run to retry.".to_string(),
                }
            } else {
                let failed: Vec<TaskBrief> = summary
                    .recent
                    .iter()
                    .filter(|b| b.status == TaskStatus::Failed)
                    .cloned()
                    .collect();
                PrimePlan::Clarify {
                    text: format!(
                        "These tasks have failed: {}. Tell me which one to retry.",
                        list_briefs(&failed)
                    ),
                }
            }
        }
        PrimeIntent::AgentCreation => {
            let name = derive_agent_name(message);
            PrimePlan::Act {
                action: PrimeAction::CreateAgent {
                    name: name.clone(),
                    adapter_plugin: "relux-adapter-local-prime".to_string(),
                },
                text: format!("Creating agent \"{name}\" on the local adapter."),
            }
        }
        PrimeIntent::PluginInstallation => {
            let plugin = derive_plugin_id(message);
            PrimePlan::Propose {
                action: PrimeAction::InstallPlugin {
                    plugin_id: plugin.clone(),
                },
                reason: "Installing a plugin adds third-party capabilities and new code paths to the control plane."
                    .to_string(),
                risk: RiskLevel::High,
                text: format!("I can install the plugin {plugin}."),
            }
        }
        PrimeIntent::PermissionChange => {
            let subject = if message.to_lowercase().contains("agent") {
                derive_agent_name(message)
            } else {
                "(unspecified subject)".to_string()
            };
            let permission = derive_permission_label(message);
            PrimePlan::Propose {
                action: PrimeAction::GrantPermission {
                    subject_id: subject.clone(),
                    permission: permission.clone(),
                },
                reason: "Granting a permission widens what an actor can do and must be reviewed."
                    .to_string(),
                risk: RiskLevel::High,
                text: format!("I can grant {permission} to {subject}."),
            }
        }
        PrimeIntent::AssignTask => {
            let parsed_task_id = extract_task_id(message);
            // The first-word extractor is kept ONLY as the "did the user name an agent?"
            // presence signal for the clarify branches; the actual resolution uses the
            // full phrase matched against the live roster (so a fuzzy "the researcher"
            // resolves where the bare first word "the" never could).
            let agent_phrase_present = extract_agent_id_from_assignment(message).is_some();

            match (parsed_task_id, agent_phrase_present) {
                (Some(task_id_str), true) => {
                    if !summary.all_task_ids.contains(&task_id_str) {
                        PrimePlan::Reply {
                            text: format!("Task with ID '{}' does not exist. Please provide a valid task ID.", task_id_str),
                        }
                    } else {
                        // Roster-aware fuzzy resolution of the named assignee: exact →
                        // unique prefix → unique substring, ambiguity asked not guessed,
                        // and a resolved id is always one that exists (fail closed).
                        let phrase = extract_assignee_phrase(message).unwrap_or_default();
                        match resolve_assignee(&phrase, &summary.all_agent_ids, &summary.agent_skills) {
                            AssigneeResolution::Resolved(agent_id) => PrimePlan::Act {
                                action: PrimeAction::AssignTask {
                                    task_id: task_id_str.clone(),
                                    agent_id: agent_id.clone(),
                                },
                                text: format!("Assigning task {} to agent {}.", task_id_str, agent_id),
                            },
                            AssigneeResolution::Ambiguous(mut matches) => {
                                matches.sort();
                                PrimePlan::Clarify {
                                    text: format!(
                                        "More than one agent matches \"{}\": {}. Which one should I assign {} to?",
                                        phrase,
                                        matches.join(", "),
                                        task_id_str,
                                    ),
                                }
                            }
                            AssigneeResolution::Unresolved => PrimePlan::Reply {
                                text: format!("Agent with ID '{}' does not exist. Please provide a valid agent name.", phrase),
                            },
                        }
                    }
                }
                (None, true) => PrimePlan::Clarify {
                    text: "I couldn't find a task ID in your request. Please specify the task to assign.".to_string(),
                },
                (Some(_), false) => PrimePlan::Clarify {
                    text: "I couldn't find an agent name in your request. Please specify which agent to assign the task to.".to_string(),
                },
                (None, false) => PrimePlan::Clarify {
                    text: "I need both a task ID and an agent name to assign a task. Please rephrase your request.".to_string(),
                },
            }
        },
        // A by-id task update is a real, SAFE mutating action: the deterministic rail
        // parses a simple command ("rename task_0001 to X", "set task_0001 priority to
        // 8", "cancel task_0001"), validates every piece against the live state, and
        // produces an `UpdateTask` `Act`. An unknown task / agent fails closed with an
        // honest reply; a non-settable status (e.g. "mark it done") is honestly refused
        // (never faked into a completion); a missing task/field asks one concrete
        // question (a resolvable clarify the memory + brain can continue). The kernel
        // applies the validated patch and enforces the terminal-state guard.
        PrimeIntent::TaskUpdate => match crate::prime_update_slots::deterministic_update(message, summary) {
            crate::prime_update_slots::DeterministicUpdate::Resolved(resolved) => PrimePlan::Act {
                action: PrimeAction::UpdateTask {
                    task_id: resolved.task_id.clone(),
                    patch: resolved.patch.to_patch_string(),
                },
                text: format!("Updating {}.", resolved.task_id),
            },
            crate::prime_update_slots::DeterministicUpdate::UnknownTask(id) => PrimePlan::Reply {
                text: format!("Task with ID '{id}' does not exist. Please provide a valid task ID."),
            },
            crate::prime_update_slots::DeterministicUpdate::AmbiguousAssignee { phrase, matches } => {
                PrimePlan::Clarify {
                    text: format!(
                        "More than one agent matches \"{}\": {}. Which one should I reassign it to?",
                        phrase,
                        matches.join(", "),
                    ),
                }
            }
            crate::prime_update_slots::DeterministicUpdate::UnknownAssignee(phrase) => PrimePlan::Reply {
                text: format!("Agent with ID '{phrase}' does not exist. Please provide a valid agent name."),
            },
            crate::prime_update_slots::DeterministicUpdate::RejectedStatus(label) => PrimePlan::Reply {
                text: format!(
                    "I can't set a task to {label} from chat — that happens through the run lifecycle. I can cancel or block a task, or change its priority, title, details, or assignee."
                ),
            },
            crate::prime_update_slots::DeterministicUpdate::NeedsClarification => PrimePlan::Clarify {
                text: task_update_clarify(message),
            },
        },
        PrimeIntent::DashboardNavigation => PrimePlan::Reply {
            text: "The board, runs, approvals, and audit log are the operating surfaces. The dashboard UI is a later slice; for now I can summarize any of them."
                .to_string(),
        },
        // Brainstorming stays a conversation (section 10.5): Prime engages the
        // idea and helps shape it, but creates nothing until the user confirms.
        // The kernel attaches a one-click "turn this into a task" suggestion
        // (section 11.1) so musing flows into work without retyping a command.
        PrimeIntent::Brainstorming => PrimePlan::Reply {
            text: brainstorm_reply(message),
        },
        // Orchestration: decompose the goal across agents. The pure planner decides
        // whether the goal genuinely splits into multiple briefs; only a real
        // multi-agent plan becomes an `Act` (creating briefs is safe and in-scope,
        // like single-task creation). A non-splittable goal clarifies instead of
        // fanning out (section 10.4, section 10.5).
        PrimeIntent::Orchestration => {
            let goal = orchestration_goal(message);
            let plan = plan_orchestration(&goal, summary);
            if plan.is_multi_agent() {
                let agents = plan.agent_labels();
                PrimePlan::Act {
                    action: PrimeAction::OrchestrateGoal { goal: goal.clone() },
                    text: format!(
                        "Planning an orchestration for \"{goal}\": {} briefs across {}.",
                        plan.steps.len(),
                        count_phrase(agents.len(), "agent"),
                    ),
                }
            } else {
                PrimePlan::Clarify {
                    text: orchestration_clarify(&goal),
                }
            }
        }
        // Orchestration RUN/continue: start the governed batch for an EXISTING
        // orchestration. When the user named an `orch_…` id it becomes a `RunOrchestration`
        // `Act` (the kernel validates the id against the live records at execute time, the
        // same way `OrchestrateGoal` defers the multi-agent check); when no id was named it
        // is a resolvable `Clarify` that the multi-turn memory + a bare-id follow-up
        // continue. Running is a SAFE, in-scope action (the same governed batch the blocking
        // `/run` API and the CLI drive); each brief still gates at run time (section 10.4).
        PrimeIntent::OrchestrationRun => match extract_orchestration_id(message) {
            Some(id) => PrimePlan::Act {
                action: PrimeAction::RunOrchestration {
                    orchestration_id: id.clone(),
                },
                text: format!("Running orchestration {id}."),
            },
            None => PrimePlan::Clarify {
                text: "Which orchestration should I run? Name it by id (for example orch_0001); ask me to list the orchestrations if you're not sure."
                    .to_string(),
            },
        },
        // Plan request: lay the idea out as a REVIEWABLE plan that creates nothing
        // (section 10 planning layer, section 11.1). The pure planner decides whether
        // the goal genuinely splits: a multi-step goal becomes a plan preview the user
        // commits with one explicit click (the kernel attaches a "Create these tasks"
        // suggestion that routes the existing orchestration `Act`), a single-step goal
        // is steered to the one-task path. Either way this turn is action-free - nothing
        // is minted or run until the user confirms (section 10.5, section 17.1).
        PrimeIntent::PlanRequest => {
            let goal = plan_goal(message);
            let plan = plan_orchestration(&goal, summary);
            if plan.is_multi_agent() {
                PrimePlan::Reply {
                    text: plan_preview_text(&goal, &plan),
                }
            } else {
                PrimePlan::Reply {
                    text: plan_single_text(&goal),
                }
            }
        }
        PrimeIntent::ApprovalResponse => PrimePlan::Clarify {
            text: "Tell me the approval id to approve or reject.".to_string(),
        },
        // Listing tools needs the live installed-plugin index, which `decide`
        // (pure) does not hold - the kernel fills it from `discover_tools` when it
        // executes this safe, read-only action (§7.4, §11.1).
        PrimeIntent::ToolDiscovery => PrimePlan::Act {
            action: PrimeAction::DiscoverTools,
            text: "Here are the tools I can use right now.".to_string(),
        },
        // Explicit multi-tool plan request: build an INERT, grounded preview the
        // operator commits with one explicit click. `decide` is pure and cannot reach
        // the live tool registry, so it only carries the request forward as a
        // `ProposeToolPlan` action; the kernel grounds + validates every step against
        // `discover_tools` and attaches the reviewable card when it executes this
        // READ-ONLY action. The turn creates nothing and runs nothing — execution
        // happens only later, through the existing tool-run task path and its
        // unchanged gates (`docs/mcp.md` "Run-driven multi-tool plan"; §10.5, §17.1).
        PrimeIntent::ToolPlanRequest => PrimePlan::Act {
            action: PrimeAction::ProposeToolPlan {
                goal: message.trim().to_string(),
            },
            text: "Here's the tool plan I'd run — review the steps and create the tool-run task when you're ready.".to_string(),
        },
        // Running a tool is a safe, in-scope `Act`: the kernel executes only the
        // resolved built-in handler through the permission/audit path, and reports
        // an installed-but-unimplemented tool honestly. When the message names no
        // tool Prime can map, ask instead of guessing.
        PrimeIntent::ToolInvocation => match parse_tool_request(message) {
            Some((plugin_id, tool_name, input_json)) => {
                let label = if tool_name.is_empty() {
                    format!("the {plugin_id} tool")
                } else {
                    format!("{plugin_id}/{tool_name}")
                };
                PrimePlan::Act {
                    action: PrimeAction::InvokeTool {
                        plugin_id,
                        tool_name,
                        input_json,
                    },
                    text: format!("Running {label}."),
                }
            }
            None => PrimePlan::Clarify {
                text: "Which tool should I run? For example I can run relux-tools-status/status.summary; other installed tools are listed on the Plugins page but are not all runnable here yet."
                    .to_string(),
            },
        },
        // Throwaway, casual chitchat. Prime answers lightly and stays in the
        // conversation — a plain non-action reply with no board/queue/crew mention
        // and (via `attach_suggestions`) no work CTA. Hermes-first: chitchat is
        // chitchat (`docs/prime-processing-audit.md` "Hermes-first general agent";
        // §10.5, §17.1).
        PrimeIntent::SmallTalk => PrimePlan::Reply {
            text: small_talk_text().to_string(),
        },
        // Venting / frustration / an insult. Prime answers like a normal person —
        // a brief, human acknowledgement — and offers, at most, contextual NON-action
        // chips (`attach_suggestions`: "Tell me what broke" / "Show me the last run"),
        // never a task/plan/run CTA. Hermes-first: emotional chat is chat, never a
        // work prompt (`docs/prime-processing-audit.md` "Hermes-first general agent";
        // §10.5, §17.1).
        PrimeIntent::EmotionalSupport => PrimePlan::Reply {
            text: emotional_support_text().to_string(),
        },
        // The general catch-all. Prime answers as a broad assistant FIRST: when a
        // brain is configured it writes the real answer; this deterministic fallback
        // keeps a Hermes-like, general framing and mentions the control plane only as
        // one of the things it CAN do, never as the only thing and never as a prompt
        // to set up work (§10.5, §17.1).
        PrimeIntent::DirectAnswer => PrimePlan::Reply {
            text: "I'm Prime, your local agent — ask me anything, think out loud with me, or have me help with whatever you're working on. I can also drive your Relux control plane (tasks, runs, agents, plugins) whenever you want that."
                .to_string(),
        },
    }
}

// --- Grounded text builders ------------------------------------------------
//
// These render the `StateSummary` into Prime's "voice". They speak only about
// what is actually in state, so Prime cannot invent runs, tasks, or plugins.

/// The grounded "There are N ... ." sentence used in greetings and status.
fn headline(s: &StateSummary) -> String {
    let mut parts: Vec<String> = Vec::new();
    if s.runs_active > 0 {
        parts.push(count_phrase(s.runs_active, "active run"));
    }
    if s.tasks_open > 0 {
        parts.push(count_phrase(s.tasks_open, "open task"));
    }
    if s.pending_approvals > 0 {
        parts.push(format!(
            "{} awaiting approval",
            count_phrase(s.pending_approvals, "item")
        ));
    }
    if s.tasks_blocked > 0 {
        parts.push(count_phrase(s.tasks_blocked, "blocked task"));
    }
    if s.tasks_failed > 0 {
        parts.push(count_phrase(s.tasks_failed, "failed task"));
    }
    if parts.is_empty() {
        return "Nothing is running yet; the control plane is idle.".to_string();
    }
    format!("There are {}.", join_list(&parts))
}

fn status_text(s: &StateSummary) -> String {
    let mut t = headline(s);
    if !s.queued.is_empty() {
        t.push_str(&format!(" Ready to start: {}.", list_briefs(&s.queued)));
    }
    if s.pending_approvals > 0 {
        t.push_str(" Some approvals need a decision.");
    }
    t
}

fn explanation_text(s: &StateSummary) -> String {
    if let Some(b) = s.recent.iter().find(|b| b.status == TaskStatus::Failed) {
        return format!(
            "The most recent failure is {} \"{}\". I do not have the run-level error in this view; open the run transcript for the exact cause. I can retry it or mark it blocked.",
            b.id, b.title
        );
    }
    if let Some(b) = s.recent.iter().find(|b| b.status == TaskStatus::Blocked) {
        return format!(
            "{} \"{}\" is blocked. Tell me to unblock or reassign it.",
            b.id, b.title
        );
    }
    "Nothing is blocked or failed right now, so there is nothing to explain.".to_string()
}

fn list_briefs(briefs: &[TaskBrief]) -> String {
    let items: Vec<String> = briefs
        .iter()
        .map(|b| format!("{} \"{}\"", b.id, b.title))
        .collect();
    join_list(&items)
}

/// `"1 active run"` / `"3 active runs"` (naive pluralization, ASCII English).
pub(crate) fn count_phrase(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Join with an Oxford-style "and": `[a]`->`a`, `[a,b]`->`a and b`,
/// `[a,b,c]`->`a, b, and c`.
fn join_list(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let (last, head) = parts.split_last().expect("len > 2");
            format!("{}, and {}", head.join(", "), last)
        }
    }
}

// --- Field extractors ------------------------------------------------------
//
// Crude, deterministic pulls from the raw message. A real Prime would let the
// model fill these slots; here they are just enough to ground the demo.

/// True when the message is ideation/musing rather than a command (section 10.5).
///
/// These lead-ins open a discussion or float an idea ("I was thinking...",
/// "what if we...", "I have an idea..."). On their own they classify as
/// [`PrimeIntent::Brainstorming`] - a conversational reply - even when the
/// sentence also contains a creation verb, so Prime does not mint a task from
/// someone thinking out loud. An [`is_explicit_command`] in the same message
/// overrides this. Matched against the already-lowercased message.
fn is_ideation(m: &str) -> bool {
    const LEAD_INS: &[&str] = &[
        "i was thinking",
        "i'm thinking",
        "im thinking",
        "i am thinking",
        "i've been thinking",
        "ive been thinking",
        "i have been thinking",
        "i was wondering",
        "i wonder if",
        "i wonder whether",
        "what if we",
        "what if i",
        "what if you",
        "can we talk about",
        "could we talk about",
        "let's talk about",
        "lets talk about",
        "let's discuss",
        "lets discuss",
        "i want to talk about",
        "i want to discuss",
        "i'd like to discuss",
        "i would like to discuss",
        "i want to brainstorm",
        "i have an idea",
        "i had an idea",
        "i've got an idea",
        "ive got an idea",
        "i got an idea",
        "just thinking",
        "thinking about",
        "thinking of",
        "thinking we could",
        "toying with",
        "kicking around",
        "playing with the idea",
        "was thinking maybe",
        // Declarative soft-intent openers: stating a wish or a "we could / let's"
        // suggestion is musing, not a command, so it stays a conversation unless an
        // explicit command rides along (the caller gates on !is_explicit_command).
        // "i want to start it" / "let's create a task to X" still act because the
        // command wins; "i want to build X" / "we should refactor auth" become a
        // Brainstorming conversation that offers a one-click task instead of minting
        // one (section 10.5, section 17.1).
        "i want to",
        "i'd like to",
        "i would like to",
        "we should",
        "we could",
        "we might",
        "i think we",
        "maybe we",
        "let's",
        "what about",
        "how about",
    ];
    LEAD_INS.iter().any(|p| m.contains(p))
}

/// True when the message carries an explicit imperative to mint or run work, so
/// it overrides an [`is_ideation`] lead-in (section 10.5). "I was thinking, create a
/// task to X" is a command; "I was thinking we could build X" is a conversation.
/// Matched against the already-lowercased message.
fn is_explicit_command(m: &str) -> bool {
    const COMMANDS: &[&str] = &[
        "create a task",
        "make a task",
        "add a task",
        "new task",
        "start it",
        "run it",
        "start the run",
        "kick off",
        "orchestrate",
        "assign ",
        "make a plan",
        "draft a plan",
        "give me a plan",
        "come up with a plan",
        "put together a plan",
        "plan this out",
        "plan it out",
        "outline the steps",
        "lay out the steps",
    ];
    COMMANDS.iter().any(|p| m.contains(p))
}

/// True when the message reads as a QUESTION - the user asking or deliberating,
/// not commanding (section 17.1: "Prime must understand conversational intent").
///
/// Matched against the already-lowercased message. A question is signalled by an
/// interrogative opener (a wh-word, or a yes/no auxiliary like "should"/"is"/"do")
/// OR a trailing "?". Polite directives ("can you ...", "could you ...", "would
/// you ...") are deliberately NOT openers: those carry a command and are handled
/// by [`is_explicit_command`] / the work-verb catch, so "can you fix the bug"
/// still acts. The caller gates this with `!is_explicit_command`, so a question
/// that also names an explicit command still acts.
fn is_question(m: &str) -> bool {
    let m = m.trim();
    let first: String = m
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let opener = matches!(
        first.as_str(),
        "how" | "what" | "whats" | "why" | "when" | "where" | "which" | "who" | "whose" | "whom"
            | "should" | "shall" | "is" | "are" | "am" | "do" | "does" | "did" | "was" | "were"
            | "has" | "have" | "had"
    );
    opener || m.ends_with('?')
}

/// True when a message is guarded as conversation — ideation/musing
/// ([`is_ideation`]) or a question ([`is_question`]) with no explicit command
/// ([`is_explicit_command`]) — so it must NOT be minted into work.
///
/// This is the same guard `classify_intent` applies inline (sections 10.5, 17.1);
/// it is exposed so the brain-mediated reconciliation gate
/// ([`crate::prime_intent::reconcile_intent`]) can enforce the SAME rail — a brain
/// may never promote a guarded turn to a work intent, no matter how confident it
/// is. Operates on the raw message; the lowercasing matches `classify_intent`.
pub fn is_chat_guarded(message: &str) -> bool {
    let m = message.trim().to_lowercase();
    // Ideation, a question, venting, or casual chitchat are all conversation: a
    // brain may never promote any of them to a work intent. Emotional / small-talk
    // turns are included so an insult or a vent ("fuck you", "ugh") can never be
    // reconciled up to task creation or a run, exactly like musing and questions
    // (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §17.1).
    (is_ideation(&m)
        || is_question(&m)
        || is_emotional_distress(&m)
        || is_casual_chat(&m))
        && !is_explicit_command(&m)
}

/// True when `word` appears in `haystack` as a WHOLE WORD - delimited by a
/// non-alphanumeric boundary (or string edge) on both sides - rather than as a
/// substring. Lets the task-creation catch fire on "please fix the bug" while NOT
/// firing on "the prefix is wrong" / "show me a preview" / "the building plan".
/// Both inputs are expected to be lowercase ASCII (the classifier's normalized
/// message and the literal verbs in [`CREATION_VERBS`]).
fn has_word(haystack: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(word) {
        let start = from + rel;
        let end = start + word.len();
        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Turn a request into a task title by stripping a few polite/imperative
/// lead-ins, or `None` when nothing actionable remains.
///
/// Prefixes are ASCII, so byte offsets line up with the original. Returns `None`
/// when the request carries no work to title: either it strips down to nothing,
/// or what is left is a single bare creation verb ("create", "fix") with no
/// object. Prime turns that `None` into a clarifying question instead of minting
/// a task titled after the verb (section 10.5).
fn task_title(message: &str) -> Option<String> {
    let trimmed = message.trim();
    let lower = trimmed.to_lowercase();
    const PREFIXES: &[&str] = &[
        "prime, ",
        "prime ",
        "please ",
        "can you ",
        "could you ",
        "create a task to ",
        "create a task ",
        "make a task to ",
        "add a task to ",
        "new task to ",
        "i need you to ",
        "i need to ",
    ];
    let mut start = 0usize;
    loop {
        let cur = &lower[start..];
        let mut matched = false;
        for p in PREFIXES {
            if cur.starts_with(p) {
                start += p.len();
                matched = true;
                break;
            }
        }
        if !matched {
            break;
        }
    }
    let title: String = trimmed[start..].trim().chars().take(120).collect();
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    // A lone creation verb ("create", "fix") names no work; ask, don't invent.
    let mut words = title.split_whitespace();
    if let (Some(first), None) = (words.next(), words.next()) {
        if CREATION_VERBS.contains(&first.to_lowercase().as_str()) {
            return None;
        }
    }
    Some(title.to_string())
}

/// Build Prime's brainstorming reply (section 10.5: "ask clarifying questions
/// when needed").
///
/// The fixed open-ended prompt was the same regardless of what the user said —
/// it never reflected the idea or asked anything specific. When the message
/// names a topic (the same noun/verb phrase [`brainstorm_task_candidate`]
/// recovers for the one-click suggestion), this reflects that topic back and
/// asks ONE concrete clarifying question, so a vague idea gets a useful, grounded
/// follow-up. It stays a CONVERSATION: nothing is created or run, and the kernel
/// still attaches the "turn this into a task" suggestion (section 11.1). The
/// topic is the cleaned candidate (lead-ins stripped), quoted as a reflection —
/// not a verbatim echo of the raw message. Falls back to the open-ended prompt
/// when the message carries no nameable topic (pure connective musing).
fn brainstorm_reply(message: &str) -> String {
    match brainstorm_task_candidate(message) {
        Some(topic) => format!(
            "Let's shape the idea: \"{topic}\". Two things help me think it through with you: what outcome would make this a win, and is there a constraint I should design around — time, scope, or an approach to avoid? Nothing gets created or run while we talk; when it's worth pursuing, I can turn it into a task in one step."
        ),
        None => "Good - let's think it through. Tell me the goal you're after and any constraints, and I'll lay out a few approaches with their trade-offs. Nothing gets created or run while we're talking; when an idea is worth pursuing, I can turn it into a task in one step."
            .to_string(),
    }
}

/// Build the clarify for an `Orchestration` request whose goal does not actually
/// split across agents (section 10.4, section 10.5).
///
/// Same reflect-and-clarify shape as [`brainstorm_reply`]: instead of a fixed
/// nudge, it echoes the parsed `goal` back so the user sees what Prime understood
/// of a single-step request, then asks for the distinct steps. The goal is the
/// already-stripped phrase from [`orchestration_goal`] (lead-ins removed, original
/// case preserved). Falls back to the generic prompt when the recovered goal is
/// not a nameable phrase — a lone word, or the whole message when nothing stripped
/// (so a bare "orchestrate this" is never quoted back as if it named work).
fn orchestration_clarify(goal: &str) -> String {
    let g = goal.trim();
    let words = g.split_whitespace().count();
    if (2..=18).contains(&words) {
        format!(
            "\"{g}\" reads like a single piece of work, not something to split across agents. What are the distinct steps — e.g. \"research the options, implement a prototype, and write the docs\"? Or say \"create a task to …\" and I'll make one brief."
        )
    } else {
        "That reads like a single piece of work, not something to split across agents. Give me the distinct steps - e.g. \"research the options, implement a prototype, and write the docs\" - or say \"create a task to ...\" and I'll make one brief."
            .to_string()
    }
}

/// Build the clarify for an under-specified `TaskUpdate` request (section 10.5).
///
/// Reached only when the deterministic rail could not resolve a concrete update (a
/// missing task id and/or no recognizable field) — a real `UpdateTask` action IS now
/// wired ([`crate::prime_update_slots`]), so this is the ask-one-question fallback, a
/// *resolvable* clarify the multi-turn memory + brain can continue. It reflects
/// whatever the message already named — the target task id ([`extract_task_id`])
/// and/or the field being changed ([`update_change_phrase`]) — and asks only for the
/// piece that is still missing. Same reflect-and-clarify shape as [`brainstorm_reply`].
fn task_update_clarify(message: &str) -> String {
    let task_id = extract_task_id(message);
    let field = update_change_phrase(message);
    match (task_id, field) {
        (Some(id), Some(field)) => {
            format!("Got it — change the {field} on {id}. What should the new {field} be?")
        }
        (Some(id), None) => {
            format!("I can update {id}. What should change — its priority, title, assignee, or status?")
        }
        (None, Some(field)) => {
            format!("Which task's {field} should I change? Give me its id (task_…) or title.")
        }
        (None, None) => {
            "Which task should I update, and what should change — priority, title, assignee, or status?"
                .to_string()
        }
    }
}

/// Name the field a `TaskUpdate` message wants to change, when it says so, so the
/// clarify reflects the user's intent instead of asking generically. Conservative
/// and ordered: priority wins over the others, then title/rename, then assignee
/// (covers "reassign"), then status (covers "mark … done/blocked"). `None` when
/// the message names no recognizable field.
fn update_change_phrase(message: &str) -> Option<&'static str> {
    let m = message.to_lowercase();
    if m.contains("priority") {
        Some("priority")
    } else if m.contains("rename") || m.contains("title") || m.contains(" name") {
        Some("title")
    } else if m.contains("reassign") || m.contains("assignee") || m.contains("assign") {
        Some("assignee")
    } else if m.contains("mark ")
        || m.contains("status")
        || m.contains("done")
        || m.contains("blocked")
    {
        Some("status")
    } else {
        None
    }
}

/// Recover the candidate work a brainstorm message gestures at, for the
/// "turn this into a task" suggestion (section 11.1).
///
/// Best effort and conservative: it strips ideation lead-ins and the connective
/// fillers that usually follow them ("I was thinking we could ...") from the
/// front, leaving the noun phrase that names the work. The result only ever
/// pre-fills the chat input for the user to confirm or edit (the suggestion is
/// `send: false`), so an imperfect strip is harmless - the user still names the
/// task. Returns `None` when nothing nameable remains.
pub fn brainstorm_task_candidate(message: &str) -> Option<String> {
    let trimmed = message.trim();
    let lower = trimmed.to_lowercase();
    // Stripped from the front, longest first so "i was thinking" wins over
    // "thinking". Each is matched only as a leading token run (trailing space),
    // so "we" never bites into "weather". Punctuation/commas between strips are
    // trimmed each pass.
    const STRIPS: &[&str] = &[
        "i have been thinking that",
        "i've been thinking that",
        "ive been thinking that",
        "i was thinking that",
        "i was thinking maybe",
        "i was thinking we should",
        "i was thinking we could",
        "i was thinking we might",
        "i was thinking",
        "i'm thinking",
        "im thinking",
        "i am thinking",
        "i've been thinking",
        "ive been thinking",
        "i have been thinking",
        "i was wondering whether",
        "i was wondering if",
        "i was wondering",
        "i wonder whether",
        "i wonder if",
        "i'd like to discuss",
        "i would like to discuss",
        "i want to discuss",
        "i want to talk about",
        "i want to brainstorm",
        "i would like to",
        "i'd like to",
        "i want to",
        "can we talk about",
        "could we talk about",
        "let's talk about",
        "lets talk about",
        "let's discuss",
        "lets discuss",
        "let's",
        "lets",
        "talk about",
        "thinking about",
        "thinking of",
        "playing with the idea of",
        "playing with the idea",
        "kicking around",
        "toying with",
        "what if we",
        "what if i",
        "what if you",
        "what if",
        // Question / suggestion lead-ins, so a deliberative question routed here by
        // the conversation guard ("what's the best way to fix the tests?") yields a
        // clean candidate ("fix the tests"). Longest-first so the specific phrasing
        // wins before the bare opener. Best-effort: it only pre-fills the input.
        "what's the best way to",
        "what is the best way to",
        "whats the best way to",
        "what's the right way to",
        "what is the right way to",
        "how should we",
        "how should i",
        "how do we",
        "how do i",
        "how can we",
        "how can i",
        "how would we",
        "how would i",
        "how do you",
        "is it worth",
        "is there a way to",
        "should i",
        "what about",
        "how about",
        "i think we should",
        "i think we could",
        "i think",
        "maybe we should",
        "maybe we could",
        "we should",
        "we could",
        "we might",
        "should we",
        "could we",
        "can we",
        "the idea of",
        "an idea to",
        "an idea for",
        "the idea",
        "maybe",
        "perhaps",
        "just",
        "we",
        "to",
        "about",
    ];
    let mut start = 0usize;
    loop {
        // Trim leading whitespace and connective punctuation between strips.
        while let Some(c) = lower[start..].chars().next() {
            if c.is_whitespace() || c == ',' || c == ':' || c == '-' {
                start += c.len_utf8();
            } else {
                break;
            }
        }
        let cur = &lower[start..];
        let mut matched = false;
        for p in STRIPS {
            // Match the strip as a leading token: it must be followed by a
            // space (or end), so a standalone "we"/"to" never eats a word.
            if let Some(after) = cur.strip_prefix(p) {
                if after.is_empty() || after.starts_with(|c: char| c.is_whitespace() || c == ',') {
                    start += p.len();
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            break;
        }
    }
    let candidate: String = trimmed[start..].trim().chars().take(120).collect();
    let candidate = candidate.trim_end_matches(['.', '?', '!']).trim();
    if candidate.is_empty() {
        return None;
    }
    Some(candidate.to_string())
}

/// The grounded, general-agent greeting Prime opens a casual hello with.
///
/// Hermes-first: it reads like a normal assistant, NOT a work-board manager. It
/// names no board/queue/crew state and asks nothing about "what to set up" — a plain
/// hello stays a plain hello (`docs/prime-processing-audit.md` "Hermes-first general
/// agent"; §10.5, §17.1). Control-plane state is surfaced only when the user
/// explicitly asks a status question, never volunteered into small talk.
fn greeting_text() -> &'static str {
    "Hey - Prime here. What's on your mind? Happy to just talk, think something through, or answer a question - and when you actually want work done, I can drive your local Relux control plane."
}

/// The light, conversational reply Prime gives to throwaway chitchat
/// ([`PrimeIntent::SmallTalk`]). Hermes-first: it stays in the moment and does NOT
/// pivot to the board or "what do you want to set up" — chitchat is chitchat
/// (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §17.1).
fn small_talk_text() -> &'static str {
    "Anytime. I'm around if you want to talk something through or need a hand with anything."
}

/// The brief, human acknowledgement Prime gives to venting / frustration / an insult
/// ([`PrimeIntent::EmotionalSupport`]). Hermes-first: it meets the user where they
/// are and never turns the moment into a work prompt; the kernel may attach contextual
/// NON-action chips ("Tell me what broke" / "Show me the last run") but no task/plan/run
/// CTA (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §17.1).
fn emotional_support_text() -> &'static str {
    "That sounds frustrating - I hear you. If something broke or a run went sideways, tell me what happened and I'll dig in; otherwise I'm happy to just talk it through."
}

/// Verbs that mark a brainstorm candidate as REAL, nameable work (a superset of
/// [`CREATION_VERBS`] with the common improvement/delivery verbs). Used ONLY by
/// [`brainstorm_offers_actionable_work`] to decide whether an idea is concrete
/// enough to offer a one-click work button for — never to classify or to act.
const WORK_INDICATORS: &[&str] = &[
    "create",
    "make",
    "add",
    "build",
    "fix",
    "implement",
    "investigate",
    "summarize",
    "review",
    "refactor",
    "write",
    "draft",
    "improve",
    "ship",
    "design",
    "automate",
    "optimize",
    "test",
    "deploy",
    "document",
    "research",
    "migrate",
    "rewrite",
    "redesign",
    "redo",
    "revamp",
    "overhaul",
    "set up",
    "clean up",
    "integrate",
    "rebuild",
    "update",
    "audit",
    "plan",
];

/// Split a normalized message into its whole alphanumeric tokens. Shared by the
/// emotional / small-talk detectors so "ugh"/"lol"/"damn" match as WHOLE words and
/// never as a substring ("damn" never bites "damnation").
fn word_tokens(m: &str) -> Vec<&str> {
    m.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// True when a message reads as venting, an insult aimed at Prime, frustration, or
/// an emotional message with no work in it — "ugh this is so frustrating", "fuck
/// you", "you're useless", "I give up", "I'm exhausted". This is the
/// [`PrimeIntent::EmotionalSupport`] detector and the CTA-suppression rail: such a
/// turn gets a normal, human acknowledgement and, crucially, NO task/plan/run CTA
/// (Hermes-first: emotional chat is chat, never a work prompt;
/// `docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §17.1).
///
/// Conservative: a genuine work command that merely contains a charged word ("fix
/// the damn login bug") is NOT flagged — the bare-word arm is bounded to short
/// messages and matches whole tokens, and a real command anyway classifies as work
/// before this is consulted. Matched against the lowercased message.
pub fn is_emotional_distress(message: &str) -> bool {
    let m = message.trim().to_lowercase();
    if m.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "fuck you",
        "fuck off",
        "screw you",
        "shut up",
        "stfu",
        "piss off",
        "you suck",
        "you're useless",
        "youre useless",
        "you are useless",
        "you're stupid",
        "youre stupid",
        "you're dumb",
        "youre dumb",
        "i hate you",
        "this is useless",
        "this is stupid",
        "this is pointless",
        "i give up",
        "i'm done",
        "im done",
        "i am done",
        "this sucks",
        "i'm frustrated",
        "im frustrated",
        "so frustrating",
        "this is frustrating",
        "i'm annoyed",
        "im annoyed",
        "i'm pissed",
        "im pissed",
        "i'm so tired",
        "im so tired",
        "i'm exhausted",
        "im exhausted",
        "i'm overwhelmed",
        "im overwhelmed",
    ];
    if PHRASES.iter().any(|p| m.contains(p)) {
        return true;
    }
    // A short message that is essentially just a charged/expletive word ("ugh",
    // "wtf", a bare expletive). Bounded to short messages so a longer sentence that
    // merely contains the word is unaffected; whole-token match.
    const WORDS: &[&str] = &[
        "fuck", "shit", "bitch", "asshole", "bastard", "idiot", "moron", "dumbass", "wtf", "ugh",
        "ughhh", "ughh", "damn", "crap", "argh", "grr", "fml",
    ];
    let tokens = word_tokens(&m);
    tokens.len() <= 4 && tokens.iter().any(|w| WORDS.contains(w))
}

/// True when a message reads as throwaway, neutral/positive casual chitchat with no
/// work and no negative affect — "lol", "haha", "nice", "cool", "thanks", "ok cool",
/// "makes sense". This is the [`PrimeIntent::SmallTalk`] detector: Prime answers
/// lightly and, crucially, attaches NO task/plan/run CTA (Hermes-first: chitchat is
/// chitchat; `docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5).
///
/// Conservative and consulted only AFTER every action/question/greeting rail in
/// [`classify_intent`] has been tried, so by the time it runs nothing actionable
/// remains. The bare-word arm is bounded to short messages and matches whole tokens,
/// so a real sentence that merely contains "nice"/"cool"/"ok" is unaffected.
pub fn is_casual_chat(message: &str) -> bool {
    let m = message.trim().to_lowercase();
    if m.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "sounds good",
        "sounds great",
        "makes sense",
        "got it",
        "fair enough",
        "good point",
        "nice one",
        "thank you",
        "much appreciated",
        "no worries",
        "no problem",
        "good to know",
        "cool cool",
        "ok cool",
        "oh ok",
        "haha",
        "lmao",
    ];
    if PHRASES.iter().any(|p| m.contains(p)) {
        return true;
    }
    // A short message that is essentially just a throwaway/affirmation token.
    const WORDS: &[&str] = &[
        "lol", "lmao", "lmfao", "haha", "hehe", "meh", "bruh", "nice", "cool", "ok", "okay", "k",
        "kk", "thanks", "thx", "ty", "yw", "np", "yay", "woohoo", "whatever", "nevermind", "nvm",
        "sure", "yep", "yup", "nope", "huh", "hmm", "wow", "dope", "sweet", "awesome", "fine",
        "gotcha", "word", "neat",
    ];
    let tokens = word_tokens(&m);
    tokens.len() <= 4 && tokens.iter().any(|w| WORDS.contains(w))
}

/// True when a message reads as venting, an insult, profanity-as-affect, or pure
/// throwaway small talk with no work in it. The union of [`is_emotional_distress`]
/// and [`is_casual_chat`]; used ONLY to SUPPRESS work CTAs / steer wording on the
/// brainstorm path — it never promotes anything to an action, so a false positive
/// only means a friendlier, button-free reply. A genuine work command that merely
/// contains a charged word ("fix the damn login bug") is unaffected: it classifies
/// as `TaskCreation`, not `Brainstorming`, so this gate (only consulted on the
/// brainstorm CTA path) never sees it. Matched against the lowercased message.
pub fn is_frustration_or_emotional(message: &str) -> bool {
    is_emotional_distress(message) || is_casual_chat(message)
}

/// True when a `Brainstorming` turn gestured at REAL, nameable work worth offering a
/// one-click "Turn this into a task" / "Plan this out" button for — and false for
/// venting, insults, or empty small talk, where those buttons are absurd.
///
/// This is the suggestion-suppression policy the chat surface keys its work CTAs off
/// ([`crate::KernelState`]'s `attach_suggestions`): Hermes-first, a casual or
/// emotional turn stays a plain conversation with NO work prompt; a button appears
/// only when the message carries an actual work verb on something nameable
/// (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §11.1,
/// §17.1). It gates PRESENTATION only — it can never create or run anything.
pub fn brainstorm_offers_actionable_work(message: &str) -> bool {
    if is_frustration_or_emotional(message) {
        return false;
    }
    match brainstorm_task_candidate(message) {
        Some(candidate) => {
            let c = candidate.to_lowercase();
            WORK_INDICATORS.iter().any(|v| has_word(&c, v))
        }
        None => false,
    }
}

/// Map a tool-invocation message onto `(plugin_id, tool_name, input_json)`, or
/// `None` when nothing nameable was requested.
///
/// Deterministic and conservative: it only emits a `(plugin, tool)` it can name
/// with confidence - an explicit `plugin/tool` reference, the two built-in tools
/// (echo/status), or a known ToolSet keyword (github/terminal/...). For a keyword
/// match the `tool_name` is left empty; the kernel resolves the plugin's first
/// installed tool and reports it honestly (it will not be runnable here, which is
/// the truthful answer). The input is the first JSON object found in the message,
/// or - for an `echo <text>` request with no JSON - `{ "message": "<text>" }`.
pub(crate) fn parse_tool_request(message: &str) -> Option<(String, String, String)> {
    let trimmed = message.trim();
    let lower = trimmed.to_lowercase();
    let json = extract_json(trimmed);

    // 1. Explicit "<plugin>/<tool>" reference, e.g. "relux-tools-github/github.create_pr".
    if let Some(tok) = lower
        .split(|c: char| c.is_whitespace())
        .find(|t| t.contains('/') && t.starts_with("relux-"))
    {
        let mut parts = tok.splitn(2, '/');
        if let (Some(plugin), Some(tool)) = (parts.next(), parts.next()) {
            let plugin = trim_token(plugin);
            let tool = trim_token(tool);
            if !plugin.is_empty() && !tool.is_empty() {
                return Some((plugin, tool, json.unwrap_or_else(|| "{}".to_string())));
            }
        }
    }

    // 1b. Explicit MCP tool reference: "mcp:<server>/<tool>" — the stable ref form a
    //     discovered MCP tool is listed under (the `mcp:<server>` synthetic plugin id
    //     from `relux_core::mcp_synthetic_plugin_id` + the tool name). Scanned over the
    //     ORIGINAL (case-preserving) message because an MCP tool name may be camelCase,
    //     unlike the lowercase relux-tools convention, and the server id + tool name
    //     must match the live `tools/list` exactly. Purely syntactic: the plan-grounding
    //     path (`KernelState::build_tool_plan_proposal`) resolves the ref against the
    //     off-lock MCP catalog and fails CLOSED (`unavailable` / `unknown`) when the
    //     server or tool is not live, never silently accepting it (`docs/mcp.md`
    //     "Run-driven multi-tool plan"; §10.5, §17.1).
    if let Some(tok) = trimmed
        .split(|c: char| c.is_whitespace())
        .map(trim_token)
        .find(|t| t.starts_with("mcp:") && t.contains('/'))
    {
        let mut parts = tok.splitn(2, '/');
        if let (Some(plugin), Some(tool)) = (parts.next(), parts.next()) {
            let plugin = plugin.trim();
            let tool = tool.trim();
            // Require a non-empty server id after the `mcp:` prefix and a non-empty tool.
            if plugin.len() > "mcp:".len() && !tool.is_empty() {
                return Some((
                    plugin.to_string(),
                    tool.to_string(),
                    json.unwrap_or_else(|| "{}".to_string()),
                ));
            }
        }
    }

    // 2. The two built-in tools, by name or by plain-language reference.
    if lower.contains("echo.say") || lower.contains("echo") {
        let input = json.unwrap_or_else(|| echo_input_from(trimmed));
        return Some((
            "relux-tools-echo".to_string(),
            "echo.say".to_string(),
            input,
        ));
    }
    if lower.contains("status.summary") || lower.contains("status tool") || lower.contains("status")
    {
        return Some((
            "relux-tools-status".to_string(),
            "status.summary".to_string(),
            json.unwrap_or_else(|| "{}".to_string()),
        ));
    }

    // 3. A known ToolSet keyword -> name the plugin; the kernel resolves the tool
    //    and reports it (these are installed-but-not-runnable here).
    const KEYWORD_PLUGINS: &[(&str, &str)] = &[
        ("github", "relux-tools-github"),
        ("terminal", "relux-tools-terminal"),
        ("shell", "relux-tools-terminal"),
        ("browser", "relux-tools-browser"),
        ("slack", "relux-tools-slack"),
        ("discord", "relux-tools-discord"),
        ("tavily", "relux-tools-tavily"),
        ("salesforce", "relux-tools-salesforce"),
        ("zendesk", "relux-tools-zendesk"),
    ];
    for (kw, plugin) in KEYWORD_PLUGINS {
        if lower.contains(kw) {
            return Some((
                (*plugin).to_string(),
                String::new(),
                json.unwrap_or_else(|| "{}".to_string()),
            ));
        }
    }

    None
}

/// Split an explicit multi-tool request into ordered raw segments, one per intended
/// tool step, so each can be grounded against the live tool registry independently
/// (`docs/mcp.md` "Run-driven multi-tool plan"). Strips a leading plan lead-in
/// ("run these tools in order:", "chain these tools:", …) and splits the remainder
/// on ordinary sequence connectors ("then", "and then", "followed by", "after that",
/// "next,", ";", a newline) plus simple numbered / bulleted markers ("1.", "2)",
/// "-"). Purely syntactic — it resolves nothing; the caller resolves each segment
/// with [`parse_tool_request`] and validates the whole plan with the existing
/// [`relux_core::task::TaskToolPlan`] gate. Matching is ASCII-case-insensitive over a
/// 1:1 char buffer, so it never panics on a non-ASCII message.
pub(crate) fn split_tool_plan_segments(message: &str) -> Vec<String> {
    let body = strip_tool_plan_lead_in(message);
    let chars: Vec<char> = body.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    // Longest connectors first so a compound form wins over its shorter prefix.
    const CONNECTORS: &[&str] = &[
        " and then ",
        ", then ",
        ", followed by ",
        ", after that ",
        " followed by ",
        " after that ",
        " then ",
        " next, ",
        ";",
        "\n",
    ];
    let mut segments: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0usize;
    'scan: while i < chars.len() {
        for conn in CONNECTORS {
            let cc: Vec<char> = conn.chars().collect();
            if i + cc.len() <= lower.len() && lower[i..i + cc.len()] == cc[..] {
                segments.push(cur.trim().to_string());
                cur = String::new();
                i += cc.len();
                continue 'scan;
            }
        }
        cur.push(chars[i]);
        i += 1;
    }
    segments.push(cur.trim().to_string());
    segments
        .into_iter()
        .map(|seg| {
            seg.trim()
                .trim_start_matches(|c: char| {
                    c.is_ascii_digit() || c == '.' || c == ')' || c == '-' || c == '*' || c == ' '
                })
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Strip a leading "run these tools in order:" / "chain these tools:" style plan
/// lead-in (and any polite prefix) so what remains is the ordered tool steps. Only a
/// known lead-in is removed, never the substance; applied repeatedly so a compound
/// prefix ("prime, please run these tools:") is fully stripped.
fn strip_tool_plan_lead_in(message: &str) -> String {
    // Longest-first within a pass so a specific lead-in wins over a shorter prefix.
    const LEAD_INS: &[&str] = &[
        "run the following tools in order:",
        "run these tools in order:",
        "run the following tools in order",
        "run these tools in order",
        "run the following tools:",
        "use the following tools:",
        "run these tools:",
        "use these tools:",
        "chain these tools:",
        "chain the tools:",
        "run a tool plan:",
        "here is the tool plan:",
        "here's the tool plan:",
        "run these tools",
        "use these tools",
        "chain these tools",
        "chain the tools",
        "run the tools:",
        "tool plan:",
        "in this order:",
        "in order:",
        "i want you to ",
        "i'd like you to ",
        "go ahead and ",
        "can you ",
        "could you ",
        "would you ",
        "please ",
        "prime, ",
        "prime ",
    ];
    let mut rest = message.trim().to_string();
    loop {
        let lower = rest.to_lowercase();
        let mut stripped = false;
        for lead in LEAD_INS {
            if lower.starts_with(lead) {
                // Lead-ins are ASCII, so the matched prefix is the same byte length
                // in the original-cased string — a safe char boundary.
                rest = rest[lead.len()..].trim_start().to_string();
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    rest
}

/// Trim a token down to a plugin/tool-id shape (ASCII alnum, `-`, `_`, `.`).
fn trim_token(tok: &str) -> String {
    tok.trim_matches(|c: char| {
        !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.'
    })
    .to_string()
}

/// Pull the first balanced-looking JSON object (`{ ... }`) out of a message and
/// return it only if it parses. Naive first-`{`..last-`}` slice - enough to lift
/// an inline `{"message":"hi"}` the user typed.
fn extract_json(message: &str) -> Option<String> {
    let start = message.find('{')?;
    let end = message.rfind('}')?;
    if end <= start {
        return None;
    }
    let candidate = &message[start..=end];
    serde_json::from_str::<serde_json::Value>(candidate)
        .ok()
        .map(|_| candidate.to_string())
}

/// Build the echo input for an `echo <text>` request that carried no JSON: the
/// text after a leading "echo" becomes `{ "message": "<text>" }`; a bare "echo"
/// echoes an empty object.
fn echo_input_from(message: &str) -> String {
    let lower = message.to_lowercase();
    let rest = match lower.find("echo") {
        Some(idx) => message[idx + "echo".len()..].trim(),
        None => message.trim(),
    };
    // Drop a polite/imperative lead-in like "echo say " / "echo the message ".
    let rest = rest
        .trim_start_matches("say ")
        .trim_start_matches("the message ")
        .trim_start_matches(": ")
        .trim();
    if rest.is_empty() {
        "{}".to_string()
    } else {
        serde_json::json!({ "message": rest }).to_string()
    }
}

/// Strip a leading orchestration directive ("orchestrate", "coordinate the work to",
/// "split this across the agents to", ...) so what remains is the goal Prime should
/// decompose. Conservative: it only removes a known lead-in, never the substance.
fn orchestration_goal(message: &str) -> String {
    let trimmed = message.trim();
    let lower = trimmed.to_lowercase();
    // Ordered longest-first so a compound lead-in wins over its shorter prefix.
    const LEAD_INS: &[&str] = &[
        "prime, ",
        "prime ",
        "please ",
        "can you ",
        "could you ",
        "i need you to ",
        "set up a team to ",
        "spin up a team to ",
        "have the team ",
        "have agents ",
        "split this across the agents to ",
        "split this across agents to ",
        "split this across agents and ",
        "split this across agents ",
        "split this across ",
        "split it across agents to ",
        "split it across ",
        "divide this across agents to ",
        "divide this into ",
        "divide this ",
        "divide it across ",
        "fan this out to ",
        "fan out to ",
        "fan out ",
        "parallelize ",
        "delegate across agents to ",
        "delegate across the agents to ",
        "delegate across ",
        "coordinate the work to ",
        "coordinate the agents to ",
        "coordinate ",
        "orchestrate the agents to ",
        "orchestrate this across agents to ",
        "orchestrate this to ",
        "orchestrate this: ",
        "orchestrate this ",
        "orchestrate: ",
        "orchestrate ",
        "plan and assign ",
        "assign across the agents to ",
        "assign across agents to ",
        "assign across ",
    ];
    let mut start = 0usize;
    loop {
        let cur = &lower[start..];
        let mut matched = false;
        for lead in LEAD_INS {
            if cur.starts_with(lead) {
                start += lead.len();
                matched = true;
                break;
            }
        }
        if !matched {
            break;
        }
    }
    let goal = trimmed[start..]
        .trim()
        .trim_start_matches(':')
        .trim()
        .trim_end_matches('.')
        .trim();
    if goal.is_empty() {
        trimmed.to_string()
    } else {
        goal.to_string()
    }
}

/// Recover the goal phrase a plan request names by stripping the plan lead-ins
/// (longest-first so a compound lead-in wins over its shorter prefix). The result
/// is the clean phrase the preview is built from and the same phrase the "Create
/// these tasks" suggestion re-wraps as `orchestrate <goal>`, so the previewed plan
/// and the committed plan are decomposed from identical input. Mirrors
/// [`orchestration_goal`]'s strip-and-trim shape. `pub(crate)` so the kernel's
/// suggestion builder recovers the same goal the decide() arm previewed.
pub(crate) fn plan_goal(message: &str) -> String {
    let trimmed = message.trim();
    let lower = trimmed.to_lowercase();
    const LEAD_INS: &[&str] = &[
        "prime, ",
        "prime ",
        "please ",
        "can you ",
        "could you ",
        "i need you to ",
        "give me a plan to ",
        "give me a plan for ",
        "give me a plan ",
        "come up with a plan to ",
        "come up with a plan for ",
        "come up with a plan ",
        "put together a plan to ",
        "put together a plan for ",
        "put together a plan ",
        "draft a plan to ",
        "draft a plan for ",
        "draft a plan ",
        "make a plan to ",
        "make a plan for ",
        "make a plan ",
        "lay out the steps to ",
        "lay out the steps for ",
        "lay out the steps ",
        "outline the steps to ",
        "outline the steps for ",
        "outline the steps ",
        "outline a plan to ",
        "outline a plan for ",
        "outline a plan ",
        "plan this out to ",
        "plan this out ",
        "plan this out",
        "plan it out to ",
        "plan it out ",
        "plan it out",
        "plan out how to ",
        "plan out ",
        "plan how to ",
        "plan how we ",
        "plan for ",
        "plan to ",
        "plan the ",
        "plan ",
    ];
    let mut start = 0usize;
    loop {
        let cur = &lower[start..];
        let mut matched = false;
        for lead in LEAD_INS {
            if cur.starts_with(lead) {
                start += lead.len();
                matched = true;
                break;
            }
        }
        if !matched {
            break;
        }
    }
    let goal = trimmed[start..]
        .trim()
        .trim_start_matches(':')
        .trim()
        .trim_end_matches('.')
        .trim();
    if goal.is_empty() {
        trimmed.to_string()
    } else {
        goal.to_string()
    }
}

/// Render a multi-step plan as a reviewable preview that COMMITS NOTHING
/// (section 10 planning layer, section 11.1). The explicit "Create these tasks"
/// suggestion is the only path that materializes the briefs, so the preview is
/// purely informational - it lists the proposed steps and the agents they would
/// land on, grounded in the planner's actual decomposition.
fn plan_preview_text(goal: &str, plan: &OrchestrationPlan) -> String {
    let steps: Vec<String> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {} ({})", i + 1, s.title, s.role.label()))
        .collect();
    format!(
        "Here is a plan for \"{goal}\" - {} across {}:\n{}\nNothing is created yet; this is just the shape. Review it, and when it is right I will create these as tasks in one step.",
        count_phrase(plan.steps.len(), "step"),
        count_phrase(plan.agent_labels().len(), "agent"),
        steps.join("\n"),
    )
}

/// Reply for a plan request whose goal does not genuinely split into multiple
/// briefs: steer the user to the one-task path instead of fanning out a single
/// piece of work into a storm (section 10.5). Still action-free - nothing is
/// created until the user confirms the one-click suggestion.
fn plan_single_text(goal: &str) -> String {
    format!(
        "\"{goal}\" reads like a single piece of work, not a multi-step plan. I can turn it straight into one task - nothing is created until you confirm."
    )
}

fn derive_agent_name(message: &str) -> String {
    let m = message.to_lowercase();
    for marker in [" named ", " called ", " as "] {
        if let Some(idx) = m.find(marker) {
            let raw = m[idx + marker.len()..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            if !raw.is_empty() {
                return raw.replace('_', "-");
            }
        }
    }
    if m.contains("browser") {
        "browser-agent".to_string()
    } else if m.contains("research") {
        "research-agent".to_string()
    } else if m.contains("coding") || m.contains("code") {
        "code-agent".to_string()
    } else if m.contains("support") {
        "support-agent".to_string()
    } else {
        "new-agent".to_string()
    }
}

fn derive_plugin_id(message: &str) -> String {
    message
        .split_whitespace()
        .find(|w| w.starts_with("relux-"))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .to_string()
        })
        .unwrap_or_else(|| "(unspecified-plugin)".to_string())
}

fn derive_permission_label(message: &str) -> String {
    let m = message.to_lowercase();
    if m.contains("github") {
        "tool:relux-tools-github:access".to_string()
    } else if m.contains("terminal") || m.contains("shell") {
        "tool:relux-tools-terminal:access".to_string()
    } else {
        "(permission to be specified)".to_string()
    }
}

pub(crate) fn extract_task_id(message: &str) -> Option<String> {
    let m = message.to_lowercase();
    if let Some(start_idx) = m.find("task_") {
        let remainder = &m[start_idx + "task_".len()..];
        let id: String = remainder
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !id.is_empty() {
            return Some(format!("task_{}", id));
        }
    }
    None
}

/// Extract an explicit orchestration id (`orch_…`) referenced in the message, if any.
/// Mirrors [`extract_task_id`] for the `orch_` prefix: it finds the first `orch_` token
/// and reads the trailing id characters. Used to honor a named orchestration on a
/// run/continue request and to continue a "which orchestration?" clarification with a
/// bare id. Existence is validated by the kernel against the live records — this only
/// recovers the reference text.
pub(crate) fn extract_orchestration_id(message: &str) -> Option<String> {
    let m = message.to_lowercase();
    if let Some(start_idx) = m.find("orch_") {
        let remainder = &m[start_idx + "orch_".len()..];
        let id: String = remainder
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !id.is_empty() {
            return Some(format!("orch_{}", id));
        }
    }
    None
}

fn extract_agent_id_from_assignment(message: &str) -> Option<String> {
    let m = message.to_lowercase();
    if let Some(to_idx) = m.find(" to ") {
        let remainder = &m[to_idx + " to ".len()..];
        let agent_name: String = remainder
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if !agent_name.is_empty() {
            return Some(agent_name);
        }
        return None;
    }
    if let Some(assign_idx) = m.find("assign ") {
        let remainder = &m[assign_idx + "assign ".len()..];
        let words: Vec<&str> = remainder.split_whitespace().collect();
        if let Some(last_word) = words.last() {
            if !last_word.starts_with("task_") && *last_word != "to" {
                return Some(last_word.to_string());
            }
        }
    }
    if let Some(named_idx) = m.find("agent named ") {
        let remainder = &m[named_idx + "agent named ".len()..];
        let agent_name: String = remainder
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if !agent_name.is_empty() {
            return Some(agent_name);
        }
    }
    None
}

/// Lead-in words that are never part of an agent's name. Dropped before resolving an
/// assignee phrase against the roster so "the researcher" / "our research agent" match
/// the agent `researcher` / `research-agent` rather than the literal first word.
const ASSIGNEE_STOPWORDS: &[&str] = &[
    "the", "a", "an", "to", "this", "that", "it", "them", "agent", "operative", "our",
    "named", "called", "please", "task", "for",
];

/// The full trailing assignment phrase a user named ("the researcher",
/// "research-agent"), with any `task_…` id token removed, or `None` when no assignment
/// cue is present.
///
/// This is the phrase [`resolve_assignee`] matches against the live agent roster. Unlike
/// [`extract_agent_id_from_assignment`] (which takes only the FIRST word, kept as the
/// deterministic "did the user name an agent?" presence signal the clarify branches
/// still use), this keeps the whole multi-word phrase so a fuzzy reference can resolve.
pub(crate) fn extract_assignee_phrase(message: &str) -> Option<String> {
    let m = message.to_lowercase();
    let remainder = if let Some(i) = m.find(" to ") {
        &m[i + " to ".len()..]
    } else if let Some(i) = m.find("agent named ") {
        &m[i + "agent named ".len()..]
    } else {
        return None;
    };
    let phrase = remainder
        .split_whitespace()
        .filter(|w| !w.starts_with("task_"))
        .collect::<Vec<_>>()
        .join(" ");
    let phrase = phrase.trim().to_string();
    if phrase.is_empty() {
        None
    } else {
        Some(phrase)
    }
}

/// The outcome of resolving an assignment target phrase against the live agent roster.
///
/// Reference-grounded (`docs/reference-driven-development.md`): mirrors openclaw's
/// `resolveSubagentTargetFromRuns` (exact → unique-prefix → ambiguous-is-an-error,
/// never resolving to a target that does not exist) and Hermes' `repair_tool_call`
/// (normalize/strip, then match against the KNOWN set). A `Resolved` id is ALWAYS one
/// already on the roster — the resolver can never invent an assignee (fail closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssigneeResolution {
    /// Exactly one existing agent matched the phrase.
    Resolved(String),
    /// The phrase matched more than one existing agent — ask which one.
    Ambiguous(Vec<String>),
    /// No usable phrase, or it matched no agent on the roster.
    Unresolved,
}

/// Resolve a named assignee `phrase` against the live agent `roster`, returning a single
/// EXISTING agent id, an ambiguity, or nothing.
///
/// The match runs in fail-closed priority order (the openclaw target-resolution shape):
/// exact (case-insensitive) id/name → unique **skill/tag** → unique prefix → unique
/// substring. A tier with more than one distinct match is `Ambiguous` (asked about, never
/// guessed); a tier with exactly one is `Resolved`. Stopwords and sub-2-char noise are
/// dropped first (the Hermes normalize step), and a returned id is always taken verbatim
/// from the roster — so a fuzzy phrase can only ever name an agent that actually exists.
///
/// The skill tier sits AFTER the exact id/name match and BEFORE the looser prefix/substring
/// fallback (`docs/relix-dashboard-design.md` §9.1): a phrase like "the researcher" resolves
/// to the single agent tagged `researcher`, but if two agents share that skill it is
/// `Ambiguous` (Prime asks which one) — a shared skill is never silently guessed.
/// `agent_skills` maps an agent id to its specialty slugs (`summary.agent_skills`).
pub(crate) fn resolve_assignee(
    phrase: &str,
    roster: &[String],
    agent_skills: &[(String, Vec<String>)],
) -> AssigneeResolution {
    let toks: Vec<String> = phrase
        .to_lowercase()
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                .to_string()
        })
        .filter(|w| w.len() >= 2 && !ASSIGNEE_STOPWORDS.contains(&w.as_str()))
        .collect();
    if toks.is_empty() {
        return AssigneeResolution::Unresolved;
    }

    // Candidates: the whole phrase joined (hyphen and space forms, so "research agent"
    // matches the id "research-agent"), then each significant token.
    let mut candidates: Vec<String> = Vec::new();
    if toks.len() > 1 {
        candidates.push(toks.join("-"));
        candidates.push(toks.join(" "));
    }
    candidates.extend(toks.iter().cloned());

    let roster_lc: Vec<(String, String)> = roster
        .iter()
        .map(|id| (id.to_lowercase(), id.clone()))
        .collect();

    let collect = |pred: &dyn Fn(&str, &str) -> bool| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (lc, orig) in &roster_lc {
            if candidates.iter().any(|c| pred(lc, c)) && !out.contains(orig) {
                out.push(orig.clone());
            }
        }
        out
    };

    let exact = collect(&|lc, c| lc == c);
    match exact.len() {
        1 => return AssigneeResolution::Resolved(exact[0].clone()),
        n if n > 1 => return AssigneeResolution::Ambiguous(exact),
        _ => {}
    }

    // Skill/tag tier: an agent whose specialty slugs contain one of the candidate slugs.
    // Matched by exact slug equality (both sides are normalized slugs), so "research"
    // routes to the agent tagged `research` but never to an unrelated longer word.
    // Unique → resolve; shared by more than one agent → ambiguous (asked, never guessed).
    let mut skill_hits: Vec<String> = Vec::new();
    for (lc, orig) in &roster_lc {
        let tagged = agent_skills.iter().any(|(aid, skills)| {
            aid.to_lowercase() == *lc
                && skills
                    .iter()
                    .any(|s| candidates.iter().any(|c| c == &s.to_lowercase()))
        });
        if tagged && !skill_hits.contains(orig) {
            skill_hits.push(orig.clone());
        }
    }
    match skill_hits.len() {
        1 => return AssigneeResolution::Resolved(skill_hits[0].clone()),
        n if n > 1 => return AssigneeResolution::Ambiguous(skill_hits),
        _ => {}
    }

    let prefix = collect(&|lc, c| c.len() >= 2 && lc.starts_with(c));
    match prefix.len() {
        1 => return AssigneeResolution::Resolved(prefix[0].clone()),
        n if n > 1 => return AssigneeResolution::Ambiguous(prefix),
        _ => {}
    }

    let contains = collect(&|lc, c| c.len() >= 3 && lc.contains(c));
    match contains.len() {
        1 => return AssigneeResolution::Resolved(contains[0].clone()),
        n if n > 1 => return AssigneeResolution::Ambiguous(contains),
        _ => {}
    }

    AssigneeResolution::Unresolved
}

/// True when a message stands on its OWN as a fresh request — a complete actionable
/// command, an explicit command phrase, or a question — rather than reading as a bare
/// answer to an earlier clarifying question (e.g. a lone `task_0001` or `researcher`).
///
/// This is the gate the multi-turn clarification memory uses to decide whether a
/// follow-up message should *resolve* a pending clarification (a bare answer → combine
/// with the original) or *supersede* it (a fresh request → drop the pending context and
/// handle the new message on its own). It deliberately reuses the SAME deterministic
/// classifier + command/question rails the turn would use, so the decision matches how
/// the message would actually be handled (sections 10.5, 17.1). A bare value the
/// classifier reads as `DirectAnswer`/`Greeting` (and that is neither an explicit command
/// nor a question) is NOT standalone, so it continues the pending request.
pub fn is_standalone_request(message: &str) -> bool {
    let lower = message.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    use PrimeIntent as I;
    let actionable = matches!(
        classify_intent(message),
        I::TaskCreation
            | I::CreateAndRunTask
            | I::AssignTask
            | I::RunStart
            | I::RunRetry
            | I::AgentCreation
            | I::PluginInstallation
            | I::PermissionChange
            | I::Orchestration
            | I::OrchestrationRun
            | I::PlanRequest
            | I::ToolInvocation
            | I::StatusQuestion
            | I::ApprovalResponse
    );
    actionable || is_explicit_command(&lower) || is_question(&lower)
}

/// A short, human label for what an actionable `Clarify` turn is still missing, shown on
/// the "waiting for: …" chip and stored on the [`relux_core::PendingClarification`]
/// record. Grounded in the same deterministic extractors `decide` used, so the label
/// names the field that is actually absent.
pub fn clarify_needs_label(intent: &PrimeIntent, message: &str) -> String {
    match intent {
        PrimeIntent::AssignTask => {
            let has_task = extract_task_id(message).is_some();
            let has_agent = extract_agent_id_from_assignment(message).is_some();
            match (has_task, has_agent) {
                (false, true) => "task id".to_string(),
                (true, false) => "agent".to_string(),
                _ => "task id and agent".to_string(),
            }
        }
        PrimeIntent::TaskCreation | PrimeIntent::CreateAndRunTask => "task description".to_string(),
        PrimeIntent::RunStart => "task id".to_string(),
        PrimeIntent::OrchestrationRun => "orchestration id".to_string(),
        PrimeIntent::TaskUpdate => {
            let has_task = extract_task_id(message).is_some();
            let has_field = update_change_phrase(message).is_some();
            match (has_task, has_field) {
                (false, true) => "task id".to_string(),
                (true, false) => "the field to change".to_string(),
                _ => "task id and change".to_string(),
            }
        }
        _ => "more detail".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_summary() -> StateSummary {
        StateSummary {
            plugins: 0,
            agents: 0,
            tasks_total: 0,
            tasks_open: 0,
            runs_active: 0,
            tasks_waiting_approval: 0,
            tasks_blocked: 0,
            tasks_failed: 0,
            pending_approvals: 0,
            all_agent_ids: vec![],
            agent_skills: vec![],
            all_task_ids: vec![],
            queued: vec![],
            recent: vec![],
        }
    }

    fn brief(id: &str, title: &str, status: TaskStatus) -> TaskBrief {
        TaskBrief {
            id: relux_core::TaskId::new(id),
            title: title.to_string(),
            status,
            assigned_agent: None,
        }
    }

    fn roster(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    // A typed empty skill map for the id/name-only resolution tests.
    const NO_SKILLS: &[(String, Vec<String>)] = &[];

    fn skills(pairs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        pairs
            .iter()
            .map(|(id, sk)| (id.to_string(), sk.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn resolve_assignee_matches_exact_prefix_and_substring_against_the_roster() {
        let r = roster(&["prime", "researcher", "research-bot"]);
        // Exact (case-insensitive).
        assert_eq!(
            resolve_assignee("researcher", &r, NO_SKILLS),
            AssigneeResolution::Resolved("researcher".to_string())
        );
        assert_eq!(
            resolve_assignee("Researcher", &r, NO_SKILLS),
            AssigneeResolution::Resolved("researcher".to_string())
        );
        // Stopwords are dropped: "the researcher" still resolves.
        assert_eq!(
            resolve_assignee("the researcher", &r, NO_SKILLS),
            AssigneeResolution::Resolved("researcher".to_string())
        );
        // A multi-word phrase joins to the hyphenated id.
        assert_eq!(
            resolve_assignee("research bot", &r, NO_SKILLS),
            AssigneeResolution::Resolved("research-bot".to_string())
        );
    }

    #[test]
    fn resolve_assignee_reports_ambiguity_and_never_invents() {
        let r = roster(&["prime", "researcher", "research-bot"]);
        // "research" prefixes BOTH researcher and research-bot -> ambiguous, never guessed.
        match resolve_assignee("research", &r, NO_SKILLS) {
            AssigneeResolution::Ambiguous(mut m) => {
                m.sort();
                assert_eq!(m, vec!["research-bot".to_string(), "researcher".to_string()]);
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
        // An unknown name matches nothing on the roster (fail closed).
        assert_eq!(
            resolve_assignee("missing-agent", &r, NO_SKILLS),
            AssigneeResolution::Unresolved
        );
        // A phrase of only stopwords/noise resolves to nothing.
        assert_eq!(
            resolve_assignee("the agent", &r, NO_SKILLS),
            AssigneeResolution::Unresolved
        );
    }

    #[test]
    fn resolve_assignee_routes_a_unique_skill_to_its_specialist() {
        // Two agents with opaque ids; only their skills connect a phrase to them.
        let r = roster(&["agent-a", "agent-b"]);
        let sk = skills(&[("agent-a", &["rust", "backend"]), ("agent-b", &["design"])]);
        // A phrase naming a skill held by exactly one agent resolves to that agent —
        // even though the id/name match nothing. Stopwords are dropped first.
        assert_eq!(
            resolve_assignee("the rust specialist", &r, &sk),
            AssigneeResolution::Resolved("agent-a".to_string())
        );
        assert_eq!(
            resolve_assignee("design", &r, &sk),
            AssigneeResolution::Resolved("agent-b".to_string())
        );
        // A skill no agent holds falls through to Unresolved (never invented).
        assert_eq!(
            resolve_assignee("kubernetes", &r, &sk),
            AssigneeResolution::Unresolved
        );
    }

    #[test]
    fn resolve_assignee_clarifies_a_shared_skill_instead_of_guessing() {
        // Both agents share the "rust" skill -> ambiguous, Prime must ask which one.
        let r = roster(&["agent-a", "agent-b"]);
        let sk = skills(&[("agent-a", &["rust"]), ("agent-b", &["rust"])]);
        match resolve_assignee("rust", &r, &sk) {
            AssigneeResolution::Ambiguous(mut m) => {
                m.sort();
                assert_eq!(m, vec!["agent-a".to_string(), "agent-b".to_string()]);
            }
            other => panic!("expected ambiguity on a shared skill, got {other:?}"),
        }
    }

    #[test]
    fn resolve_assignee_prefers_an_exact_id_over_a_skill_match() {
        // An exact id/name match wins before the skill tier is consulted: "researcher"
        // is an id, so it resolves to that agent even though another agent is tagged
        // with the "researcher" skill.
        let r = roster(&["researcher", "helper"]);
        let sk = skills(&[("helper", &["researcher"])]);
        assert_eq!(
            resolve_assignee("researcher", &r, &sk),
            AssigneeResolution::Resolved("researcher".to_string())
        );
    }

    #[test]
    fn assign_decide_resolves_a_fuzzy_assignee_against_the_roster() {
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0001"]);
        s.all_agent_ids = roster(&["prime", "researcher"]);
        // The motivating dialogue's combined message: a fuzzy "the researcher" + a task id.
        let plan = decide(
            "assign this to the researcher task_0001",
            &PrimeIntent::AssignTask,
            &s,
        );
        match plan {
            PrimePlan::Act {
                action: PrimeAction::AssignTask { task_id, agent_id },
                ..
            } => {
                assert_eq!(task_id, "task_0001");
                assert_eq!(agent_id, "researcher");
            }
            other => panic!("expected an AssignTask Act, got {other:?}"),
        }
    }

    #[test]
    fn assign_decide_clarifies_an_ambiguous_assignee() {
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0001"]);
        s.all_agent_ids = roster(&["researcher", "research-bot"]);
        let plan = decide(
            "assign task_0001 to research",
            &PrimeIntent::AssignTask,
            &s,
        );
        match plan {
            PrimePlan::Clarify { text } => {
                assert!(text.contains("More than one agent matches"), "got: {text}");
                assert!(text.contains("researcher") && text.contains("research-bot"));
            }
            other => panic!("expected a Clarify, got {other:?}"),
        }
    }

    #[test]
    fn assign_decide_routes_by_a_unique_skill() {
        // The roster ids are opaque; only the skill connects "the rust specialist" to
        // the right agent. The decide() arm resolves it through the skill tier.
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0001"]);
        s.all_agent_ids = roster(&["agent-a", "agent-b"]);
        s.agent_skills = skills(&[("agent-a", &["rust"]), ("agent-b", &["design"])]);
        let plan = decide(
            "assign task_0001 to the rust specialist",
            &PrimeIntent::AssignTask,
            &s,
        );
        match plan {
            PrimePlan::Act {
                action: PrimeAction::AssignTask { task_id, agent_id },
                ..
            } => {
                assert_eq!(task_id, "task_0001");
                assert_eq!(agent_id, "agent-a");
            }
            other => panic!("expected an AssignTask Act, got {other:?}"),
        }
    }

    #[test]
    fn assign_decide_clarifies_a_shared_skill_instead_of_assigning() {
        // Two agents share the "rust" skill -> Prime must ask, never guess an assignee.
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0001"]);
        s.all_agent_ids = roster(&["agent-a", "agent-b"]);
        s.agent_skills = skills(&[("agent-a", &["rust"]), ("agent-b", &["rust"])]);
        let plan = decide(
            "assign task_0001 to the rust specialist",
            &PrimeIntent::AssignTask,
            &s,
        );
        match plan {
            PrimePlan::Clarify { text } => {
                assert!(text.contains("More than one agent matches"), "got: {text}");
                assert!(text.contains("agent-a") && text.contains("agent-b"));
            }
            other => panic!("expected a Clarify on a shared skill, got {other:?}"),
        }
    }

    #[test]
    fn assign_decide_still_rejects_an_unknown_agent() {
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0001"]);
        s.all_agent_ids = roster(&["prime"]);
        let plan = decide(
            "assign task_0001 to missing-agent",
            &PrimeIntent::AssignTask,
            &s,
        );
        match plan {
            PrimePlan::Reply { text } => {
                assert!(text.contains("Agent with ID 'missing-agent' does not exist."));
            }
            other => panic!("expected a does-not-exist Reply, got {other:?}"),
        }
    }

    #[test]
    fn classifies_the_master_plan_examples() {
        // Examples drawn from RELUX_MASTER_PLAN.md section 1 and section 10.1.
        assert_eq!(classify_intent("hey"), PrimeIntent::Greeting);
        assert_eq!(classify_intent("hi prime"), PrimeIntent::Greeting);
        assert_eq!(
            classify_intent("what is going on?"),
            PrimeIntent::StatusQuestion
        );
        assert_eq!(
            classify_intent("show me active runs"),
            PrimeIntent::StatusQuestion
        );
        assert_eq!(
            classify_intent("fix the login bug"),
            PrimeIntent::TaskCreation
        );
        assert_eq!(classify_intent("start it"), PrimeIntent::RunStart);
        assert_eq!(
            classify_intent("hire another coding agent"),
            PrimeIntent::AgentCreation
        );
        assert_eq!(
            classify_intent("give this agent GitHub access"),
            PrimeIntent::PermissionChange
        );
        assert_eq!(
            classify_intent("why did it fail?"),
            PrimeIntent::ExplanationRequest
        );
        assert_eq!(
            classify_intent("retry the failed run"),
            PrimeIntent::RunRetry
        );
    }

    #[test]
    fn greeting_only_matches_whole_words() {
        // "this" contains "hi" but is not a greeting.
        assert_ne!(classify_intent("is this thing on"), PrimeIntent::Greeting);
    }

    #[test]
    fn classifies_tool_discovery_and_invocation() {
        // Discovery: the user wants the list of usable tools.
        assert_eq!(
            classify_intent("what tools can you use?"),
            PrimeIntent::ToolDiscovery
        );
        assert_eq!(
            classify_intent("list your tools"),
            PrimeIntent::ToolDiscovery
        );
        // Invocation: explicit echo/status/tool requests.
        assert_eq!(classify_intent("echo hello"), PrimeIntent::ToolInvocation);
        assert_eq!(
            classify_intent("use echo.say with {\"x\":1}"),
            PrimeIntent::ToolInvocation
        );
        assert_eq!(
            classify_intent("run the status tool"),
            PrimeIntent::ToolInvocation
        );
        assert_eq!(
            classify_intent("use the github tool"),
            PrimeIntent::ToolInvocation
        );
        // "start it"/"run it" reference no tool - run control, not tool use.
        assert_eq!(classify_intent("start it"), PrimeIntent::RunStart);
        assert_eq!(classify_intent("run it"), PrimeIntent::RunStart);
        // A plain status question and task creation are untouched.
        assert_eq!(
            classify_intent("what is going on?"),
            PrimeIntent::StatusQuestion
        );
        assert_eq!(
            classify_intent("fix the login bug"),
            PrimeIntent::TaskCreation
        );
    }

    #[test]
    fn classifies_explicit_multi_tool_plan_requests() {
        // An explicit ordered multi-tool command previews an INERT plan, distinct
        // from a single tool invocation (`docs/mcp.md` "Run-driven multi-tool plan").
        assert_eq!(
            classify_intent("use the status tool then the echo tool"),
            PrimeIntent::ToolPlanRequest
        );
        assert_eq!(
            classify_intent("run these tools in order: status then echo hello"),
            PrimeIntent::ToolPlanRequest
        );
        assert_eq!(
            classify_intent("echo hi then echo bye"),
            PrimeIntent::ToolPlanRequest
        );
        // A SINGLE tool stays ToolInvocation — one step is not a plan.
        assert_eq!(classify_intent("echo hello"), PrimeIntent::ToolInvocation);
        assert_eq!(
            classify_intent("run the status tool"),
            PrimeIntent::ToolInvocation
        );
    }

    #[test]
    fn parses_explicit_mcp_tool_reference() {
        // An `mcp:<server>/<tool>` token resolves to the `mcp:<server>` synthetic plugin
        // id + the tool name, case-PRESERVED (MCP tool names may be camelCase), so the
        // plan-grounding path can look it up in the live MCP catalog and the resolved
        // step lands in the SAME `mcp:<server>` task tool_plan shape (`docs/mcp.md`).
        assert_eq!(
            parse_tool_request("mcp:fs/search"),
            Some(("mcp:fs".to_string(), "search".to_string(), "{}".to_string()))
        );
        // Case preserved, trailing punctuation trimmed, inline JSON args lifted.
        assert_eq!(
            parse_tool_request("use mcp:notes/listNotes with {\"q\":\"x\"}"),
            Some((
                "mcp:notes".to_string(),
                "listNotes".to_string(),
                "{\"q\":\"x\"}".to_string()
            ))
        );
        // A bare `mcp:` with no tool segment resolves nothing (never a half tool ref).
        assert_eq!(parse_tool_request("mcp:fs"), None);
        assert_eq!(parse_tool_request("mcp:/search"), None);
    }

    #[test]
    fn classifies_explicit_mcp_multi_tool_plan() {
        // Two MCP tool references in sequence are a multi-tool plan, exactly like two
        // installed-tool references — the same `parse_tool_request` drives both.
        assert_eq!(
            classify_intent("use mcp:fs/search then mcp:fs/read"),
            PrimeIntent::ToolPlanRequest
        );
        // A single MCP reference is one step, not a multi-tool plan.
        assert_ne!(
            classify_intent("run mcp:fs/search"),
            PrimeIntent::ToolPlanRequest
        );
        // A casual MENTION of an mcp ref with no plan/sequence cue is never a plan.
        assert_ne!(
            classify_intent("what does mcp:fs/search even do"),
            PrimeIntent::ToolPlanRequest
        );
    }

    #[test]
    fn casual_chat_with_a_connector_is_never_a_tool_plan() {
        // A "then" in ordinary conversation, ideation, or a question must NOT be read
        // as a tool plan: with no tool references the segments resolve to nothing
        // (Hermes-first: §10.5, §17.1). These stay conversational / work as before.
        for msg in [
            "let me think then I'll decide",
            "first we plan then we build",
            "should we ship then iterate?",
            "i'm so frustrated, nothing works then it breaks again",
        ] {
            assert_ne!(
                classify_intent(msg),
                PrimeIntent::ToolPlanRequest,
                "{msg:?} must not be a tool plan"
            );
        }
        // "run the build then run the tests" references no installed TOOL (build/tests
        // are work, not tools), so it stays task creation, exactly as before.
        assert_eq!(
            classify_intent("run the build then run the tests"),
            PrimeIntent::TaskCreation
        );
    }

    #[test]
    fn splits_multi_tool_segments_and_strips_lead_ins() {
        // The lead-in is stripped and the connectors split the steps in order.
        assert_eq!(
            split_tool_plan_segments("run these tools in order: status then echo hello"),
            vec!["status".to_string(), "echo hello".to_string()]
        );
        // Numbered markers and "and then" both split.
        assert_eq!(
            split_tool_plan_segments("1. run the status tool and then 2. echo done"),
            vec!["run the status tool".to_string(), "echo done".to_string()]
        );
        // A non-ASCII message never panics and still splits on the ASCII connector.
        assert_eq!(
            split_tool_plan_segments("echo café then echo déjà"),
            vec!["echo café".to_string(), "echo déjà".to_string()]
        );
    }

    #[test]
    fn parses_tool_requests_into_plugin_tool_input() {
        // echo by name, with the trailing text as the message.
        assert_eq!(
            parse_tool_request("echo hello"),
            Some((
                "relux-tools-echo".to_string(),
                "echo.say".to_string(),
                "{\"message\":\"hello\"}".to_string()
            ))
        );
        // explicit plugin/tool with inline JSON input.
        assert_eq!(
            parse_tool_request("use relux-tools-github/github.create_pr {\"n\":1}"),
            Some((
                "relux-tools-github".to_string(),
                "github.create_pr".to_string(),
                "{\"n\":1}".to_string()
            ))
        );
        // a known ToolSet keyword names the plugin; the kernel resolves the tool.
        assert_eq!(
            parse_tool_request("use the github tool"),
            Some((
                "relux-tools-github".to_string(),
                String::new(),
                "{}".to_string()
            ))
        );
        // nothing nameable -> None (Prime will ask).
        assert_eq!(parse_tool_request("do the thing"), None);
    }

    #[test]
    fn greeting_is_conversational_and_never_a_work_board_prompt() {
        // Hermes-first: a plain hello gets a plain, general reply — never a Plan, and
        // never the old "the board's empty / what do you want to set up" steering. The
        // reply must not volunteer board/queue/crew state or push the user into work
        // setup on casual chat (`docs/prime-processing-audit.md` "Hermes-first general
        // agent"; §10.5, §17.1).
        let plan = decide("hey", &PrimeIntent::Greeting, &empty_summary());
        match plan {
            PrimePlan::Reply { text } => {
                let lower = text.to_lowercase();
                for banned in ["the board", "queue", "crew", "set up", "what do you want to work on"]
                {
                    assert!(
                        !lower.contains(banned),
                        "a casual greeting must not mention {banned:?}: {text:?}"
                    );
                }
                // It still reads like Prime opening a conversation.
                assert!(lower.contains("prime"));
            }
            other => panic!("greeting must be a Reply, got {other:?}"),
        }
    }

    #[test]
    fn frustration_and_insults_are_flagged_emotional() {
        for m in [
            "fuck you",
            "this is so frustrating",
            "ugh",
            "lol",
            "you're useless",
            "i give up",
            "meh",
        ] {
            assert!(is_frustration_or_emotional(m), "{m:?} must read as emotional");
        }
        // A real work command that merely contains a charged word is NOT flagged as
        // emotional small talk (and anyway classifies as task creation, not brainstorm).
        for m in [
            "fix the damn login bug",
            "summarize the README",
            "we should refactor the auth module",
            "what is going on?",
        ] {
            assert!(!is_frustration_or_emotional(m), "{m:?} must NOT read as emotional");
        }
    }

    #[test]
    fn only_a_real_work_idea_offers_a_brainstorm_cta() {
        // A genuine idea with a work verb gets the one-click work buttons.
        for m in [
            "we should refactor the auth module",
            "i was thinking we could improve the onboarding flow",
            "what if we automate the release process",
        ] {
            assert!(
                brainstorm_offers_actionable_work(m),
                "{m:?} should offer a work CTA"
            );
        }
        // Emotional / insult / pure small talk gets NO work CTA.
        for m in ["fuck you", "this is so frustrating", "ugh", "lol nice", "i'm so tired"] {
            assert!(
                !brainstorm_offers_actionable_work(m),
                "{m:?} must NOT offer a work CTA"
            );
        }
    }

    #[test]
    fn casual_and_emotional_messages_get_dedicated_conversational_intents() {
        // Hermes-first: throwaway chitchat is its own SmallTalk intent and venting /
        // insults / frustration is EmotionalSupport — represented deliberately, NOT
        // misfiled as pseudo-brainstorming or a generic direct answer
        // (`docs/prime-processing-audit.md` "Hermes-first general agent"; §10.5, §17.1).
        for m in [
            "lol", "haha", "nice", "cool", "thanks", "ok cool", "makes sense", "meh", "gotcha",
        ] {
            assert_eq!(classify_intent(m), PrimeIntent::SmallTalk, "{m:?} is small talk");
        }
        for m in [
            "ugh",
            "fuck you",
            "this is so frustrating",
            "i give up",
            "i'm exhausted",
            "you're useless",
        ] {
            assert_eq!(
                classify_intent(m),
                PrimeIntent::EmotionalSupport,
                "{m:?} is emotional support"
            );
        }
        // Both decide to a plain conversational Reply that never steers to the board.
        for intent in [PrimeIntent::SmallTalk, PrimeIntent::EmotionalSupport] {
            match decide("ugh", &intent, &empty_summary()) {
                PrimePlan::Reply { text } => {
                    let lower = text.to_lowercase();
                    for banned in
                        ["the board", "queue", "crew", "set up", "what do you want to work on"]
                    {
                        assert!(
                            !lower.contains(banned),
                            "{intent:?} reply must not mention {banned:?}: {text:?}"
                        );
                    }
                }
                other => panic!("{intent:?} must be a Reply, got {other:?}"),
            }
        }
    }

    #[test]
    fn explicit_work_and_questions_beat_casual_and_emotional_detection() {
        // The conversational catch is LAST and conservative: a genuine command or a
        // real question that merely contains a charged or casual word still routes to
        // its true intent and is never swallowed as chitchat (do not weaken action).
        assert_eq!(
            classify_intent("fix the damn login bug"),
            PrimeIntent::TaskCreation
        );
        assert_eq!(
            classify_intent("create a task to clean up the logs"),
            PrimeIntent::TaskCreation
        );
        // A longer sentence that merely contains a casual token is not chitchat.
        assert_eq!(
            classify_intent("summarize the nice clean readme"),
            PrimeIntent::TaskCreation
        );
        // And the boundary predicates do not bite full sentences.
        assert!(!is_casual_chat("nice work on the build today, can you continue it"));
        assert!(!is_emotional_distress("fix the damn login bug"));
    }

    #[test]
    fn bare_creation_verbs_classify_as_task_creation() {
        // The most natural task verb must not fall through to a canned answer
        // (section 10.1). "create" and "add" previously did because their needles
        // required a trailing object.
        assert_eq!(classify_intent("create"), PrimeIntent::TaskCreation);
        assert_eq!(classify_intent("add"), PrimeIntent::TaskCreation);
        assert_eq!(classify_intent("fix"), PrimeIntent::TaskCreation);
    }

    #[test]
    fn contentless_creation_request_clarifies_instead_of_minting_a_task() {
        // A bare verb names no work: Prime asks rather than creating a junk task
        // titled "create"/"fix" (section 10.5).
        for msg in ["create", "fix", "please create"] {
            let plan = decide(msg, &PrimeIntent::TaskCreation, &empty_summary());
            assert!(
                matches!(plan, PrimePlan::Clarify { .. }),
                "{msg:?} must clarify, got {plan:?}"
            );
        }
    }

    #[test]
    fn task_creation_plans_a_create_task_action() {
        let plan = decide(
            "create a task to summarize the README",
            &PrimeIntent::TaskCreation,
            &empty_summary(),
        );
        match plan {
            PrimePlan::Act {
                action: PrimeAction::CreateTask { title },
                ..
            } => assert_eq!(title, "summarize the README"),
            other => panic!("expected Act/CreateTask, got {other:?}"),
        }
    }

    #[test]
    fn create_and_run_task_plans_a_create_and_run_action() {
        let plan = decide(
            "create a task to summarize the README and run it",
            &PrimeIntent::CreateAndRunTask,
            &empty_summary(),
        );
        match plan {
            PrimePlan::Act {
                action: PrimeAction::CreateAndRunTask { title },
                ..
            } => assert_eq!(title, "summarize the README"),
            other => panic!("expected Act/CreateAndRunTask, got {other:?}"),
        }
    }

    #[test]
    fn run_start_clarifies_when_nothing_is_ready() {
        let plan = decide("start it", &PrimeIntent::RunStart, &empty_summary());
        assert!(matches!(plan, PrimePlan::Clarify { .. }), "got {plan:?}");
    }

    #[test]
    fn run_start_acts_on_the_single_ready_task() {
        let mut s = empty_summary();
        s.queued = vec![brief("task_0007", "Run the tests", TaskStatus::Queued)];
        let plan = decide("start it", &PrimeIntent::RunStart, &s);
        match plan {
            PrimePlan::Act {
                action: PrimeAction::StartRun { task_id },
                ..
            } => assert_eq!(task_id, "task_0007"),
            other => panic!("expected Act/StartRun, got {other:?}"),
        }
    }

    #[test]
    fn run_start_honors_an_explicit_ready_task_id() {
        // When the user (or a continued clarification) names a ready task id, that one is
        // started even when several are ready — no guessing off the queue.
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0007", "task_0008"]);
        s.queued = vec![
            brief("task_0007", "Run the tests", TaskStatus::Queued),
            brief("task_0008", "Build the docs", TaskStatus::Queued),
        ];
        let plan = decide("start task_0008", &PrimeIntent::RunStart, &s);
        match plan {
            PrimePlan::Act {
                action: PrimeAction::StartRun { task_id },
                ..
            } => assert_eq!(task_id, "task_0008"),
            other => panic!("expected Act/StartRun for the named id, got {other:?}"),
        }
    }

    #[test]
    fn run_start_reports_an_unready_or_unknown_explicit_id() {
        let mut s = empty_summary();
        s.all_task_ids = roster(&["task_0007"]);
        // Exists but not in the ready queue.
        let plan = decide("start task_0007", &PrimeIntent::RunStart, &s);
        match plan {
            PrimePlan::Reply { text } => assert!(text.contains("not ready to start"), "got: {text}"),
            other => panic!("expected a not-ready Reply, got {other:?}"),
        }
        // Does not exist at all.
        let plan = decide("start task_9999", &PrimeIntent::RunStart, &s);
        match plan {
            PrimePlan::Reply { text } => {
                assert!(text.contains("does not exist"), "got: {text}")
            }
            other => panic!("expected a does-not-exist Reply, got {other:?}"),
        }
    }

    #[test]
    fn permission_change_is_proposed_behind_approval() {
        let plan = decide(
            "give this agent GitHub access",
            &PrimeIntent::PermissionChange,
            &empty_summary(),
        );
        match plan {
            PrimePlan::Propose { action, risk, .. } => {
                assert!(matches!(action, PrimeAction::GrantPermission { .. }));
                assert_eq!(risk, RiskLevel::High);
            }
            other => panic!("permission change must be a Propose, got {other:?}"),
        }
    }

    #[test]
    fn classifies_orchestration_requests() {
        assert_eq!(
            classify_intent("orchestrate research the options, build a prototype, and write docs"),
            PrimeIntent::Orchestration
        );
        assert_eq!(
            classify_intent("split this across agents: investigate, implement, and test"),
            PrimeIntent::Orchestration
        );
        assert_eq!(
            classify_intent("coordinate the release across multiple agents"),
            PrimeIntent::Orchestration
        );
        // A plain task creation is untouched (no coordination phrasing).
        assert_eq!(
            classify_intent("fix the login bug"),
            PrimeIntent::TaskCreation
        );
        // A greeting never becomes orchestration.
        assert_eq!(classify_intent("hey"), PrimeIntent::Greeting);
    }

    #[test]
    fn classifies_orchestration_run_distinct_from_create() {
        // Running/continuing an EXISTING orchestration is its own intent, distinct from
        // creating one. Keyed on a run/continue verb + the orchestration noun or an `orch_` id.
        assert_eq!(
            classify_intent("run the orchestration"),
            PrimeIntent::OrchestrationRun
        );
        assert_eq!(
            classify_intent("run orch_0001"),
            PrimeIntent::OrchestrationRun
        );
        assert_eq!(
            classify_intent("continue orch_0002"),
            PrimeIntent::OrchestrationRun
        );
        assert_eq!(
            classify_intent("start the orchestration batch"),
            PrimeIntent::OrchestrationRun
        );
        // Creating one is still the create intent (keyed on "orchestrate"/…).
        assert_eq!(
            classify_intent("orchestrate research, build, and test"),
            PrimeIntent::Orchestration
        );
        // A QUESTION about running stays a conversation, never an action.
        assert_eq!(
            classify_intent("should we run the orchestration?"),
            PrimeIntent::Brainstorming
        );
        // A bare "run it" (no orchestration noun / id) is still single-task run control.
        assert_eq!(classify_intent("run it"), PrimeIntent::RunStart);
    }

    #[test]
    fn extract_orchestration_id_recovers_an_explicit_reference() {
        assert_eq!(
            extract_orchestration_id("please run orch_0007 now"),
            Some("orch_0007".to_string())
        );
        assert_eq!(extract_orchestration_id("run the orchestration"), None);
        assert_eq!(extract_orchestration_id("start task_0001"), None);
    }

    #[test]
    fn orchestration_run_acts_on_a_named_id_and_clarifies_without_one() {
        // A named id becomes a RunOrchestration Act (the kernel validates existence at run time).
        let plan = decide("run orch_0003", &PrimeIntent::OrchestrationRun, &empty_summary());
        match plan {
            PrimePlan::Act {
                action: PrimeAction::RunOrchestration { orchestration_id },
                ..
            } => assert_eq!(orchestration_id, "orch_0003"),
            other => panic!("expected Act/RunOrchestration, got {other:?}"),
        }
        // No id named → a resolvable clarify (the memory + a bare-id follow-up continue it).
        let plan = decide(
            "run the orchestration",
            &PrimeIntent::OrchestrationRun,
            &empty_summary(),
        );
        assert!(matches!(plan, PrimePlan::Clarify { .. }));
    }

    #[test]
    fn orchestration_acts_on_a_multi_step_goal() {
        let mut s = empty_summary();
        s.all_agent_ids = vec![
            "prime".to_string(),
            "research-agent".to_string(),
            "code-agent".to_string(),
        ];
        let plan = decide(
            "orchestrate research the options, implement a prototype, and write the docs",
            &PrimeIntent::Orchestration,
            &s,
        );
        match plan {
            PrimePlan::Act {
                action: PrimeAction::OrchestrateGoal { goal },
                ..
            } => assert!(
                goal.contains("research the options"),
                "goal should keep the substance, got {goal:?}"
            ),
            other => panic!("expected Act/OrchestrateGoal, got {other:?}"),
        }
    }

    #[test]
    fn orchestration_clarifies_a_single_step_goal() {
        // A coordination request whose goal does not actually split must ask, not
        // fan out a single brief into a fake "orchestration" (section 10.5).
        let plan = decide(
            "orchestrate summarizing the README",
            &PrimeIntent::Orchestration,
            &empty_summary(),
        );
        assert!(matches!(plan, PrimePlan::Clarify { .. }), "got {plan:?}");
    }

    #[test]
    fn orchestration_clarify_reflects_the_parsed_goal() {
        // Same reflect-and-clarify shape as brainstorming: a single-step goal is
        // echoed back so the user sees what Prime understood, then asked to split
        // it — not a generic nudge (section 10.5).
        let plan = decide(
            "orchestrate summarizing the README",
            &PrimeIntent::Orchestration,
            &empty_summary(),
        );
        let text = match plan {
            PrimePlan::Clarify { text } => text,
            other => panic!("expected Clarify, got {other:?}"),
        };
        assert!(
            text.contains("summarizing the README"),
            "clarify must reflect the parsed goal, got {text:?}"
        );
        assert!(text.contains('?'), "clarify must ask for the steps, got {text:?}");

        // A bare directive that strips to nothing nameable falls back to the
        // generic prompt rather than quoting the whole message back.
        let bare = decide("orchestrate", &PrimeIntent::Orchestration, &empty_summary());
        match bare {
            PrimePlan::Clarify { text } => assert!(
                text.starts_with("That reads like a single piece of work"),
                "no-goal request falls back to the generic prompt, got {text:?}"
            ),
            other => panic!("expected Clarify, got {other:?}"),
        }
    }

    #[test]
    fn classifies_plan_requests() {
        // The explicit "idea -> plan -> tasks" rung: a plan ask classifies as
        // PlanRequest, not task creation or orchestration (§10 planning layer).
        for msg in [
            "plan this out",
            "make a plan to redo the onboarding flow",
            "draft a plan for the migration",
            "give me a plan to ship the beta",
            "plan out research the options, build a prototype, and write docs",
            "outline the steps to launch",
        ] {
            assert_eq!(
                classify_intent(msg),
                PrimeIntent::PlanRequest,
                "{msg:?} should be a plan request"
            );
        }
        // "plan and assign" is a COMMIT, not a preview: it stays Orchestration.
        assert_eq!(
            classify_intent("plan and assign the release across the agents"),
            PrimeIntent::Orchestration
        );
        // An ideation lead-in plus an explicit plan ask escapes Brainstorming so
        // the idea reaches the plan rung (§10.5: explicit command overrides musing).
        assert_eq!(
            classify_intent("i was thinking we could make a plan to overhaul billing"),
            PrimeIntent::PlanRequest
        );
        // Plain task creation is untouched (no plan phrasing).
        assert_eq!(classify_intent("fix the login bug"), PrimeIntent::TaskCreation);
    }

    #[test]
    fn plan_request_previews_a_multi_step_plan_without_creating() {
        // A plan request must be ACTION-FREE: it previews the steps and creates
        // nothing. The commit is a separate explicit click (§10.5, §17.1).
        let mut s = empty_summary();
        s.all_agent_ids = vec![
            "prime".to_string(),
            "research-agent".to_string(),
            "code-agent".to_string(),
        ];
        let plan = decide(
            "plan out research the options, implement a prototype, and write the docs",
            &PrimeIntent::PlanRequest,
            &s,
        );
        let text = match plan {
            // No Act, no Propose: a plan preview never mints or queues work.
            PrimePlan::Reply { text } => text,
            other => panic!("a plan preview must be an action-free Reply, got {other:?}"),
        };
        assert!(
            text.contains("research the options"),
            "preview must reflect the goal, got {text:?}"
        );
        assert!(
            text.to_lowercase().contains("nothing is created"),
            "preview must state nothing is created yet, got {text:?}"
        );
    }

    #[test]
    fn plan_request_single_step_steers_to_one_task() {
        // A goal that does not genuinely split is steered to the one-task path
        // rather than fanned into a storm (§10.5). Still action-free.
        let plan = decide(
            "plan out summarizing the README",
            &PrimeIntent::PlanRequest,
            &empty_summary(),
        );
        match plan {
            PrimePlan::Reply { text } => assert!(
                text.contains("single piece of work"),
                "single-step plan steers to one task, got {text:?}"
            ),
            other => panic!("expected an action-free Reply, got {other:?}"),
        }
    }

    #[test]
    fn plan_goal_round_trips_with_orchestration() {
        // The goal the preview is built from must equal the goal the "Create these
        // tasks" suggestion commits, so the previewed and committed plans decompose
        // from identical input. The suggestion re-wraps as `orchestrate <goal>`.
        let goal = plan_goal("plan out research the options, build a prototype, and write docs");
        assert_eq!(goal, "research the options, build a prototype, and write docs");
        let committed = orchestration_goal(&format!("orchestrate {goal}"));
        assert_eq!(committed, goal, "preview and commit must share the goal");
    }

    /// A summary with the given task ids (queued) and agent ids, for the by-id update
    /// decide tests.
    fn summary_with(task_ids: &[&str], agent_ids: &[&str]) -> StateSummary {
        let mut s = empty_summary();
        s.all_task_ids = task_ids.iter().map(|t| t.to_string()).collect();
        s.all_agent_ids = agent_ids.iter().map(|a| a.to_string()).collect();
        s.tasks_total = task_ids.len();
        s.agents = agent_ids.len();
        s.queued = task_ids
            .iter()
            .map(|id| brief(id, &format!("title for {id}"), TaskStatus::Queued))
            .collect();
        s
    }

    #[test]
    fn task_update_decide_applies_a_simple_command() {
        // The deterministic rail turns a simple, grounded command into a real
        // `UpdateTask` Act (the action is finally wired).
        let s = summary_with(&["task_0001"], &[]);
        match decide("set task_0001 priority to 8", &PrimeIntent::TaskUpdate, &s) {
            PrimePlan::Act {
                action: PrimeAction::UpdateTask { task_id, patch },
                ..
            } => {
                assert_eq!(task_id, "task_0001");
                assert!(patch.contains("\"priority\":8"), "patch carries priority: {patch}");
            }
            other => panic!("expected an UpdateTask Act, got {other:?}"),
        }
    }

    #[test]
    fn task_update_decide_fails_closed_and_refuses_completion() {
        let s = summary_with(&["task_0001"], &[]);
        // An unknown task id fails closed with an honest reply (never a guessed edit).
        match decide("set task_9999 priority to 8", &PrimeIntent::TaskUpdate, &s) {
            PrimePlan::Reply { text } => assert!(text.contains("does not exist"), "got {text:?}"),
            other => panic!("expected a Reply, got {other:?}"),
        }
        // "mark it done" is honestly refused — Prime never fakes a completion.
        match decide("mark task_0001 as done", &PrimeIntent::TaskUpdate, &s) {
            PrimePlan::Reply { text } => assert!(
                text.contains("run lifecycle"),
                "completion is refused honestly, got {text:?}"
            ),
            other => panic!("expected a Reply, got {other:?}"),
        }
    }

    #[test]
    fn task_update_decide_clarifies_when_underspecified() {
        let s = summary_with(&["task_0001"], &[]);
        // A field but no task id reflects the field and asks for the task.
        match decide("set the priority", &PrimeIntent::TaskUpdate, &s) {
            PrimePlan::Clarify { text } => {
                assert!(text.contains("priority"), "reflects the field, got {text:?}");
                assert!(text.contains('?'), "asks a question, got {text:?}");
            }
            other => panic!("expected Clarify, got {other:?}"),
        }
        // A task but no field clarifies too.
        assert!(matches!(
            decide("update task_0001", &PrimeIntent::TaskUpdate, &s),
            PrimePlan::Clarify { .. }
        ));
    }

    #[test]
    fn task_update_is_classified_for_by_id_field_commands() {
        // The classify rail recognizes a task-anchored field command as a by-id update,
        // so the deterministic fallback reaches the right arm without a brain.
        assert_eq!(classify_intent("set task_0001 priority to 8"), PrimeIntent::TaskUpdate);
        assert_eq!(classify_intent("cancel task_0001"), PrimeIntent::TaskUpdate);
        assert_eq!(classify_intent("rename task_0001 to Fix the login page"), PrimeIntent::TaskUpdate);
        // A question about a task is still a conversation, never a silent edit.
        assert_eq!(classify_intent("should I cancel task_0001?"), PrimeIntent::Brainstorming);
    }

    #[test]
    fn ideation_stays_a_conversation_not_a_task() {
        // The exact regression: musing that carries a creation verb must NOT mint a
        // task. "I was thinking to create ..." is someone floating an idea, not a
        // command (section 10.5).
        assert_eq!(
            classify_intent(
                "I was thinking to create a n8n like program using 20 agents but better than n8n"
            ),
            PrimeIntent::Brainstorming
        );
        // A spread of ideation lead-ins, each carrying a creation verb, all stay chat.
        for msg in [
            "I have an idea: build a workflow engine",
            "what if we make a graph editor for agents",
            "can we talk about creating a new orchestration layer",
            "I want to discuss building a plugin marketplace",
            "I'm thinking of writing a new adapter",
        ] {
            assert_eq!(
                classify_intent(msg),
                PrimeIntent::Brainstorming,
                "{msg:?} should stay a brainstorming conversation"
            );
        }
        // And the decided plan is a Reply - never a state change.
        let plan = decide(
            "I was thinking to create a n8n like program using 20 agents but better than n8n",
            &PrimeIntent::Brainstorming,
            &empty_summary(),
        );
        assert!(
            matches!(plan, PrimePlan::Reply { .. }),
            "ideation must be a Reply, got {plan:?}"
        );
    }

    #[test]
    fn brainstorm_candidate_strips_lead_ins_to_the_work() {
        // The "turn this into a task" suggestion recovers the noun phrase that
        // names the work (section 11.1). Best-effort; it only pre-fills the input.
        assert_eq!(
            brainstorm_task_candidate("I was thinking we could redo the onboarding flow"),
            Some("redo the onboarding flow".to_string())
        );
        assert_eq!(
            brainstorm_task_candidate("what if we build a graph editor for agents"),
            Some("build a graph editor for agents".to_string())
        );
        assert_eq!(
            brainstorm_task_candidate("let's discuss the auth flow."),
            Some("the auth flow".to_string())
        );
        // A leading token like "we" must never bite into a real word.
        assert_eq!(
            brainstorm_task_candidate("weather alerts for the dashboard"),
            Some("weather alerts for the dashboard".to_string())
        );
        // Pure connective musing with nothing nameable left yields None.
        assert_eq!(brainstorm_task_candidate("maybe we could"), None);
    }

    #[test]
    fn brainstorm_reply_reflects_the_topic_and_asks_a_clarifying_question() {
        // section 10.5: Prime should "ask clarifying questions when needed". When
        // the idea names a topic, the brainstorming reply reflects that topic and
        // asks ONE concrete follow-up — instead of the old fixed prompt. It stays
        // a conversation: no task is created or run.
        let msg = "I was thinking we could redo the onboarding flow";
        let plan = decide(msg, &PrimeIntent::Brainstorming, &empty_summary());
        let text = match plan {
            PrimePlan::Reply { text } => text,
            other => panic!("brainstorming must stay a Reply, got {other:?}"),
        };
        // It reflects the recovered topic (the noun phrase, lead-ins stripped) and
        // asks a clarifying question, while reaffirming nothing is created yet.
        assert!(
            text.contains("redo the onboarding flow"),
            "reply must reflect the topic, got {text:?}"
        );
        assert!(text.contains('?'), "reply must ask a clarifying question, got {text:?}");
        assert!(
            text.to_lowercase().contains("nothing gets created"),
            "reply must reaffirm it stays a conversation, got {text:?}"
        );

        // Pure connective musing with nothing nameable falls back to the
        // open-ended prompt (still a Reply, still no action).
        let bare = decide("maybe we could", &PrimeIntent::Brainstorming, &empty_summary());
        match bare {
            PrimePlan::Reply { text } => assert!(
                text.contains("let's think it through"),
                "no-topic musing falls back to the open-ended prompt, got {text:?}"
            ),
            other => panic!("expected Reply, got {other:?}"),
        }
    }

    #[test]
    fn explicit_commands_override_ideation_lead_ins() {
        // An explicit imperative still mints/runs work even after an ideation
        // lead-in: the command wins (section 10.5).
        assert_eq!(
            classify_intent("I was thinking, create a task to summarize the README"),
            PrimeIntent::TaskCreation
        );
        assert_eq!(
            classify_intent("I had an idea - orchestrate research, build, and test it"),
            PrimeIntent::Orchestration
        );
        // The canonical explicit task creation is untouched by the new guard.
        assert_eq!(
            classify_intent("create a task to summarize the README"),
            PrimeIntent::TaskCreation
        );
        // "start it" still controls a run, never reads as ideation.
        assert_eq!(classify_intent("start it"), PrimeIntent::RunStart);
    }

    #[test]
    fn questions_about_work_stay_a_conversation_not_a_task() {
        // section 17.1: Prime must understand conversational intent and must not
        // blindly turn every message into a plan. An informational/deliberative
        // QUESTION that merely mentions a work verb is NOT minted into a task - it
        // is answered as a conversation (Brainstorming), which creates nothing.
        for msg in [
            "how does the build work?",
            "what's the best way to fix the flaky tests?",
            "should we refactor the auth module?",
            "is it worth rewriting the scheduler?",
            "what do you think we should build next?",
        ] {
            assert_eq!(
                classify_intent(msg),
                PrimeIntent::Brainstorming,
                "{msg:?} is a question, not a task"
            );
            let plan = decide(msg, &PrimeIntent::Brainstorming, &empty_summary());
            assert!(
                matches!(plan, PrimePlan::Reply { .. }),
                "{msg:?} must stay a Reply, got {plan:?}"
            );
        }
    }

    #[test]
    fn soft_intent_musing_stays_a_conversation_not_a_task() {
        // Declarative soft-intent ("we should ...", "let's ...", "I want to ...") is
        // musing, not a command, so it does not mint a task (section 10.5). Each of
        // these carries a creation verb and would previously have been read as work.
        for msg in [
            "we should refactor the auth module",
            "let's build a graph editor for agents",
            "I want to build a workflow engine",
            "I'd like to redo the onboarding flow",
            "maybe we could add a plugin marketplace",
        ] {
            assert_eq!(
                classify_intent(msg),
                PrimeIntent::Brainstorming,
                "{msg:?} is soft-intent musing, not a task"
            );
        }
    }

    #[test]
    fn work_verbs_match_whole_words_not_substrings() {
        // The task-creation catch fires on a work verb only as a WHOLE WORD, so an
        // embedded verb no longer fabricates work (section 17.1). None of these
        // carry a command, a question opener, or a soft-intent lead-in, so they must
        // not classify as task creation off the embedded verb.
        for msg in [
            "the prefix is wrong",          // "fix" inside "prefix"
            "show me a preview",            // "review" inside "preview"
            "the building plan looks off",  // "build" inside "building"
            "it fixes the crash already",   // "fix" inside "fixes"
        ] {
            assert_ne!(
                classify_intent(msg),
                PrimeIntent::TaskCreation,
                "{msg:?} must not be read as task creation off an embedded verb"
            );
        }
        // A real whole-word verb still creates work.
        assert_eq!(
            classify_intent("please fix the login bug"),
            PrimeIntent::TaskCreation
        );
        assert_eq!(
            classify_intent("refactor the scheduler"),
            PrimeIntent::TaskCreation
        );
    }

    #[test]
    fn explicit_command_inside_a_question_still_acts() {
        // The conversation guard never blocks an explicit command, even when the
        // message is phrased as a question (section 10.5).
        assert_eq!(
            classify_intent("can you create a task to fix the login bug?"),
            PrimeIntent::TaskCreation
        );
        // Status / explanation / tool questions are classified before the guard, so
        // it never swallows them.
        assert_eq!(
            classify_intent("what is going on?"),
            PrimeIntent::StatusQuestion
        );
        assert_eq!(
            classify_intent("why did it fail?"),
            PrimeIntent::ExplanationRequest
        );
        assert_eq!(
            classify_intent("what tools can you use?"),
            PrimeIntent::ToolDiscovery
        );
    }

    #[test]
    fn brainstorm_candidate_strips_question_and_soft_intent_lead_ins() {
        // The recovered candidate (for the one-click "turn this into a task") drops
        // question and soft-intent lead-ins so the pre-fill names the work cleanly.
        assert_eq!(
            brainstorm_task_candidate("what's the best way to fix the flaky tests?"),
            Some("fix the flaky tests".to_string())
        );
        assert_eq!(
            brainstorm_task_candidate("we should refactor the auth module"),
            Some("refactor the auth module".to_string())
        );
        assert_eq!(
            brainstorm_task_candidate("I want to build a workflow engine"),
            Some("build a workflow engine".to_string())
        );
    }

    #[test]
    fn ideation_creating_a_task_still_starts_a_ready_run() {
        // End-to-end of the two regression anchors together: after the ideation
        // sentence creates nothing, an explicit "start it" still acts on the single
        // ready task.
        let mut s = empty_summary();
        s.queued = vec![brief("task_0007", "Run the tests", TaskStatus::Queued)];
        assert_eq!(
            classify_intent("start it"),
            PrimeIntent::RunStart,
            "explicit run control must survive the ideation guard"
        );
        let plan = decide("start it", &PrimeIntent::RunStart, &s);
        match plan {
            PrimePlan::Act {
                action: PrimeAction::StartRun { task_id },
                ..
            } => assert_eq!(task_id, "task_0007"),
            other => panic!("expected Act/StartRun, got {other:?}"),
        }
    }

    #[test]
    fn explanation_is_grounded_in_a_failed_task() {
        let mut s = empty_summary();
        s.tasks_failed = 1;
        s.recent = vec![brief("task_0003", "Deploy the worker", TaskStatus::Failed)];
        let plan = decide("why did it fail?", &PrimeIntent::ExplanationRequest, &s);
        match plan {
            PrimePlan::Reply { text } => {
                assert!(text.contains("task_0003"));
                assert!(text.contains("Deploy the worker"));
            }
            other => panic!("expected Reply, got {other:?}"),
        }
    }
}
