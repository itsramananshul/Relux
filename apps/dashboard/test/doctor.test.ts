import { test } from "node:test";
import assert from "node:assert/strict";
import {
  severityBadgeClass,
  severityLabel,
  sortChecksBySeverity,
  doctorHeadline,
} from "../src/doctor.ts";

// The Doctor presentation helpers only map the kernel's severities to the
// dashboard's badge vocabulary and order rows for scanning — the kernel owns the
// diagnostics. These assertions pin that mapping + ordering so a regression (a
// fail painted as ok, an unsorted list) fails loudly. doctor.ts imports only
// TYPES from ./api, which strip-types erases, so this runs without a DOM.

test("severity maps to the right badge class and label", () => {
  assert.equal(severityBadgeClass("ok"), "done");
  assert.equal(severityBadgeClass("warn"), "in_progress");
  assert.equal(severityBadgeClass("fail"), "blocked");
  assert.equal(severityBadgeClass("info"), "backlog");

  assert.equal(severityLabel("ok"), "OK");
  assert.equal(severityLabel("warn"), "WARN");
  assert.equal(severityLabel("fail"), "FAIL");
  assert.equal(severityLabel("info"), "INFO");
});

test("checks sort worst-first, stable within a severity", () => {
  const checks = [
    { id: "a", label: "A", severity: "ok", message: "" },
    { id: "b", label: "B", severity: "fail", message: "" },
    { id: "c", label: "C", severity: "warn", message: "" },
    { id: "d", label: "D", severity: "info", message: "" },
    { id: "e", label: "E", severity: "fail", message: "" },
  ];
  const sorted = sortChecksBySeverity(checks).map((c) => c.id);
  // fail rows first (b before e, original order preserved), then warn, info, ok.
  assert.deepEqual(sorted, ["b", "e", "c", "d", "a"]);
});

test("headline summarizes non-zero buckets worst-first", () => {
  const report = {
    generated_at: 0,
    overall: "fail",
    summary: { ok: 4, info: 1, warn: 2, fail: 1 },
    checks: [],
  };
  assert.equal(doctorHeadline(report), "1 fail, 2 warn, 1 info, 4 ok");

  const clean = {
    generated_at: 0,
    overall: "ok",
    summary: { ok: 5, info: 0, warn: 0, fail: 0 },
    checks: [],
  };
  assert.equal(doctorHeadline(clean), "5 ok");
});
