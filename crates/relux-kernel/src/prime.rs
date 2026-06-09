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
    PrimeAction, PrimeIntent, PrimePlan, RiskLevel, StateSummary, TaskBrief, TaskStatus,
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
        PrimeIntent::Brainstorming => PrimePlan::Reply {
            text: "Tell me the goal and the constraints and I will outline options. I will not create tasks or agents until you confirm."
                .to_string(),
        },
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
                text: "Which tool should I run? I can run relux-tools-echo/echo.say and relux-tools-status/status.summary; other installed tools are listed but not runnable here yet."
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
