# Prime tool use — using installed plugins / tools / MCP from chat

> Spec refs: `docs/RELUX_MASTER_PLAN.md` §10.1 (Intent Layer — brain-mediated, keyword
> classifier is a fallback rail), §10.5 (Conversation Rules), §17.1 (Prime Must Be Smart
> And Grounded); `docs/mcp.md` ("Prime Agent Loop", "Invocation"); `docs/reference-driven-development.md`
> (keyword rules are fallback safety rails only — never the primary brain).

## The product promise

Installing or configuring a plugin is only useful if **Prime can actually use it from chat**.
A user should be able to:

1. Install a plugin and turn one of its capabilities into a runnable tool (a governed
   command tool, an HTTP-loopback tool, or a registered MCP server's tool — see
   `docs/mcp.md` and the Plugins page), then
2. Ask Prime, in plain conversation, to use it — *"summarise this repo with the readme tool"*,
   *"what tools do you have?"*, *"run the status tool"* — and have Prime **discover the tool,
   decide whether it is relevant, route through approval if required, invoke it through the
   governed path, and answer with the result.**

This document describes the path that makes that real, and exactly where the safety gates sit.

## What Prime can see

Prime is handed the **inventory of tools it can actually run** before it decides anything —
the same way Hermes/Codex are handed their tool list. The inventory is the runnable set only:

- installed plugin tools that are `ready` (a built-in handler, an enabled HTTP-loopback
  runtime, or an **enabled governed command tool** backs them), and
- installed tools that are `needs_approval` (declared approval-gated, or an unclassified MCP
  tool — fail-closed to "needs approval"), and
- the **live tools of every enabled MCP server** (discovered with a bounded `tools/list`).

A tool Prime *cannot* run (disabled runtime, no runtime configured, missing permission) is
**never** offered to the brain — it is surfaced to the operator on the Plugins page instead.
This is honest by construction: the brain is never told it can run something the kernel would
refuse.

Two surfaces expose this inventory:

- **In the decision prompt.** `render_tool_inventory_with_mcp` (in `prime_decision.rs`) renders
  the runnable installed tools **and the live tool names of every enabled MCP server** into the
  unified decision prompt, so the brain can recognise both *"use the readme summarizer"* and a
  natural-language MCP request like *"search my notes"* as a tool request and classify the turn
  as `tool_invocation`. The live MCP tool names come from a **bounded, off-lock, TTL-cached**
  `tools/list` run *before* the decision (`decision_time_mcp_catalog` in `server.rs`): a slow or
  unreachable server can never hang chat (an overall timeout falls back to the last cached
  catalog, or to naming the servers only), and a server whose discovery fails is listed as
  *unavailable* — never given a fabricated tool. The **same** discovered catalog is reused to
  ground the agent loop below, so a tool turn pays at most one bounded discovery (usually a cache
  hit). With no enabled MCP server (or a discovery that yields nothing), the prompt is
  byte-for-byte the prior installed-tools-only form.
- **In the dashboard / over HTTP.** `GET /v1/relux/prime/tools` returns the exact runnable
  catalog the agent loop offers (`KernelState::prime_agent_catalog`), including live MCP tools.
  The Prime page renders it as the collapsible **"Tools Prime can use"** panel.

## How a tool request flows

```
user chat message
      │
      ▼
unified brain decision (off-lock)  ── sees the tool inventory in its prompt
      │   proposes classification.intent (e.g. "tool_invocation")
      ▼
reconcile_intent  (fail-closed gate — guarded chat can NEVER be promoted to a
      │            sensitive intent like tool_invocation; §10.1/§10.5/§17.1)
      ▼
effective_intent == ToolInvocation ?
      │ yes                                  │ no
      ▼                                      ▼
Prime Agent Loop (drive_prime_agent_loop)    normal conversational turn
  • build the live catalog (installed + MCP)   (chat stays chat — nothing runs)
  • brain picks a tool from the catalog
  • execute through the UNCHANGED gate:
        prime_agent_step → prime_invoke_tool
  • observe the real result, continue / answer
```

The loop entry is **brain-driven**: the brain proposes the intent and `reconcile_intent`
decides, with the deterministic keyword classifier (`classify_intent`) as the **fallback rail**
only — the rule from `docs/reference-driven-development.md`. A natural-language tool request the
keyword rail alone would miss (*"summarise this with the readme tool"*) now enters the governed
path because the brain recognised it; casual chat, a greeting, frustration, a vague idea, or a
question *about* a tool still never enters it, because `reconcile_intent` treats
`tool_invocation` as **sensitive** and refuses to promote guarded chat.

## The safety gates (unchanged — there is no second security model)

Every tool execution flows through the single existing chokepoint
(`prime_invoke_tool` → `invoke_tool`), in this order:

1. **Permission.** The acting agent must hold the tool's required permission, else an honest refusal.
2. **Approval / risk.** A tool whose declared approval blocks a direct invocation (or an
   unclassified MCP tool) is `needs_approval`:
   - if a standing **allow-always grant** already authorises the exact
     `(agent, plugin, tool, permission, risk)`, it runs directly (the grant fast path), else
   - Prime **stages a per-call approval card** (`PrimeToolApprovalRequest`) and **pauses** —
     nothing runs. The card shows the `<plugin>/<tool>` label, source (`mcp`/`plugin`), risk,
     a **secret-redacted** args preview, the required permission, and the reason. The chat
     `ApprovalCard` (and the Approvals page) offer *Approve & run* / *Allow always* / *Deny*
     wired to the existing `/v1/relux/approvals/*` routes only.
3. **Execute + observe.** A `ready` tool (or one covered by a grant) runs, and its **shaped,
   redacted** result is folded back into Prime's reply and recorded as a `tool_trace` chip /
   `tool_output`. A failed run is an honest `ok:false` observation with the error — never a
   fabricated success. Raw CLI / MCP envelopes never reach the user.

## Continuous approval (approve → run → continue)

When the agent loop pauses on a gated tool, it persists a **resumable continuation** whose
pending-approval marker names the staged approval. The flow is then continuous, with no second
typed prompt required from the operator:

1. The chat renders the `ApprovalCard` (and the same handle drives the Board-Oversight / Inbox
   surfaces). Nothing has run.
2. The operator clicks **Approve & run** (or **Allow always**). That drives the **existing**
   `/v1/relux/approvals/*` routes: `decide(approved)` then `execute`. `execute` runs the bound
   call **once** through `execute_approved_tool_invocation`, which then calls
   `fold_approved_into_continuation` — appending the real, shaped result to the paused
   continuation, **clearing** its pending-approval marker, and marking the call completed.
3. The dashboard then **automatically resumes** the loop (`POST /v1/relux/prime/agent/continue`
   with the continuation token). Because the result is already folded in and the marker cleared,
   the resumed loop proceeds **with the tool result in context** and Prime summarises / continues
   — it never re-runs the completed call (the loop skips it by signature), and if it needs
   another gated tool it pauses again with a fresh card.

This is safe and adds **no new authority**: every step is an existing route behind the unchanged
gates; **Deny** drops the continuation (`drop_continuation_for_approval`) so a refused tool can
never resume; and when there is no continuation (e.g. a non-loop approval, or a Local brain that
has no agent loop) the inline tool result is the answer — the chat is **never** a dead-end.

## Asking for missing input

If the user names a tool but not its arguments, the brain asks for them (a clarifying turn),
or the per-tool arg reading fails closed with an honest message — Prime never fabricates an
argument.

## Powering Prime: brain setup & the safe probe

> Spec refs: `docs/RELUX_MASTER_PLAN.md` §14 (recommended first adapter: Claude CLI / Codex CLI /
> OpenRouter; "the dashboard shows the state") and §10.1 (the LLM brain is the PRIMARY surface;
> the deterministic classifier / Local brain is the fallback rail). Adapter probe contract:
> `docs/relix-agent-adapters.md` §2 ("Probe/test environment — is the agent installed?").

Everything above assumes Prime has a **real** brain. The setup surface for that lives on
**Health → Prime Brain / AI Runtime** (the `PrimeBrainPanel`), and is product-grade by design:

- **Recommended vs fallback is explicit.** Claude CLI, Codex CLI, and OpenRouter are tagged
  *recommended* (a real conversational Prime). **Local** is tagged *fallback / test* — grounded,
  always available, but **not** the product chat path; it is used automatically only when no real
  brain is set up. When Prime is on the Local fallback, its chat banner says so and links straight
  to the setup panel (*"Set up a real brain →"*).
- **One-click enable + select.** For a CLI brain, *"Use Claude/Codex for Prime"* enables the
  adapter and selects it as the brain in a single action; the panel shows live adapter state
  (installed / on-PATH / enabled) and the exact install + sign-in step when the binary is missing.
- **Test before you trust it.** Each brain has a **Test** button → `POST /v1/relux/ai/probe`
  (`{ brain?, ... }`; omit `brain` to probe the brain Prime currently resolves to). The probe is
  **safe and bounded** and reuses the exact same spawn contract as an assigned run:
  - **CLI brains** run `<bin> --version` only when the adapter is enabled and on PATH — argv-only,
    empty stdin closed immediately, a short timeout, an output cap, secret redaction, and **no
    bypass/danger flag** (the only argument is the read-only `--version`). It proves the binary is
    installed and runnable; **sign-in is verified on the first real chat turn**, not by the probe.
  - **OpenRouter** reports whether its key resolves **without** sending a billable request (a pure
    configuration check), and names a missing secret reference so the next step is obvious.
  - **Local** is always `ready`.
  The result is a clear status — `ready` / `disabled` / `missing_binary` / `not_configured` /
  `missing_key` / `failed` — each with a secret-free `detail` and the next step. The probe never
  runs an agent turn and never crosses a permission boundary.
- **Prove a real chat turn — the live probe.** The quick probe proves *availability*; it cannot
  prove Prime can complete a chat turn (for CLI brains, sign-in is only confirmed on the first real
  turn, and OpenRouter is never contacted). The **Test live chat** button → `POST
  /v1/relux/ai/probe/live` (`{ brain?, ... }`) closes that gap by sending **one tiny, bounded
  prompt** through the selected/resolved brain and classifying the result. It is **explicit-only**:
  the dashboard never calls it on load, the button copy states it **may use the real provider / CLI
  and may incur provider usage**, and it is disabled (with the reason shown) when the brain is not
  set up yet. It is a setup diagnostic — it creates **no task and no run** and grants no broader
  permission.
  - **CLI brains** run the **same safe adapter invocation a real turn uses** (`build_adapter_args`,
    so **no bypass/danger flag**), with the tiny prompt on stdin, a bounded timeout (60 s), an
    output cap, and secret redaction. The reply is parsed via `parse_adapter_result` and a redacted,
    truncated `sample` is returned. A not-logged-in / auth error is detected and reported as
    `auth_failed` with the next step; a clean exit with no readable reply is an honest `failed`,
    never a fake success.
  - **OpenRouter** sends **one** small, low-token (billable) request through the existing client
    path; the key travels only in the `Authorization` header and never appears in the result. A
    401/403 maps to `auth_failed`, a transport timeout to `timeout`. When no usable key resolves (or
    the LLM path is disabled) it returns **without** making any request.
  - **Local** answers deterministically and is labelled a fallback/test brain — no provider is
    contacted and no usage is incurred.
  The live result is a clear status — `ready` / `not_configured` / `missing_key` / `auth_failed` /
  `timeout` / `failed` / `unsupported` — with a secret-free `detail`, a `duration_ms`, and (on
  success) a redacted `sample` of the real reply. Reference-driven (`reference/hermes-agent-main/
  agent/auxiliary_client.py`): Hermes validates a provider with a real minimal completion and
  classifies the failure (auth / payment / timeout) rather than trusting a config check — the live
  probe mirrors that "prove it with a real call, then classify" shape.

Keys are never stored or shown in plaintext: OpenRouter's key is a write-only secret reference
(only the secret *name* and a redacted preview cross the wire); Claude/Codex authenticate through
their own local CLI login, so Relux stores no key for them at all.

## What this slice changed

- The decision prompt now carries the runnable-tool inventory (so the brain can choose a tool).
- The agent-loop entry is **brain-driven** (reconciled intent), with the keyword classifier as
  the fallback rail — previously it was keyword-only.
- The live MCP `tools/list` discovery is no longer gated on the literal `"mcp:"` token in the
  message; it runs whenever the turn is a plausible tool turn and an MCP server is enabled, so a
  natural-language request can use an MCP tool.
- New `GET /v1/relux/prime/tools` + the dashboard "Tools Prime can use" panel.

### Later additions (continuous tool use)

- **Live MCP tool names in the *first* decision.** A bounded, off-lock, TTL-cached `tools/list`
  (`decision_time_mcp_catalog`) runs before the decision and feeds `render_tool_inventory_with_mcp`,
  so the brain's first classification sees the actual MCP tool names — not just that a server
  exists. It is non-blocking (overall timeout → cached / servers-only fallback) and fail-closed
  (an unreachable server is listed *unavailable*), and the same catalog grounds the agent loop
  (one discovery per turn, usually a cache hit).
- **Continuous approval.** After the operator approves a staged Prime tool call, the dashboard
  auto-resumes the paused loop so Prime continues with the real result without a second typed
  prompt (see *Continuous approval* above). The chat never dead-ends on an approval.

## Importing a plugin from GitHub (from chat)

Prime recognizes a GitHub plugin-import request and stages the **safe manifestless
import behind a human confirmation** — it never turns it into a generic work task and
never runs the repo's code.

What works from chat:

- `install nousresearch/hermes-agent as a plugin`
- `import https://github.com/owner/repo as plugin`
- `clone owner/repo and import it as a plugin`

The flow:

1. **Parse.** `crate::prime_plugin_install::parse_github_plugin_install` turns the
   message into a **canonical, credential-free** `https://github.com/<owner>/<repo>`
   (anything before `github.com/`, including `user:token@`, is dropped; a non-GitHub
   host yields no match). The owner/repo shorthand mirrors the dashboard's
   `normalizeGithubUrl`, so chat and the Plugins page agree on what `owner/repo` means.
2. **Propose (confirmation-gated).** The turn classifies as `PluginInstallation`, routes
   to `PrimeAction::InstallPluginFromGithub { repo_url, plugin_id }`, and comes back as a
   `RiskLevel::High` **proposal awaiting approval** — exactly like every other risky
   Prime action. A logged approval is the governance/audit record. A GitHub-import
   request with **no parseable repo** ("import a plugin from github") asks *which repo*
   instead of proposing an unspecified install.
3. **Confirm (one backend chokepoint).** The chat renders a `PluginInstallCard` showing
   the source, the proposed local id, the destination, and the explicit **"no code from
   the repository runs on import"** guarantee. Confirm posts to the **single
   backend-governed action route** `POST /v1/relux/prime/actions/install-plugin` instead
   of chaining generic routes client-side. The kernel:
   - **re-validates server-side** — it re-canonicalizes the submitted `repo_url` through
     the same parser (`canonicalize_github_repo_url`), so a tampered URL (swapped host,
     embedded credential, extra path) is rebuilt or rejected, and a client-echoed
     `plugin_id` that does not match the id re-derived from the repo is a `400`. No
     client-only field is trusted.
   - runs the **existing** manifestless installer (`install_from_github`, which re-runs
     the authoritative `validate_github_url`) and the **same** read-only candidate scan
     (`detect_hints` + `detect_candidates`) **internally** — no duplicated shell code.
   - closes the logged governance approval (best-effort; the install is the authoritative
     gate) and returns **one structured envelope**.
   This makes headless/API Prime usable and gives a single auditable execution path.
   Reference: Hermes `hermes_cli/plugins_cmd.py::cmd_install` — one entry resolves the
   URL, clones, and returns a structured result; enable/configure is a separate step
   (install ≠ auto-enable).
4. **Result.** The structured response carries the installed plugin record (id + status),
   the canonical `source`, the `generated` (scaffolded wrapper vs real manifest) flag, the
   detected capability `candidates` + `candidate_count`, honest `next_actions`, the
   `no_code_executed` guarantee, and the closed `approval_id`. The card shows the plugin
   id + status, the candidate count, the next-action list, and **Configure / Open
   Plugins** links. A repo with no `relux-plugin.json` lands as a metadata-only
   (scaffolded, disabled) wrapper that runs nothing until the operator configures a
   tool/runtime.

**Safety:** the import clones metadata, runs no repo code, and grants no new authority;
tools stay disabled until configured through the unchanged plugin/tool paths. Casual
musing ("what if I made a plugin system?") stays a conversation — the conversation guard
routes a question to Brainstorming before the install check.

**Reference-driven** (`docs/reference-driven-development.md`): this mirrors Hermes
`reference/hermes-agent-main/hermes_cli/plugins_cmd.py` — `_resolve_git_url`
(owner/repo shorthand or full URL → cloneable URL, GitHub default) and `cmd_install`
(clone `--depth 1`, validate, then a separate **confirm/enable** step: install ≠
auto-enable) — and openclaw's single-classifier confirmation discipline
(`reference/openclaw-main/src/acp/approval-classifier.ts`): one deterministic function
decides, and the stateful path is always confirmation-gated, never auto-run.

## Configuring a detected capability (from chat)

Importing a plugin (above) *detects* capability candidates but configures nothing — a
repo with no `relux-plugin.json` lands as a metadata-only wrapper whose detected MCP
server / scripts are still inert. Prime can now **guide the next step**: activate one
detected candidate through the **existing governed configuration paths**, behind a human
confirmation, without the operator leaving chat for the Plugins page.

What works from chat:

- `configure the first candidate`
- `enable the MCP server from hermes-agent`
- `turn that script into a tool`

The flow:

1. **Parse.** The deterministic classifier routes an explicit activation request
   (`configure`/`activate`/`enable`/`set up`/`register` + a candidate / MCP-server /
   command-tool cue) to `PluginConfiguration`. `crate::prime_candidate_config::parse_candidate_config_request`
   turns the message into a **plugin selector** (a fuzzy name, or empty when none is
   named) + a **candidate selector** (`mcp` / `command` / `first`). Casual talk
   ("should I configure my editor?") and questions stay conversational — the conversation
   guard routes a question to Brainstorming before this check.
2. **Propose (confirmation-gated).** The turn routes to
   `PrimeAction::ConfigurePluginCandidate { plugin_id, candidate_id }` and comes back as a
   `RiskLevel::High` **proposal awaiting approval** — a logged approval is the
   governance/audit record. Both selectors are advisory: the backend re-resolves and
   re-validates them, so nothing here is trusted as a concrete command.
3. **Confirm (one backend chokepoint).** The chat renders a `ConfigureCandidateCard` (and
   the import result renders a **Configure with Prime** button per detected candidate)
   stating what will be activated, where, and the explicit **"no code from the source
   runs"** guarantee. Confirm posts to the **single backend-governed action route**
   `POST /v1/relux/prime/actions/configure-candidate`. The kernel:
   - **re-reads the candidates server-side** from the plugin's install directory (the same
     read-only `detect_hints` + `detect_candidates` scan as `/hints`), then **re-resolves**
     the selector against that fresh list (an exact id wins; otherwise `mcp` / `command` /
     `first`). A tampered command in the request body can never reach a spawn, because the
     spawn recipe is rebuilt from the server-side scan, not the request.
   - resolves the **target plugin**: an exact installed id wins (the button path);
     otherwise it picks the unique plugin that has an activatable candidate, or the one
     whose id/name matches a named selector. Ambiguity ("more than one plugin has
     candidates") is an honest `400`, never a silent guess.
   - **activates through the existing governed path** — no duplicated unsafe code:
     - an `mcp_register` candidate is registered on the **unchanged MCP registry**
       (`register_mcp_server` / `register_mcp_stdio_server`, which re-validate the
       loopback/argv contract). `env` is **not** pre-filled — a managed-stdio server's
       secrets are mapped separately on the MCP page, never carried in this request.
     - a `command_tool` candidate's pre-filled argv draft is rendered into the exact JSON
       the **unchanged** `parse_command_tool_input` validator accepts and stored through
       `configure_command_tool` (argv-only, no shell, confined cwd, approval always
       Required).
   - closes the logged governance approval (best-effort) and returns **one structured
     envelope**.
4. **Result.** The structured response carries the `plugin_id` / `plugin_name`, the
   activated candidate's `kind` + `activation`, the registered `mcp_server` **or** the
   updated `plugin` record, the new `tool_name`, the honest `next_step` (**"ask me to use
   it"** — the tool stays gated until invoked), and the `no_code_executed` guarantee. A
   command tool is invokable through the same gated chat path (`prime_invoke_tool`).
5. **Guided post-activation discovery (MCP only).** Immediately after it registers an
   `mcp_register` candidate, the route runs ONE bounded `tools/list` probe against the
   freshly-registered server — **off the kernel lock** (a loopback dial or a
   spawn-per-operation managed-stdio child; `relux_kernel::discover_and_classify_mcp_tools`)
   — so the user sees what Prime can now use without driving a separate manual Discover.
   The result rides back on the response as `mcp_discovery`:
   - **Reachable:** the discovered tools (each carrying its fail-closed classification —
     unclassified ⇒ `needs_approval`), a `tool_count` / `gated_count` summary, and a
     `guidance` line naming a few tools and the gated split. The chat card lists each tool
     with a **gated / runnable** chip.
   - **Unreachable / no tools:** `reachable: false` with **actionable** guidance (map the
     server's secrets `ENV_VAR=secret_name`, then **Discover**, for a managed-stdio server
     with env placeholders; **Start it on the MCP page** otherwise) plus the sanitized,
     value-free `error` reason. **No fabricated tools.**
   - **Guided secret/env setup (when the source declared env vars).** The same response
     also carries a value-free `setup` requirement view (`relux_core::McpServerSetup`):
     which env vars the server needs, which already map to a *present* stored secret, and
     what is still `missing`. The chat renders an inline **"Set up the secrets this server
     needs"** form (`McpEnvSetupForm`) so the user can supply a value (stored write-only) or
     map an existing secret, then re-discover — **without** hand-editing config. The form
     posts to the single governed `POST /v1/relux/mcp/servers/:id/env-setup` chokepoint,
     which stores + maps the secrets through the existing write-only store + managed-stdio
     `env` contract and returns the recomputed (still value-free) setup + a fresh discovery.
     See `docs/mcp.md` "Guided env/secret setup". **No secret value ever crosses the wire;
     setup runs no source code; the resulting tools stay gated.**
   This step is best-effort: the server is already registered, so a probe failure becomes
   guidance, **never a failed activation**. Discovery **lists** tools only — it never calls
   one, and never silently marks a discovered tool low-risk. This mirrors Hermes
   `cmd_mcp_add`'s discovery-first flow (`reference/hermes-agent-main/hermes_cli/mcp_config.py`):
   connect, list tools, and on failure surface a clear "fix it, then test" path.

**Safety:** activation registers metadata/recipe only — it runs no code from the source,
grants no new authority, and the resulting MCP tool / command tool stays gated (needs
approval) until invoked. An honest `manual` candidate (a CLI Relux has no runtime for)
has no one-click path and points at the Plugins page instead of faking a "ready" state.

**Reference-driven** (`docs/reference-driven-development.md`): this mirrors Hermes
`reference/hermes-agent-main/hermes_cli/mcp_config.py` — `cmd_mcp_add` keys a
`{command, args, env}` (or `{url}`) server by name, and **configuring a server is a
separate step from running it** (configure ≠ run) — and openclaw's
`extensions/acpx/src/config-schema.ts` (`McpServerConfig = {command, args, env}`), the
same server shape Relux rebuilds from the candidate's proposal before handing it to the
existing registry validation.

## Configuring a command tool for a source-only plugin

> Spec refs: `docs/RELUX_MASTER_PLAN.md` §8.2 (Command Tools), §10.2 (Action Layer),
> §10.3 (Approval Rules); `docs/mcp.md` "Importing a repository as a plugin".

The *detected-candidate* path above only fires when the import inferred a runnable
entrypoint. A **source-only** plugin — an arbitrary repo with no `relux-plugin.json`,
no declared MCP server / `bin` / console-script / Cargo binary — yields an honest
`manual` candidate with **nothing to one-click**, and Relux refuses to *guess* a command
from repo content. That used to be a dead-end: the operator had to hand-edit JSON to make
the plugin usable. This is the bridge that closes it — the operator (or Prime, when the
user names the command) defines a governed command tool through the **existing**
command-tool path, with **no new authority** and **no manifest editing**.

What works:

- **On the Plugins page.** A non-bundled plugin's **Configure tools** panel now has an
  **"Add a command tool"** section (`apps/dashboard/src/pages/Plugins.tsx`
  `AddCommandToolSection`). The operator names a safe argv recipe — tool name, **program
  (argv[0])**, **args (one per line)**, an optional **working dir** inside the install dir,
  a timeout, and a risk band — and submits to the **unchanged**
  `POST /v1/relux/plugins/:id/command-tools` route. Defining it runs nothing; the tool is
  always approval-gated.
- **From chat.** *"configure this repo as a tool that runs npm test"* / *"use npm test
  from this plugin"* / *"make a command tool that runs cargo build for <plugin>"*. The
  deterministic classifier (`crate::prime_command_tool_config::parse_command_tool_config_request`
  — a **fallback rail only**, `docs/reference-driven-development.md`) extracts the plugin
  selector + the argv recipe **only when the user named a concrete command** (a bare
  "run the tests", a pronoun, or an article is refused, never fabricated). Prime stages it
  as a `RiskLevel::High` `PrimeAction::ConfigureCommandTool` **proposal awaiting
  approval** — never an unrelated task. The chat renders a `ConfigureCommandToolCard` that
  **pre-fills the reviewable fields** (the operator edits program/args/name/cwd before
  confirming, with the same client-side argv pre-check the Plugins form uses).

The flow (one backend chokepoint):

1. **Propose (confirmation-gated).** A from-scratch command-tool request routes to
   `PluginConfiguration` and proposes `ConfigureCommandTool { plugin_id, tool_name,
   program, args, cwd }` — a logged approval is the governance/audit record. The fields are
   advisory; the route re-validates everything.
2. **Confirm (`POST /v1/relux/prime/actions/configure-command-tool`).** A session-protected
   route that re-resolves the plugin server-side (an exact installed id wins; otherwise a
   unique fuzzy name match — ambiguity / no-match is an honest `400`, **never a silent
   guess**), then builds the **exact JSON** the **unchanged** `parse_command_tool_input`
   validator accepts and stores it through `configure_command_tool` — the **same** governed
   path a detected candidate uses: **argv-only, no shell, no danger flag, confined `cwd`,
   approval always Required.** A bad recipe (shell metacharacter, danger flag, `..` cwd
   traversal, missing program) is a clean `400` that never touches the store; a bundled
   plugin is refused. It closes the logged approval (best-effort) and returns **one
   structured envelope**.
3. **Result.** The envelope carries the `plugin_id` / `plugin_name`, the `tool_name`, the
   derived `permission` (`tool:<plugin>:<verb>`), `gated: true`, the honest `next_step`
   (**"ask me to use it"**), `no_code_executed: true`, and `catalog_refresh: true` so the
   dashboard re-pulls `GET /v1/relux/prime/tools` and the new tool shows up in **"Tools
   Prime can use"** — gated until invoked.

**Safety:** configuration stores an argv **recipe** only — it runs no code from the source,
grants no new authority, and the resulting command tool stays gated (needs approval) until
invoked, where it runs through the **single** `prime_invoke_tool` → `invoke_tool` gate
(permission → approval/grant → execute argv-only, confined cwd, bounded + secret-redacted
output, hard timeout, audited). A command tool carries **argv only** — never a secret
value — so the config and the result envelope never store or echo a secret.

**Reference-driven** (`docs/reference-driven-development.md`): this mirrors Hermes
`reference/hermes-agent-main/hermes_cli/mcp_config.py` (`cmd_mcp_add` — key a
`{command,args}` entry by name; **configuring is a separate, confirmed step from running
it**) and openclaw's single-classifier confirmation discipline
(`reference/openclaw-main/src/acp/approval-classifier.ts`): one deterministic function
decides, and the stateful path is always confirmation-gated, never auto-run.

## Hiring an operative (from chat)

> Spec refs: `docs/RELUX_MASTER_PLAN.md` §6 (the canonical hire exchange), §7.3 (an agent
> has an adapter plugin + permissions), §7.5 (granting permissions is a reviewed action),
> §8.1 (the adapter catalog); reference: openclaw `src/agents/tools/common.ts`
> (`normalizeToolModelOverride` — a backend preference is honored only when it resolves).

Prime can **hire an operative** from chat: *"make a coding agent for this repo"*, *"hire a
research agent named researcher that uses Claude"*. The deterministic classifier
(`creates_an_operative`, a **fallback rail only**) reads a natural hire phrasing as
`AgentCreation`; a message that merely *references* an agent stays a task. Two things are
resolved honestly, and **nothing risky is auto-done**:

- **Adapter preference.** *"uses Claude"* / *"run codex on this"* / a verbatim adapter id is
  honored **only when that adapter plugin is installed** (`resolve_adapter_preference`
  against the live adapter roster). A named-but-uninstalled adapter falls back to the local
  adapter with an honest caveat — Prime never invents or enables an adapter. The operative is
  always created on a real, resolved adapter (default `relux-adapter-local-prime`).
- **Capability honesty.** *"that can read GitHub"* / *"…and run shell commands"* is **not**
  silently dropped and **not** granted on creation. The operative is created with **no
  permissions**; Prime names the scoped permission it would need and offers the grant as a
  **separate, approval-gated follow-up** (the unchanged `PermissionChange` path). No access
  is fabricated.

**In the dashboard** (`apps/dashboard/src/pages/Prime.tsx` `AgentCreatedCard`, built from the
pure `agentCreatedView` in `apps/dashboard/src/prime.ts`): a real hire turn renders a
**result card** — the new operative's name/id, the **adapter it runs on** (human brand +
raw id), any brain-shaped role/persona, and a clear **"View in Crew"** link plus a
**"Give it work"** assignment pre-fill. When the user asked for a sensitive capability the
card shows it as **needing setup** with a *"Grant &lt;X&gt; access to &lt;agent&gt;"* button
that pre-fills the approval-gated grant — clicking it can do nothing the user could not type,
and **nothing is granted until the approval is greenlit**. Casual ideation and a
duplicate-name refusal carry no `created_agent`, so they render as **normal chat**, never an
action card. The operative then shows up on **Crew** (`CrewMemberCard`) with its adapter
brand, status, Lead/reporting line, skills/persona, and least-privilege permissions — and the
card renders cleanly even when an optional field is missing.

## Coordinating multi-agent work (from chat)

> Spec refs: `docs/RELUX_MASTER_PLAN.md` §10.4 (Delegation Rules), §11.1 (Prime Chat),
> §17.1 (Prime must be smart and grounded), and the "Orchestration (First Multi-Agent
> Slice)" rollup. Code: `crates/relux-core/src/orchestration.rs` (the pure planner),
> `crates/relux-kernel/src/state.rs` (`prime_orchestrate`, the `OrchestrateGoal` turn),
> `apps/dashboard/src/orchestration.ts` + `pages/Prime.tsx` (`OrchestrationResultCard`).

Prime can **turn one explicit goal into assigned work across the crew** from chat:
*"orchestrate research the options, build a prototype, and write the docs"*, *"split this
work across researcher and coder"*. Only **explicit coordination phrasing** classifies as
an orchestration; a bare imperative still creates a single task, and **casual ideation
stays conversation** (a guarded "should we split this across a few agents?" gets a
clarifying question, not a fan-out — §10.5/§17.1). Nothing here bypasses the kernel.

What happens, honestly:

- **The deterministic planner owns the decomposition.** The goal is split into role-typed
  briefs, each grounded to a real roster agent by **id keyword or declared specialty
  skill** — so a conversational hire (`researcher`) and a manually-configured operative
  with an opaque id but a `research` skill both match. A role with **no specialist** falls
  back to Prime and is reported as a missing hire — access is never fabricated, and the
  width is the operator-configured policy limit (no hidden cap).
- **Creating the orchestration runs nothing.** One brief (task) per step is created and
  assigned, recorded as a durable `Orchestration`. No run starts, and **no paid CLI is
  spawned**, without an explicit start.
- **The chat returns a STRUCTURED result card**, not a wall of prose. The turn carries the
  record on `PrimeTurn.orchestration`, so the Prime chat renders the ordered briefs with
  their **assignee + role + outcome**, the distinct specialists work landed on, the roles
  still on Prime (with the planner's honest "hire one" notes), and a link to the **Work
  board**. The card commits nothing.
- **The next actions are one-click, governed, and explicit.** The turn attaches ordinary
  `suggested_actions`: **Run this orchestration** (the explicit start of the same governed,
  dependency-aware batch the panel/CLI drive — each brief still gates at run time through
  its assigned agent's adapter) and **Hire a `<role>` agent** for each unstaffed role
  (pre-filled, not auto-sent, so the operator confirms the adapter). Each button is just a
  user message routed through the same grounded turn — never a privileged path.

The full run controls (live progress, cancel, restart-honest resume) stay on the Prime
**Orchestration panel**; the **Work board** groups the briefs by goal; **Crew** shows each
operative's open/running brief counts. The chat card is the entry point that makes the
fan-out legible the moment Prime creates it.

## The verified install → use path (end-to-end)

This is the path a regression test pins end-to-end, route for route — the same sequence
the dashboard / Prime drive, with no private-helper shortcut:

1. **Install** — from GitHub (`POST /v1/relux/plugins/install-github` / the governed
   `…/prime/actions/install-plugin`), a local **ZIP** (`…/install-zip`), or a local
   **folder** (`…/install-dir`). A `relux-plugin.json` manifest is **optional**: a source
   without one lands as an honest **metadata-only wrapper** (`generated: true`) that runs
   nothing until configured.
2. **Detect** — the same read-only scan (`detect_hints` + `detect_candidates`, exposed by
   `GET /v1/relux/plugins/:id/hints` and folded into the GitHub install envelope) surfaces
   capability **candidates** with honest `next_steps`: an `mcp_register` candidate (a
   one-click governed MCP registration) or a `command_tool` candidate (a pre-filled,
   reviewable argv draft). Detection **runs nothing**.
3. **Configure** — Prime's governed backend action `POST
   /v1/relux/prime/actions/configure-candidate` re-reads the candidates server-side and
   activates the selected one through the **unchanged** governed path (the MCP registry, or
   the command-tool validator/store). `no_code_executed: true` — activation registers
   metadata/recipe only.
4. **See it** — the new tool appears in the **exact runnable catalog Prime is handed**
   (`GET /v1/relux/prime/tools` / the decision-prompt inventory), marked `gated` (needs
   approval) until invoked.
5. **Use it** — invocation flows only through the single gate. A gated tool is **refused**
   (`409`) until a standing **allow-always grant** (`POST /v1/relux/grants`) or a per-call
   approval authorises the exact `(agent, plugin, tool, permission, risk)`. With one, the
   tool runs **argv-only** (no shell, confined cwd, bounded + secret-redacted output, hard
   timeout) and the result is **recorded in the audit log** against Prime, so Prime can
   summarise/see it.

Regression coverage: `crates/relux-kernel/src/server.rs::install_configure_then_prime_can_use_the_governed_command_tool`
drives steps 1–5 over the real routes against a local non-echo fixture (a tiny npm CLI),
and `scripts/smoke-plugin-install-to-prime-use.ps1` is an optional, manual smoke that runs
it (and the related configure-candidate tests) — local fixture only, no clone, no remote
code, no real brain, not required by CI.

**Honest limitations.** A **source-only** plugin with no detected runnable entrypoint
(no `relux-plugin.json`, no declared MCP server / `bin` / console-script / Cargo binary)
yields an honest `manual` candidate with **no auto-detected activation** — Relux never
infers a command from repo content. But it is **no longer a dead-end**: the operator (or
Prime, when the user names the command) can define a governed command tool through the
bridge above (**"Configuring a command tool for a source-only plugin"**) — argv-only,
gated, no manifest editing. Detection still never *guesses* a command; the operator
supplies it, and nothing is ever claimed `ready` until it is actually configured and
invoked.

See `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` for the lifted constraints and `docs/mcp.md` for the
agent loop and MCP transports.
