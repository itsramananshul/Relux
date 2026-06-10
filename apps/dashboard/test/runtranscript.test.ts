import { test } from "node:test";
import assert from "node:assert/strict";
import {
  kindLabel,
  latestEventId,
  lastEventAgo,
  mergeRunEvents,
  noActivityLabel,
  RUN_STALL_SECS,
  runTranscriptProgress,
  transcriptBarClass,
  transcriptScrollMax,
  TRANSCRIPT_SCROLL_MAX,
  TRANSCRIPT_SCROLL_MAX_COMPACT,
} from "../src/runtranscript.ts";
import { noActivityLabel as reluxNoActivityLabel } from "../src/reluxruntranscript.ts";
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

test("transcript compact layout: bar modifier + shorter scroll, cue never dropped", () => {
  // The Runs-page (full) render gets the plain bar + the taller viewport.
  assert.equal(transcriptBarClass(false), "xtr-bar");
  assert.equal(transcriptBarClass(undefined), "xtr-bar");
  assert.equal(transcriptScrollMax(false), TRANSCRIPT_SCROLL_MAX);
  // The Brief-workroom embed (compact) tightens the header via the `--compact`
  // modifier and shortens the viewport — but it only adds to the bar class, so
  // every live/stalled cue rendered inside it survives.
  assert.equal(transcriptBarClass(true), "xtr-bar xtr-bar--compact");
  assert.ok(transcriptBarClass(true).startsWith("xtr-bar "));
  assert.equal(transcriptScrollMax(true), TRANSCRIPT_SCROLL_MAX_COMPACT);
  // Compact is genuinely shorter (so it doesn't dominate the workroom) but not
  // collapsed away.
  assert.ok(TRANSCRIPT_SCROLL_MAX_COMPACT < TRANSCRIPT_SCROLL_MAX);
  assert.ok(TRANSCRIPT_SCROLL_MAX_COMPACT > 0);
});

test("noActivityLabel drives the legacy stalled cue (shared with the Relux surface)", () => {
  // The legacy `<RunTranscript>` re-exports the SAME pure helper the Relux Work
  // Run Detail uses, so the threshold + `No activity for Xs` copy stay identical
  // across both transcript surfaces.
  assert.equal(noActivityLabel, reluxNoActivityLabel);
  assert.equal(RUN_STALL_SECS, 10);

  // Unknown / recent activity → no signal (normal live chip shows instead).
  assert.equal(noActivityLabel(null, 100_000), null);
  assert.equal(noActivityLabel(100_000, 105_000), null); // under threshold
  // At/over the shared threshold → honest elapsed silence; minutes format as Xm Ys.
  assert.equal(noActivityLabel(100_000, 100_000 + 10_000), "No activity for 10s");
  assert.equal(noActivityLabel(100_000, 100_000 + 90_000), "No activity for 1m 30s");
  // Clock skew (now before last) never fabricates a signal.
  assert.equal(noActivityLabel(100_000, 90_000), null);
  // Legacy drives it from the real wall-clock event `ts` (seconds → millis): an
  // event 12s old at a threshold of 10 is stale; the same event at 8s old isn't.
  const lastTsSecs = 1000;
  assert.equal(noActivityLabel(lastTsSecs * 1000, (lastTsSecs + 12) * 1000), "No activity for 12s");
  assert.equal(noActivityLabel(lastTsSecs * 1000, (lastTsSecs + 8) * 1000), null);
});
