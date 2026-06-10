import { test } from "node:test";
import assert from "node:assert/strict";
import { isLegacyPath, LEGACY_PATHS, RELUX_PATHS } from "../src/routing.ts";

// Guard against the blank-page regression: the Relux shell must own every path
// except the explicit legacy set. If a Relux route (or any unknown sub-path) ever
// fell into the legacy branch, it would render blank/login under
// `relux-kernel serve`. These assertions fail loudly if that happens again.

test("every Relux route is owned by the Relux shell (not legacy)", () => {
  for (const p of RELUX_PATHS) {
    assert.equal(isLegacyPath(p), false, `${p} must render in the Relux shell`);
  }
});

test("/approvals is owned by the Relux shell, not the legacy console", () => {
  assert.equal(isLegacyPath("/approvals"), false);
});

test("unknown deep links are NOT legacy, so they hit the in-shell not-found", () => {
  // The exact case from the bug: a Prime-created agent link like /crew/<id>.
  for (const p of ["/crew/agent_0001", "/work/123", "/totally-unknown", "/plugins/x"]) {
    assert.equal(isLegacyPath(p), false, `${p} must not fall into the bridge dashboard`);
  }
});

test("the declared legacy paths still route to the legacy console", () => {
  for (const p of LEGACY_PATHS) {
    assert.equal(isLegacyPath(p), true, `${p} should stay on the legacy dashboard`);
  }
});

test("Relux and legacy path sets do not overlap", () => {
  const legacy = new Set(LEGACY_PATHS);
  for (const p of RELUX_PATHS) {
    assert.equal(legacy.has(p), false, `${p} is claimed by both shells`);
  }
});
