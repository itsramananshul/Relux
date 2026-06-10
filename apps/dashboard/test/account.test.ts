import { test } from "node:test";
import assert from "node:assert/strict";
import { MIN_PASSWORD_LEN, validatePasswordChange } from "../src/account.ts";

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
