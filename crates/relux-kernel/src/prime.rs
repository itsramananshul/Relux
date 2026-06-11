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
    plan_orchestration, PrimeAction, PrimeIntent, PrimePlan, RiskLevel, StateSummary, TaskBrief,
    TaskStatus,
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
    if has(&["echo.say", "status.summary"])
        || (first_word == "echo")
        || (invoke_verb && has(&[" tool", "echo", "status tool"]))
        || (invoke_verb && m.contains("relux-tools-"))
        || has(&["use the echo", "use the status", "the echo tool", "the status tool"])
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
    if has(&[
        "create a task",
        "make a task",
        "new task",
        "add a task",
        "build",
        "fix",
        "implement",
        "investigate",
        "summarize",
        "review",
        "refactor",
        "write ",
        "set up",
        "add ",
        "create ",
        "update the",
        "draft ",
    ]) || CREATION_VERBS.contains(&first_word.as_str())
    {
        return PrimeIntent::TaskCreation;
    }
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
        PrimeIntent::Greeting => PrimePlan::Reply {
            text: format!("I am here. {} What do you want to work on?", headline(summary)),
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
        PrimeIntent::RunStart => match summary.queued.as_slice() {
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
            let parsed_agent_id = extract_agent_id_from_assignment(message);

            match (parsed_task_id, parsed_agent_id) {
                (Some(task_id_str), Some(agent_id_str)) => {
                    // Validate task and agent existence using summary
                    let task_exists = summary.all_task_ids.contains(&task_id_str);
                    let agent_exists = summary.all_agent_ids.contains(&agent_id_str);

                    if !task_exists {
                        PrimePlan::Reply {
                            text: format!("Task with ID '{}' does not exist. Please provide a valid task ID.", task_id_str),
                        }
                    } else if !agent_exists {
                        PrimePlan::Reply {
                            text: format!("Agent with ID '{}' does not exist. Please provide a valid agent name.", agent_id_str),
                        }
                    } else {
                        PrimePlan::Act {
                            action: PrimeAction::AssignTask {
                                task_id: task_id_str.clone(),
                                agent_id: agent_id_str.clone(),
                            },
                            text: format!("Assigning task {} to agent {}.", task_id_str, agent_id_str),
                        }
                    }
                }
                (None, Some(_)) => PrimePlan::Clarify {
                    text: "I couldn't find a task ID in your request. Please specify the task to assign.".to_string(),
                },
                (Some(_), None) => PrimePlan::Clarify {
                    text: "I couldn't find an agent name in your request. Please specify which agent to assign the task to.".to_string(),
                },
                (None, None) => PrimePlan::Clarify {
                    text: "I need both a task ID and an agent name to assign a task. Please rephrase your request.".to_string(),
                },
            }
        },
        PrimeIntent::TaskUpdate => PrimePlan::Clarify {
            text: "Which task should I update, and what should change?".to_string(),
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
                    text: "That reads like a single piece of work, not something to split across agents. Give me the distinct steps - e.g. \"research the options, implement a prototype, and write the docs\" - or say \"create a task to ...\" and I'll make one brief."
                        .to_string(),
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
        PrimeIntent::DirectAnswer => PrimePlan::Reply {
            text: "I can inspect state, create tasks, start runs, explain blockers, and request approvals for risky actions. What would you like to do?"
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
fn count_phrase(n: usize, noun: &str) -> String {
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
    ];
    COMMANDS.iter().any(|p| m.contains(p))
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
        "can we talk about",
        "could we talk about",
        "let's talk about",
        "lets talk about",
        "let's discuss",
        "lets discuss",
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
fn parse_tool_request(message: &str) -> Option<(String, String, String)> {
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

fn extract_task_id(message: &str) -> Option<String> {
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
    fn greeting_is_grounded_and_never_a_plan() {
        let plan = decide("hey", &PrimeIntent::Greeting, &empty_summary());
        match plan {
            PrimePlan::Reply { text } => {
                assert!(text.contains("I am here"));
                assert!(text.contains("idle"));
            }
            other => panic!("greeting must be a Reply, got {other:?}"),
        }
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
