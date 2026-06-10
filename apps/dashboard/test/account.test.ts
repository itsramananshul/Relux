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
  sessionWarning,
  reauthCallout,
  ABSOLUTE_WARN_SECS,
  IDLE_WARN_SECS,
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

// ── Passive session-expiry warning (shell chip) ──────────────────────────
// These pin the decision the Relux shell uses to show its quiet expiry chip
// from the SAME non-sliding /v1/auth/me metadata (RELUX_MASTER_PLAN "Local
// operator login v1"). The shell renders whatever this returns — so the rules
// (when to warn, which window wins, when to stay silent) are pinned here.

test("sessionWarning thresholds match the documented windows", () => {
  assert.equal(ABSOLUTE_WARN_SECS, 30 * 60);
  assert.equal(IDLE_WARN_SECS, 10 * 60);
});

test("sessionWarning stays hidden when both windows are comfortably open", () => {
  // META has 12h idle + 7d absolute remaining — nothing to warn about.
  assert.equal(sessionWarning(META, 0), null);
});

test("sessionWarning fires on the absolute ceiling within 30 min", () => {
  const m: SessionMeta = { username: "ops", absolute_expires_in_secs: 20 * 60 };
  const w = sessionWarning(m, 0);
  assert.equal(w?.kind, "absolute");
  assert.equal(w?.secsLeft, 20 * 60);
  assert.match(w?.message ?? "", /re-sign-in required in/i);
});

test("sessionWarning fires on idle inactivity within 10 min", () => {
  const m: SessionMeta = { username: "ops", idle_expires_in_secs: 8 * 60 };
  const w = sessionWarning(m, 0);
  assert.equal(w?.kind, "idle");
  assert.equal(w?.secsLeft, 8 * 60);
  assert.match(w?.message ?? "", /inactivity/i);
});

test("sessionWarning shows the more urgent window; a tie favours absolute", () => {
  // Idle closer than absolute → idle wins (it signs out sooner).
  const closerIdle: SessionMeta = {
    username: "ops",
    absolute_expires_in_secs: 25 * 60,
    idle_expires_in_secs: 5 * 60,
  };
  assert.equal(sessionWarning(closerIdle, 0)?.kind, "idle");
  // Equal seconds left → absolute (only a fresh sign-in clears it).
  const tie: SessionMeta = {
    username: "ops",
    absolute_expires_in_secs: 6 * 60,
    idle_expires_in_secs: 6 * 60,
  };
  assert.equal(sessionWarning(tie, 0)?.kind, "absolute");
});

test("sessionWarning honours local elapsed time and the threshold edge", () => {
  const m: SessionMeta = { username: "ops", absolute_expires_in_secs: 31 * 60 };
  assert.equal(sessionWarning(m, 0), null); // 31m out — just outside the window
  assert.equal(sessionWarning(m, 2 * 60)?.kind, "absolute"); // 29m left — now warns
});

test("sessionWarning stays silent under the dev bypass and for an older kernel", () => {
  // RELUX_AUTH_DISABLED sends no deadlines — never warn.
  const bypass: SessionMeta = {
    username: "ops",
    auth_disabled: true,
    absolute_expires_in_secs: 60,
  };
  assert.equal(sessionWarning(bypass, 0), null);
  // An older kernel sends only { username } — hide the chip, don't invent it.
  assert.equal(sessionWarning({ username: "ops" }, 0), null);
});

// ── Re-authentication callout (Account control) ──────────────────────────
// The Account panel always offers a "Sign out and sign back in" button; this
// helper decides only when to EMPHASISE it (RELUX_MASTER_PLAN "Local operator
// login v1"). It fires solely on the non-sliding absolute ceiling within the
// same warning window the chip uses — a fresh sign-in is the only thing that
// extends it — and stays silent for idle, the dev bypass, and an older kernel.

test("reauthCallout emphasises within the absolute warning window", () => {
  const m: SessionMeta = { username: "ops", absolute_expires_in_secs: 20 * 60 };
  const c = reauthCallout(m, 0);
  assert.equal(c?.secsLeft, 20 * 60);
  assert.match(c?.message ?? "", /re-sign-in required in/i);
  // Right at the threshold edge it still fires; just past it stays quiet.
  assert.ok(reauthCallout({ username: "ops", absolute_expires_in_secs: ABSOLUTE_WARN_SECS }, 0));
  assert.equal(reauthCallout({ username: "ops", absolute_expires_in_secs: ABSOLUTE_WARN_SECS + 1 }, 0), null);
});

test("reauthCallout stays quiet when the ceiling is comfortably far off", () => {
  // META has 7d absolute remaining — the button renders unadorned, no banner.
  assert.equal(reauthCallout(META, 0), null);
});

test("reauthCallout ignores idle expiry (only a fresh sign-in clears absolute)", () => {
  // Idle about to bite but the absolute ceiling is a week out → no re-auth banner.
  const m: SessionMeta = {
    username: "ops",
    idle_expires_in_secs: 2 * 60,
    absolute_expires_in_secs: 7 * 86400,
  };
  assert.equal(reauthCallout(m, 0), null);
});

test("reauthCallout honours local elapsed time", () => {
  const m: SessionMeta = { username: "ops", absolute_expires_in_secs: 32 * 60 };
  assert.equal(reauthCallout(m, 0), null); // 32m out — outside the window
  assert.equal(reauthCallout(m, 3 * 60)?.secsLeft, 29 * 60); // 29m left — now warns
});

test("reauthCallout stays silent under the dev bypass and for an older kernel", () => {
  const bypass: SessionMeta = { username: "ops", auth_disabled: true, absolute_expires_in_secs: 60 };
  assert.equal(reauthCallout(bypass, 0), null);
  assert.equal(reauthCallout({ username: "ops" }, 0), null);
});
