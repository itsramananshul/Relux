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
  columnDropTarget,
  encodeTaskDrag,
  parseTaskDrag,
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

// ---------------------------------------------------------------------------
// Drag-to-column resolution (design §6 "Drag a card to a column → status mutation,
// with transition validation"). The drop must resolve to EXACTLY the move the select
// would offer, and reject everything else with an honest reason.
// ---------------------------------------------------------------------------

test("dropping a non-terminal task on Blocked → block; on Done → cancel", () => {
  for (const s of ["created", "queued", "running", "leased", "waiting_for_approval"]) {
    const toBlocked = columnDropTarget("blocked", s);
    assert.equal(toBlocked.ok, true, `${s} → blocked column`);
    if (toBlocked.ok) {
      assert.equal(toBlocked.status, "blocked");
      assert.equal(toBlocked.label, "Block");
    }
    const toDone = columnDropTarget("done", s);
    assert.equal(toDone.ok, true, `${s} → done column`);
    if (toDone.ok) {
      assert.equal(toDone.status, "cancelled");
      assert.equal(toDone.label, "Cancel");
    }
  }
});

test("dropping on the Open or Running (machine-driven) lanes is rejected with a reason", () => {
  for (const col of ["open", "running"] as const) {
    const res = columnDropTarget(col, "queued");
    assert.equal(res.ok, false, `${col} is not operator-settable`);
    if (!res.ok) assert.ok(res.reason.length > 0, `${col} carries a reason`);
  }
  // The Running reason names the run lifecycle (it is machine-driven, not a decree).
  const running = columnDropTarget("running", "created");
  assert.equal(running.ok, false);
  if (!running.ok) assert.match(running.reason, /run lifecycle/i);
});

test("dropping a blocked task on the Blocked column is a rejected no-op (already there)", () => {
  const res = columnDropTarget("blocked", "blocked");
  assert.equal(res.ok, false);
  if (!res.ok) assert.match(res.reason, /already/i);
  // …but a blocked task CAN still be dropped on Done → cancel.
  const cancel = columnDropTarget("done", "blocked");
  assert.equal(cancel.ok, true);
  if (cancel.ok) assert.equal(cancel.status, "cancelled");
});

test("a terminal task is rejected from every column (a finished task is never moved)", () => {
  for (const s of ["completed", "failed", "cancelled", "expired"]) {
    for (const col of ["open", "running", "blocked", "done"] as const) {
      const res = columnDropTarget(col, s);
      assert.equal(res.ok, false, `terminal ${s} → ${col}`);
      if (!res.ok) assert.match(res.reason, /finished/i);
    }
  }
});

test("columnDropTarget never resolves to a move the select would not offer", () => {
  // For every (source status, column) pair, an ok drop must be in operatorStatusMoves.
  for (const s of ["created", "queued", "running", "blocked", "waiting_for_approval", "completed"]) {
    for (const col of ["open", "running", "blocked", "done"] as const) {
      const res = columnDropTarget(col, s);
      if (res.ok) {
        assert.ok(
          operatorStatusMoves(s).some((m) => m.status === res.status),
          `${s} → ${col} resolved ${res.status} the select does not offer`,
        );
      }
    }
  }
});

test("the drag payload round-trips and a foreign payload is ignored, not thrown", () => {
  const enc = encodeTaskDrag({ id: "task_7", status: "queued" });
  const back = parseTaskDrag(enc);
  assert.deepEqual(back, { id: "task_7", status: "queued" });
  // Foreign / malformed payloads decode to null (a non-task drop is ignored).
  assert.equal(parseTaskDrag(""), null);
  assert.equal(parseTaskDrag(null), null);
  assert.equal(parseTaskDrag("not json"), null);
  assert.equal(parseTaskDrag('{"id":""}'), null); // empty id rejected
  assert.equal(parseTaskDrag('{"status":"queued"}'), null); // no id
});
