import { test } from "node:test";
import assert from "node:assert/strict";
import {
  reluxEventSeq,
  latestReluxEventId,
  mergeReluxRunEvents,
  noActivityLabel,
} from "../src/reluxruntranscript.ts";
import type { ReluxRunEvent } from "../src/api.ts";

function ev(seq: number, kind = "tool_call", message = ""): ReluxRunEvent {
  const id = `revent_${String(seq).padStart(4, "0")}`;
  return { id, run_id: "run_0001", ts: `t${seq}`, kind, source: "kernel", message };
}

test("reluxEventSeq parses the numeric suffix (null when absent)", () => {
  assert.equal(reluxEventSeq("revent_0001"), 1);
  assert.equal(reluxEventSeq("revent_12345"), 12345); // past the 4-digit pad
  assert.equal(reluxEventSeq("revent_"), null);
  assert.equal(reluxEventSeq("garbage"), null);
  assert.equal(reluxEventSeq(undefined), null);
});

test("latestReluxEventId returns the high-water id (null when empty)", () => {
  assert.equal(latestReluxEventId([]), null);
  assert.equal(latestReluxEventId([ev(1), ev(5), ev(3)]), "revent_0005");
  // Numeric ordering, not lexicographic: revent_0010 > revent_0009.
  assert.equal(latestReluxEventId([ev(9), ev(10)]), "revent_0010");
});

test("mergeReluxRunEvents appends only the new tail, deduped + ordered", () => {
  const have = [ev(1, "run_started"), ev(2, "tool_call")];
  const tail = [ev(3, "tool_call"), ev(4, "run_completed")];
  const merged = mergeReluxRunEvents(have, tail);
  assert.deepEqual(merged.map((e) => e.id), [
    "revent_0001",
    "revent_0002",
    "revent_0003",
    "revent_0004",
  ]);
  // No mutation of the input array.
  assert.equal(have.length, 2);
});

test("mergeReluxRunEvents drops a duplicate id (poll + initial load overlap)", () => {
  const have = [ev(1, "run_started"), ev(2, "tool_call")];
  const merged = mergeReluxRunEvents(have, [ev(2, "tool_call"), ev(3, "run_completed")]);
  assert.deepEqual(merged.map((e) => e.id), ["revent_0001", "revent_0002", "revent_0003"]);
});

test("mergeReluxRunEvents returns existing unchanged for an empty tail", () => {
  const have = [ev(1, "run_started")];
  assert.equal(mergeReluxRunEvents(have, []), have);
});

test("mergeReluxRunEvents sorts an out-of-order tail by sequence", () => {
  const merged = mergeReluxRunEvents([ev(1)], [ev(4), ev(2), ev(3)]);
  assert.deepEqual(merged.map((e) => e.id), [
    "revent_0001",
    "revent_0002",
    "revent_0003",
    "revent_0004",
  ]);
});

test("noActivityLabel stays quiet until the threshold, then reports honest elapsed", () => {
  // Unknown activity time → no signal.
  assert.equal(noActivityLabel(null, 100_000, 10), null);
  // Recent activity (under threshold) → no signal, show the normal live state.
  assert.equal(noActivityLabel(100_000, 105_000, 10), null);
  // At/over the threshold → honest "No activity for Xs".
  assert.equal(noActivityLabel(100_000, 114_000, 10), "No activity for 14s");
  // Over a minute formats as Xm Ys.
  assert.equal(noActivityLabel(100_000, 100_000 + 75_000, 10), "No activity for 1m 15s");
  // Clock skew (now before last) never produces a bogus signal.
  assert.equal(noActivityLabel(100_000, 90_000, 10), null);
});
