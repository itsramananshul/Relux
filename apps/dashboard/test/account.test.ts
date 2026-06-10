import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MIN_PASSWORD_LEN,
  validatePasswordChange,
  formatDuration,
  idleRemaining,
  absoluteRemaining,
  describeIdlePolicy,
  describeAbsolutePolicy,
  type SessionMeta,
} from "../src/account.ts";

// The password-change form's client-side guard (RELUX_MASTER_PLAN "Local
// operator login v1"). These pin the friendly pre-flight rules so a regression
// (an empty field slipping through, a too-short password, a confirm mismatch)
// fails loudly. The kernel is still the authority — it re-validates server-side
// — but the form should never POST an obviously-bad request.

test("accepts a well-formed change", () => {
  assert.equal(validatePasswordChange("oldpass12", "newpass34", "newpass34"), null);
});

test("requires the current password", () => {
  const msg = validatePasswordChange("", "newpass34", "newpass34");
  assert.match(msg ?? "", /current password/i);
});

test("requires a new password", () => {
  const msg = validatePasswordChange("oldpass12", "", "");
  assert.match(msg ?? "", /new password/i);
});

test("enforces the minimum length", () => {
  const short = "x".repeat(MIN_PASSWORD_LEN - 1);
  const msg = validatePasswordChange("oldpass12", short, short);
  assert.match(msg ?? "", new RegExp(`${MIN_PASSWORD_LEN} characters`));
});

test("requires the confirmation to match", () => {
  const msg = validatePasswordChange("oldpass12", "newpass34", "newpass99");
  assert.match(msg ?? "", /do not match/i);
});

test("rejects reusing the current password", () => {
  const msg = validatePasswordChange("samepass1", "samepass1", "samepass1");
  assert.match(msg ?? "", /differ from the current/i);
});

// ── Session expiry / idle readout helpers ────────────────────────────────
// These pin the friendly formatting the Account control renders from
// `/v1/auth/me` (RELUX_MASTER_PLAN "Local operator login v1" — sliding sessions).

test("formatDuration shows the coarsest two non-zero units", () => {
  assert.equal(formatDuration(12 * 3600), "12h");
  assert.equal(formatDuration(3600 + 5 * 60), "1h 5m");
  assert.equal(formatDuration(45 * 60), "45m");
  assert.equal(formatDuration(30), "30s");
  assert.equal(formatDuration(7 * 86400), "7d");
  // Days + hours drop the finer minutes/seconds (at most two units).
  assert.equal(formatDuration(7 * 86400 + 3 * 3600 + 12 * 60 + 9), "7d 3h");
});

test("formatDuration clamps zero and negative to 0s", () => {
  assert.equal(formatDuration(0), "0s");
  assert.equal(formatDuration(-100), "0s");
});

const META: SessionMeta = {
  username: "ops",
  idle_expires_in_secs: 12 * 3600,
  absolute_expires_in_secs: 7 * 86400,
  idle_timeout_secs: 12 * 3600,
  absolute_max_secs: 7 * 86400,
};

test("idle/absolute remaining decrement by local elapsed time, clamped at 0", () => {
  assert.equal(idleRemaining(META, 0), 12 * 3600);
  assert.equal(idleRemaining(META, 60), 12 * 3600 - 60);
  // Past the window → 0, never negative.
  assert.equal(idleRemaining(META, 13 * 3600), 0);
  assert.equal(absoluteRemaining(META, 86400), 7 * 86400 - 86400);
});

test("remaining helpers return null when the kernel omitted the field", () => {
  // An older kernel sends only { username } — hide the countdown, don't invent it.
  const bare: SessionMeta = { username: "ops" };
  assert.equal(idleRemaining(bare, 0), null);
  assert.equal(absoluteRemaining(bare, 0), null);
});

test("policy descriptions read in friendly form, or null when absent", () => {
  assert.equal(describeIdlePolicy(META), "Signs out after 12h of inactivity");
  assert.equal(describeAbsolutePolicy(META), "Re-sign-in required after 7d");
  const bare: SessionMeta = { username: "ops" };
  assert.equal(describeIdlePolicy(bare), null);
  assert.equal(describeAbsolutePolicy(bare), null);
});
