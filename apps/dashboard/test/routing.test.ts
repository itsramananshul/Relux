import { test } from "node:test";
import assert from "node:assert/strict";
import {
  isLegacyPath,
  LEGACY_PATHS,
  RELUX_PATHS,
  workRunHref,
  workRunShareUrl,
  runIdFromSearch,
  workTaskHref,
  taskIdFromSearch,
} from "../src/routing.ts";

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

// Run-detail deep links stay in the Relux shell (Work surface), never the legacy
// `/runs` console. workRunHref builds the link; runIdFromSearch reads it back.

test("workRunHref points a run into the in-shell Work surface, not legacy /runs", () => {
  const href = workRunHref("run_0001");
  assert.equal(href, "/work?run=run_0001");
  // Owned by the Relux shell so it never falls into the bridge-gated console.
  assert.equal(isLegacyPath("/work"), false);
});

test("workRunHref percent-encodes ids so odd characters can't break the query", () => {
  assert.equal(workRunHref("a b&c=d"), "/work?run=a%20b%26c%3Dd");
});

test("runIdFromSearch round-trips the id workRunHref encodes", () => {
  const tricky = "durable:orch_0001/2 3";
  const search = new URL(`http://x${workRunHref(tricky)}`).search;
  assert.equal(runIdFromSearch(search), tricky);
});

test("runIdFromSearch reads the run param with or without a leading '?'", () => {
  assert.equal(runIdFromSearch("?run=run_42"), "run_42");
  assert.equal(runIdFromSearch("run=run_42"), "run_42");
  // Other Work filters alongside it don't confuse the read.
  assert.equal(runIdFromSearch("?status=running&run=run_42"), "run_42");
});

test("runIdFromSearch returns null when no run is selected", () => {
  for (const s of ["", "?", "?status=running", "?run="]) {
    assert.equal(runIdFromSearch(s), null, `'${s}' must select no run`);
  }
});

// workRunShareUrl builds the copy-paste-able absolute link to the in-shell run
// detail. It must carry the `/dashboard` basename and reuse workRunHref's
// encoding so a shared link round-trips through runIdFromSearch.

test("workRunShareUrl prefixes the /dashboard basename onto the in-shell href", () => {
  assert.equal(
    workRunShareUrl("run_0001", "https://host:9000"),
    "https://host:9000/dashboard/work?run=run_0001",
  );
});

test("workRunShareUrl reuses workRunHref encoding so odd ids stay query-safe", () => {
  const origin = "https://h";
  assert.equal(workRunShareUrl("a b&c=d", origin), `${origin}/dashboard${workRunHref("a b&c=d")}`);
  assert.equal(workRunShareUrl("a b&c=d", origin), "https://h/dashboard/work?run=a%20b%26c%3Dd");
});

test("a shared workRunShareUrl round-trips back to the same run id", () => {
  const id = "durable:orch_0001/2 3";
  const url = new URL(workRunShareUrl(id, "https://host"));
  assert.equal(url.pathname, "/dashboard/work");
  assert.equal(runIdFromSearch(url.search), id);
});

// Task-detail deep links: the link Prime shows after creating a task. The old
// bare `/work` landed on the board with nothing focused (the reported blank/wrong
// page); `/work?task=<id>` opens that task's detail panel. workTaskHref builds it;
// taskIdFromSearch reads it back.

test("workTaskHref points a task into the in-shell Work surface, not a blank board", () => {
  assert.equal(workTaskHref("task_0001"), "/work?task=task_0001");
  // Owned by the Relux shell so the deep link never falls into the bridge console.
  assert.equal(isLegacyPath("/work"), false);
});

test("workTaskHref percent-encodes ids so odd characters can't break the query", () => {
  assert.equal(workTaskHref("a b&c=d"), "/work?task=a%20b%26c%3Dd");
});

test("taskIdFromSearch round-trips the id workTaskHref encodes", () => {
  const tricky = "task:weird/1 2&3";
  const search = new URL(`http://x${workTaskHref(tricky)}`).search;
  assert.equal(taskIdFromSearch(search), tricky);
});

test("taskIdFromSearch reads the task param with or without a leading '?'", () => {
  assert.equal(taskIdFromSearch("?task=task_42"), "task_42");
  assert.equal(taskIdFromSearch("task=task_42"), "task_42");
  // Other Work filters alongside it don't confuse the read.
  assert.equal(taskIdFromSearch("?status=running&task=task_42"), "task_42");
});

test("taskIdFromSearch returns null when no task is selected", () => {
  for (const s of ["", "?", "?status=running", "?task=", "?run=run_1"]) {
    assert.equal(taskIdFromSearch(s), null, `'${s}' must select no task`);
  }
});

test("run and task params are read independently on the same Work URL", () => {
  // Defensive: a URL never carries both (selecting one clears the other), but the
  // readers must not cross-wire if it somehow did.
  assert.equal(taskIdFromSearch("?task=task_7"), "task_7");
  assert.equal(runIdFromSearch("?task=task_7"), null);
  assert.equal(runIdFromSearch("?run=run_7"), "run_7");
  assert.equal(taskIdFromSearch("?run=run_7"), null);
});
