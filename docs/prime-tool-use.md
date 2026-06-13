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

See `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` for the lifted constraints and `docs/mcp.md` for the
agent loop and MCP transports.
