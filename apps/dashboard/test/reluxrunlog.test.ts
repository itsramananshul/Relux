// Tests for the Relux Work run-log / tail surface (docs/HERMES_OPENCLAW_DEEP_AUDIT.md
// §8/§10). They cover the pure helpers (the render/no-logs/error/truncated state
// logic), pin the HTTP method + path + `since` cursor the dashboard sends to
// `/v1/relux/runs/:id/logs`, and confirm the committed bundle ships the Logs/Tail
// UI copy (catches a stale dist). Run: `npm test` or `node --test test/reluxrunlog.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { readdirSync, readFileSync } from "node:fs";
import {
  latestRunLogSeq,
  mergeRunLog,
  runLogIsEmpty,
  runLogSourceLabel,
  runLogTruncationNote,
} from "../src/reluxrunlog.ts";
import { reluxWork, type ReluxRunLog, type ReluxRunLogLine } from "../src/api.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const distDir = join(repoRoot, "crates", "relix-web-bridge", "dashboard-dist");

function line(seq: number, source: ReluxRunLogLine["source"], text = `l${seq}`): ReluxRunLogLine {
  return { seq, source, text };
}
function log(lines: ReluxRunLogLine[], extra: Partial<ReluxRunLog> = {}): ReluxRunLog {
  return { run_id: "run_0001", lines, ...extra };
}

// --- Pure helpers: source labels --------------------------------------------

test("runLogSourceLabel maps the three streams (and is total)", () => {
  assert.equal(runLogSourceLabel("stdout"), "stdout");
  assert.equal(runLogSourceLabel("stderr"), "stderr");
  assert.equal(runLogSourceLabel("system"), "system");
  // Defensive: an unknown source still renders rather than throwing.
  assert.equal(runLogSourceLabel("future" as never), "future");
});

// --- Pure helpers: empty / no-logs state ------------------------------------

test("runLogIsEmpty is true for null, undefined, and a zero-line log", () => {
  assert.equal(runLogIsEmpty(null), true);
  assert.equal(runLogIsEmpty(undefined), true);
  assert.equal(runLogIsEmpty(log([])), true);
  assert.equal(runLogIsEmpty(log([line(1, "stdout")])), false);
});

// --- Pure helpers: cursor ---------------------------------------------------

test("latestRunLogSeq returns the high-water seq (null when empty)", () => {
  assert.equal(latestRunLogSeq(null), null);
  assert.equal(latestRunLogSeq(log([])), null);
  assert.equal(latestRunLogSeq(log([line(1, "system"), line(5, "stdout"), line(3, "stderr")])), 5);
});

// --- Pure helpers: incremental merge ----------------------------------------

test("mergeRunLog appends only the new tail, deduped + ordered by seq", () => {
  const have = log([line(1, "system"), line(2, "stdout")]);
  const tail = log([line(3, "stdout"), line(4, "system")]);
  const merged = mergeRunLog(have, tail);
  assert.deepEqual(merged.lines.map((l) => l.seq), [1, 2, 3, 4]);
  // No mutation of the input.
  assert.equal(have.lines.length, 2);
});

test("mergeRunLog drops a duplicate seq (poll + initial load overlap) and sorts", () => {
  const have = log([line(1, "system"), line(2, "stdout")]);
  const merged = mergeRunLog(have, log([line(4, "system"), line(2, "stdout"), line(3, "stderr")]));
  assert.deepEqual(merged.lines.map((l) => l.seq), [1, 2, 3, 4]);
});

test("mergeRunLog onto a null base returns the incoming log", () => {
  const incoming = log([line(1, "stdout")]);
  assert.equal(mergeRunLog(null, incoming), incoming);
});

test("mergeRunLog carries the freshest run-level markers", () => {
  const have = log([line(1, "stdout")], { dropped_lines: 0 });
  const tail = log([line(2, "stdout")], { dropped_lines: 3, stdout_truncated: true });
  const merged = mergeRunLog(have, tail);
  assert.equal(merged.dropped_lines, 3);
  assert.equal(merged.stdout_truncated, true);
});

// --- Pure helpers: truncation / redaction note ------------------------------

test("runLogTruncationNote summarizes dropped lines and byte-capped streams (null when clean)", () => {
  assert.equal(runLogTruncationNote(null), null);
  assert.equal(runLogTruncationNote(log([line(1, "stdout")])), null);
  assert.equal(runLogTruncationNote(log([], { dropped_lines: 1 })), "1 earlier line dropped");
  assert.equal(runLogTruncationNote(log([], { dropped_lines: 4 })), "4 earlier lines dropped");
  assert.equal(
    runLogTruncationNote(log([], { stdout_truncated: true, stderr_truncated: true })),
    "stdout + stderr byte-capped",
  );
  assert.equal(
    runLogTruncationNote(log([], { dropped_lines: 2, stderr_truncated: true })),
    "2 earlier lines dropped; stderr byte-capped",
  );
});

// --- API request shape ------------------------------------------------------

type Captured = { url: string; method: string };
let captured: Captured | null = null;
const realFetch = globalThis.fetch;

function stubFetch(responseBody: unknown) {
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    captured = { url: String(url), method: init?.method ?? "GET" };
    return { ok: true, status: 200, text: async () => JSON.stringify(responseBody) } as Response;
  }) as typeof fetch;
}

afterEach(() => {
  globalThis.fetch = realFetch;
  captured = null;
});

test("getRunLogs GETs the run logs route (no since cursor on first load)", async () => {
  stubFetch(log([]));
  await reluxWork.getRunLogs("run_0001");
  assert.equal(captured!.method, "GET");
  assert.equal(captured!.url, "/v1/relux/runs/run_0001/logs");
});

test("getRunLogs appends the since cursor for the incremental tail", async () => {
  stubFetch(log([]));
  await reluxWork.getRunLogs("run_0001", 7);
  assert.equal(captured!.url, "/v1/relux/runs/run_0001/logs?since=7");
});

test("getRunLogs omits a zero/undefined since (degrades to a full fetch)", async () => {
  stubFetch(log([]));
  await reluxWork.getRunLogs("run_0001", 0);
  assert.equal(captured!.url, "/v1/relux/runs/run_0001/logs");
});

test("cancelRun POSTs the cancel route", async () => {
  // The mid-run cancel request shape (HERMES_OPENCLAW_DEEP_AUDIT §8/§26).
  stubFetch({ run_id: "run_0001", status: "requested", cancelling: true, message: "ok" });
  await reluxWork.cancelRun("run_0001");
  assert.equal(captured!.method, "POST");
  assert.equal(captured!.url, "/v1/relux/runs/run_0001/cancel");
});

// --- Committed bundle (no stale dist) ---------------------------------------

test("the committed dashboard bundle ships the Logs/Tail copy (no stale dist)", () => {
  const assetsDir = join(distDir, "assets");
  const jsFiles = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
  const bundle = jsFiles.map((f) => readFileSync(join(assetsDir, f), "utf8")).join("\n");
  assert.match(bundle, /Logs \/ Tail/);
  assert.match(bundle, /No logs captured for this run/);
});
