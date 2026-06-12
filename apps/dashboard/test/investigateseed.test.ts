// Unit tests for the "Investigate with Prime" seed builder + handoff
// (src/investigateseed.ts) — the §3.3b "Investigate → chat companion pre-loaded
// with the diagnosis" choice (docs/relix-dashboard-design.md §6.10).
//
// Pure logic only — no React/DOM — so it runs under `node --test
// --experimental-strip-types`. The card render + navigation is covered by
// work-investigate-render.test.mjs; this pins the seed CONTENT (right fields,
// bounded, redacted, read-only framing) and the consume-once storage semantics.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  buildInvestigationSeed,
  runInvestigationInput,
  taskInvestigationInput,
  stashInvestigationSeed,
  consumeInvestigationSeed,
  redactSecrets,
  INVESTIGATION_SEED_KEY,
  MAX_LOG_TAIL_CHARS,
  type SeedStorage,
} from "../src/investigateseed.ts";
import { assessRunRecovery, assessTaskRecovery } from "../src/recovery.ts";
import type { ReluxRunDetail, ReluxTask } from "../src/api.ts";

function run(over: Partial<ReluxRunDetail> = {}): ReluxRunDetail {
  return {
    id: "run_0007",
    task_id: "task_0007",
    agent_id: "scout",
    adapter_plugin: "claude-cli",
    status: "failed",
    ...over,
  } as ReluxRunDetail;
}

function task(over: Partial<ReluxTask> = {}): ReluxTask {
  return {
    id: "task_0007",
    title: "ship the recovery card",
    input: {},
    status: "blocked",
    priority: 5,
    created_by: "operator",
    assigned_agent: "scout",
    namespace_id: "ns_root",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  } as ReluxTask;
}

// A simple in-memory storage matching the SeedStorage surface.
function memStorage(): SeedStorage & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k) => (map.has(k) ? map.get(k)! : null),
    setItem: (k, v) => void map.set(k, v),
    removeItem: (k) => void map.delete(k),
  };
}

// ── Seed content ───────────────────────────────────────────────────────────
test("a run seed includes the read-only framing and the task/run/diagnosis fields", () => {
  const r = run({ failure_class: "auth_required", error: "missing OPENAI key" });
  const seed = buildInvestigationSeed(runInvestigationInput(r, assessRunRecovery(r)!));
  // Read-only framing: explicitly tells Prime NOT to create/run anything.
  assert.match(seed, /DON'T create tasks, start runs/i);
  // Identity + diagnosis.
  assert.match(seed, /run_0007/);
  assert.match(seed, /task_0007/);
  assert.match(seed, /failure class auth_required/);
  assert.match(seed, /adapter claude-cli/);
  assert.match(seed, /assignee scout/);
  // The deterministic recovery diagnosis is folded in.
  assert.match(seed, /Authentication required/);
  assert.match(seed, /Root cause:/);
  assert.match(seed, /Recommended:/);
  // No client log tail supplied → points Prime at its read-only run tools, no fake block.
  assert.match(seed, /read-only run tools/i);
  assert.doesNotMatch(seed, /```/);
});

test("a blocked-task seed folds in the latest failed run's diagnosis and identity", () => {
  const latest = run({ id: "run_0008", status: "failed", failure_class: "timeout" });
  const seed = buildInvestigationSeed(
    taskInvestigationInput(task(), latest, assessTaskRecovery(task(), latest)!),
  );
  assert.match(seed, /a blocked task in Relux/);
  assert.match(seed, /title "ship the recovery card"/);
  assert.match(seed, /status blocked/);
  assert.match(seed, /run_0008/);
  assert.match(seed, /failure class timeout/);
});

test("a blocked task with no failed run names only the task (no fabricated run)", () => {
  const seed = buildInvestigationSeed(
    taskInvestigationInput(task(), null, assessTaskRecovery(task(), null)!),
  );
  assert.match(seed, /task_0007/);
  assert.doesNotMatch(seed, /- Run:/);
  assert.doesNotMatch(seed, /failure class/);
});

// ── Redaction + bounding ─────────────────────────────────────────────────────
test("redactSecrets scrubs api keys, bearer tokens, and key=value secrets but keeps prose", () => {
  const dirty =
    "calling provider with sk-ABC123def456ghi and Authorization: Bearer abcdef.ghijkl " +
    "and api_key=supersecretvalue while the tester ran fine";
  const clean = redactSecrets(dirty);
  assert.doesNotMatch(clean, /sk-ABC123def456ghi/);
  assert.doesNotMatch(clean, /abcdef\.ghijkl/);
  assert.doesNotMatch(clean, /supersecretvalue/);
  assert.match(clean, /\[REDACTED\]/);
  // Ordinary words around the secrets survive.
  assert.match(clean, /the tester ran fine/);
});

test("a supplied log tail is redacted and bounded to the most recent chars", () => {
  const longTail =
    "X".repeat(MAX_LOG_TAIL_CHARS + 500) + "\nFATAL token=leakme123456 at the very end";
  const seed = buildInvestigationSeed(
    runInvestigationInput(run(), assessRunRecovery(run())!, longTail),
  );
  // The tail block is present and the recent failure line survives...
  assert.match(seed, /Recent log tail/);
  assert.match(seed, /earlier lines omitted/);
  assert.match(seed, /FATAL/);
  // ...the secret in it is scrubbed...
  assert.doesNotMatch(seed, /leakme123456/);
  // ...and the whole seed stays bounded (tail clamp + framing, not the full 1700 chars).
  assert.ok(seed.length < MAX_LOG_TAIL_CHARS + 900, `seed too long: ${seed.length}`);
});

// ── Consume-once storage handoff ─────────────────────────────────────────────
test("the seed handoff is one-shot: consume reads then removes it", () => {
  const storage = memStorage();
  stashInvestigationSeed(storage, "investigate this");
  assert.equal(storage.map.get(INVESTIGATION_SEED_KEY), "investigate this");
  // First consume returns it and clears storage.
  assert.equal(consumeInvestigationSeed(storage), "investigate this");
  assert.equal(storage.map.has(INVESTIGATION_SEED_KEY), false);
  // Second consume (a refresh / re-mount) returns null → never re-sent.
  assert.equal(consumeInvestigationSeed(storage), null);
});

test("no pending seed yields null so normal Prime chat is untouched", () => {
  assert.equal(consumeInvestigationSeed(memStorage()), null);
});

test("a blank stashed seed is treated as absent", () => {
  const storage = memStorage();
  stashInvestigationSeed(storage, "   ");
  assert.equal(consumeInvestigationSeed(storage), null);
});
