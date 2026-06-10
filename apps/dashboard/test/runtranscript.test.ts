import { test } from "node:test";
import assert from "node:assert/strict";
import {
  kindLabel,
  latestEventId,
  lastEventAgo,
  mergeRunEvents,
  runTranscriptProgress,
} from "../src/runtranscript.ts";
import type { RunEvent } from "../src/api.ts";

function ev(event_id: number, kind: string, message = "", ts = 1000 + event_id): RunEvent {
  return { event_id, kind, message, ts, source: "claude" };
}

test("latestEventId returns the high-water cursor (0 when empty)", () => {
  assert.equal(latestEventId([]), 0);
  assert.equal(latestEventId([ev(1, "tool_use"), ev(5, "result"), ev(3, "tool_use")]), 5);
  // Events lacking an id never advance the cursor.
  assert.equal(latestEventId([{ kind: "x" } as RunEvent, ev(2, "tool_use")]), 2);
});

test("mergeRunEvents appends only the new tail, deduped + ordered", () => {
  const have = [ev(1, "accepted"), ev(2, "assistant_message")];
  const tail = [ev(3, "tool_use", "Read"), ev(4, "tool_use", "Edit")];
  const merged = mergeRunEvents(have, tail);
  assert.deepEqual(
    merged.map((e) => e.event_id),
    [1, 2, 3, 4],
  );
  // The originals are untouched (no mutation of the input array).
  assert.equal(have.length, 2);
});

test("mergeRunEvents drops a duplicate event_id (poll + stream overlap)", () => {
  const have = [ev(1, "accepted"), ev(2, "tool_use")];
  // A tail that re-includes id 2 plus a genuinely new id 3.
  const merged = mergeRunEvents(have, [ev(2, "tool_use"), ev(3, "result")]);
  assert.deepEqual(
    merged.map((e) => e.event_id),
    [1, 2, 3],
  );
});

test("mergeRunEvents returns existing unchanged for an empty tail", () => {
  const have = [ev(1, "accepted")];
  assert.equal(mergeRunEvents(have, []), have);
});

test("mergeRunEvents sorts an out-of-order tail chronologically", () => {
  const merged = mergeRunEvents([ev(1, "accepted")], [ev(4, "result"), ev(2, "tool_use"), ev(3, "tool_use")]);
  assert.deepEqual(
    merged.map((e) => e.event_id),
    [1, 2, 3, 4],
  );
});

test("runTranscriptProgress reports real count / phase / last ts — no fabrication", () => {
  assert.deepEqual(runTranscriptProgress([]), { count: 0, phase: null, lastTs: null });
  const events = [ev(1, "accepted", "", 1000), ev(2, "tool_use", "Edit", 1042)];
  const p = runTranscriptProgress(events);
  assert.equal(p.count, 2);
  assert.equal(p.phase, "tool call"); // humanized latest kind
  assert.equal(p.lastTs, 1042);
});

test("kindLabel humanizes known kinds and falls back to the raw kind", () => {
  assert.equal(kindLabel("assistant_message"), "thinking");
  assert.equal(kindLabel("apply.applied"), "applied");
  assert.equal(kindLabel("totally_new_kind"), "totally_new_kind");
  assert.equal(kindLabel(undefined), "event");
});

test("lastEventAgo formats the last-event clock (pure, injected now)", () => {
  assert.equal(lastEventAgo(null, 100), null);
  assert.equal(lastEventAgo(100, 100), "just now");
  assert.equal(lastEventAgo(100, 112), "12s ago");
  assert.equal(lastEventAgo(100, 100 + 185), "3m ago");
  // A clock skew (future ts) never goes negative.
  assert.equal(lastEventAgo(200, 100), "just now");
});
