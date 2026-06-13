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

- **In the decision prompt.** `render_tool_inventory` (in `prime_decision.rs`) renders the
  runnable installed tools (+ the names of enabled MCP servers) into the unified decision
  prompt, so the brain can recognise *"use the readme summarizer"* as a tool request and
  classify the turn as `tool_invocation`. The live MCP tool **names** are not enumerated in
  this prompt (that needs an off-lock `tools/list`); the agent loop below has the full live
  catalog when it actually picks a tool.
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

## Asking for missing input

If the user names a tool but not its arguments, the brain asks for them (a clarifying turn),
or the per-tool arg reading fails closed with an honest message — Prime never fabricates an
argument.

## What this slice changed

- The decision prompt now carries the runnable-tool inventory (so the brain can choose a tool).
- The agent-loop entry is **brain-driven** (reconciled intent), with the keyword classifier as
  the fallback rail — previously it was keyword-only.
- The live MCP `tools/list` discovery is no longer gated on the literal `"mcp:"` token in the
  message; it runs whenever the turn is a plausible tool turn and an MCP server is enabled, so a
  natural-language request can use an MCP tool.
- New `GET /v1/relux/prime/tools` + the dashboard "Tools Prime can use" panel.

See `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` for the lifted constraints and `docs/mcp.md` for the
agent loop and MCP transports.
