import { test } from "node:test";
import assert from "node:assert/strict";
import { errorBoundaryMessage } from "../src/components/errorBoundaryMessage.ts";

// The route-level ErrorBoundary turns a render crash into a readable card instead
// of a white screen (RELUX_MASTER_PLAN §17.6; the reported blank pages). Its
// human-facing message is a pure function, unit-tested here without a DOM.

test("errorBoundaryMessage prefers an Error's message", () => {
  assert.equal(errorBoundaryMessage(new Error("boom")), "boom");
  assert.equal(errorBoundaryMessage(new TypeError("bad type")), "bad type");
});

test("errorBoundaryMessage falls back to the name for a message-less Error", () => {
  const e = new Error("");
  assert.equal(errorBoundaryMessage(e), "Error");
});

test("errorBoundaryMessage passes a string through", () => {
  assert.equal(errorBoundaryMessage("kaput"), "kaput");
});

test("errorBoundaryMessage never returns empty for null/undefined", () => {
  assert.equal(errorBoundaryMessage(null), "Unknown error");
  assert.equal(errorBoundaryMessage(undefined), "Unknown error");
  assert.equal(errorBoundaryMessage(""), "Unknown error");
});

test("errorBoundaryMessage stringifies an unexpected non-Error value", () => {
  assert.equal(errorBoundaryMessage(42), "42");
});
