// Pure unit tests for the Work board status-MOVE helpers (design §6). Framework-free,
// so they run under `node --strip-types` without the esbuild render harness (see the
// docs note dashboard-test-tsx-vs-ts-split).
//
// These pin that the offered moves mirror the backend EXACTLY: relux-kernel
// `set_task_status` accepts only the operator-settable targets (`blocked` /
// `cancelled`) on a NON-terminal task, never a machine-driven state, never a finished
// task. If the UI offered more than the route accepts, an operator would hit a 4xx.
//
// Run: `npm test` (auto-discovered) or `node --test test/taskmove.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  operatorStatusMoves,
  canMoveStatus,
  isTerminalStatus,
} from "../src/taskmove.ts";

test("a non-terminal task offers the operator-settable moves except its own status", () => {
  // created/queued (the "open" bucket) → both Block and Cancel.
  for (const s of ["created", "queued"]) {
    const moves = operatorStatusMoves(s);
    assert.deepEqual(
      moves.map((m) => m.status),
      ["blocked", "cancelled"],
      `open status ${s}`,
    );
    assert.deepEqual(moves.map((m) => m.label), ["Block", "Cancel"]);
  }
  // running / leased / waiting_for_tool are non-terminal → still Block / Cancel.
  for (const s of ["running", "leased", "waiting_for_tool", "waiting_for_approval"]) {
    assert.deepEqual(operatorStatusMoves(s).map((m) => m.status), ["blocked", "cancelled"], s);
  }
});

test("a blocked task drops Block (its own status) and keeps Cancel", () => {
  const moves = operatorStatusMoves("blocked");
  assert.deepEqual(moves.map((m) => m.status), ["cancelled"]);
  assert.deepEqual(moves.map((m) => m.label), ["Cancel"]);
});

test("a terminal task offers NO moves (a finished task is never edited)", () => {
  for (const s of ["completed", "failed", "cancelled", "expired"]) {
    assert.equal(operatorStatusMoves(s).length, 0, `terminal ${s} must offer no move`);
    assert.equal(canMoveStatus(s), false, `terminal ${s} cannot move`);
    assert.equal(isTerminalStatus(s), true, `${s} is terminal`);
  }
});

test("a machine-driven status is never an OFFERED target", () => {
  // running/completed/failed are driven by the run lifecycle; the board never sets
  // them, so they must not appear as a move target for ANY source status.
  const machineDriven = new Set(["running", "completed", "failed"]);
  for (const src of ["created", "queued", "running", "blocked", "waiting_for_approval"]) {
    for (const m of operatorStatusMoves(src)) {
      assert.equal(machineDriven.has(m.status), false, `${src} offered ${m.status}`);
    }
  }
});

test("canMoveStatus is true exactly when a move exists", () => {
  assert.equal(canMoveStatus("queued"), true);
  assert.equal(canMoveStatus("blocked"), true); // can still cancel
  assert.equal(canMoveStatus("completed"), false);
  assert.equal(isTerminalStatus("queued"), false);
});
