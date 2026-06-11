// Request-shape tests for the per-agent bearer path `agentSelfAssignTask`. These pin the
// HONEST trust boundary of the token-authenticated manager-subtree assignment
// (docs/HERMES_OPENCLAW_DEEP_AUDIT.md §21):
//   - it sends `Authorization: Bearer <token>` (the only thing that authenticates here),
//   - it OMITS credentials so the operator's `relux_session` cookie plays no part,
//   - the acting manager is the token subject — the body carries only task + target,
//   - a non-OK response throws an ApiError WITHOUT firing the session-expired signal
//     (a 401 here means a bad TOKEN, not an operator-session lapse).
// We stub `globalThis.fetch` to capture the exact request, so the test never hits a server.
// Run: `npm test` (auto-discovered) or `node --test test/manager-token-actions.test.ts`.

import { test, afterEach } from "node:test";
import assert from "node:assert/strict";
import {
  agentSelfAssignTask,
  agentSelfManagerGrant,
  ApiError,
  onSessionExpired,
} from "../src/api.ts";

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

function fakeResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (body === undefined ? "" : JSON.stringify(body)),
  } as unknown as Response;
}

test("agentSelfAssignTask sends a bearer token, omits the operator cookie, and posts only task+target", async () => {
  let captured: { url: string; init: RequestInit } | null = null;
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    captured = { url, init };
    return fakeResponse(200, { id: "task_1", status: "Queued", assigned_agent: "ic" });
  }) as typeof fetch;

  const task = await agentSelfAssignTask("relux_agt_secret123", "task_1", "ic");

  assert.ok(captured, "fetch was called");
  const { url, init } = captured!;
  // The real agent-self route — never an operator route.
  assert.equal(url, "/v1/relux/agents/me/assign-task");
  assert.equal(init.method, "POST");
  // The operator session must NOT be used: credentials are omitted.
  assert.equal(init.credentials, "omit");
  // The bearer token is the credential; the cookie path is not used.
  const headers = init.headers as Record<string, string>;
  assert.equal(headers.authorization, "Bearer relux_agt_secret123");
  assert.equal(headers["content-type"], "application/json");
  // The body carries ONLY task + target — the acting manager is the token subject, never
  // a body field, so a token can only ever assign as itself.
  assert.deepEqual(JSON.parse(init.body as string), {
    task_id: "task_1",
    target_agent_id: "ic",
  });
  // The updated task record flows back to the caller.
  assert.equal(task.status, "Queued");
  assert.equal(task.assigned_agent, "ic");
});

test("agentSelfManagerGrant sends a bearer token, omits the operator cookie, and posts only target+permission", async () => {
  let captured: { url: string; init: RequestInit } | null = null;
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    captured = { url, init };
    return fakeResponse(200, {
      agent_id: "ic",
      permissions: ["tool:relux-tools-echo:say"],
    });
  }) as typeof fetch;

  const perms = await agentSelfManagerGrant(
    "relux_agt_secret123",
    "ic",
    "tool:relux-tools-echo:say",
  );

  assert.ok(captured, "fetch was called");
  const { url, init } = captured!;
  // The real agent-self route — never an operator route.
  assert.equal(url, "/v1/relux/agents/me/manager-grant");
  assert.equal(init.method, "POST");
  // The operator session must NOT be used: credentials are omitted.
  assert.equal(init.credentials, "omit");
  // The bearer token is the credential; the cookie path is not used.
  const headers = init.headers as Record<string, string>;
  assert.equal(headers.authorization, "Bearer relux_agt_secret123");
  assert.equal(headers["content-type"], "application/json");
  // The body carries ONLY target + permission — the acting manager is the token subject,
  // never a body field, so a token can only ever grant as itself.
  assert.deepEqual(JSON.parse(init.body as string), {
    target_id: "ic",
    permission: "tool:relux-tools-echo:say",
  });
  // The target's updated explicit permission list flows back to the caller.
  assert.deepEqual(perms.permissions, ["tool:relux-tools-echo:say"]);
  assert.equal(perms.agent_id, "ic");
});

test("a non-OK manager-grant response throws an honest ApiError and does NOT fire the session-expired signal", async () => {
  globalThis.fetch = (async () =>
    fakeResponse(403, { error: "manager lacks grant_permission scope" })) as typeof fetch;

  let sessionExpiredFired = false;
  const off = onSessionExpired(() => {
    sessionExpiredFired = true;
  });
  try {
    await assert.rejects(
      () => agentSelfManagerGrant("relux_agt_x", "outsider", "tool:relux-tools-echo:say"),
      (e: unknown) => {
        assert.ok(e instanceof ApiError);
        assert.equal((e as ApiError).status, 403);
        assert.match((e as ApiError).message, /grant_permission scope/);
        return true;
      },
    );
  } finally {
    off();
  }
  // A 401/403 on the BEARER path means a bad/expired token, not an operator-session lapse.
  assert.equal(sessionExpiredFired, false);
});

test("a non-OK response throws an honest ApiError and does NOT fire the session-expired signal", async () => {
  globalThis.fetch = (async () =>
    fakeResponse(403, { error: "manager lacks assign_task scope" })) as typeof fetch;

  let sessionExpiredFired = false;
  const off = onSessionExpired(() => {
    sessionExpiredFired = true;
  });
  try {
    await assert.rejects(
      () => agentSelfAssignTask("relux_agt_x", "task_1", "outsider"),
      (e: unknown) => {
        assert.ok(e instanceof ApiError);
        assert.equal((e as ApiError).status, 403);
        assert.match((e as ApiError).message, /assign_task scope/);
        return true;
      },
    );
  } finally {
    off();
  }
  // A 401/403 on the BEARER path means a bad/expired token, not an operator-session lapse
  // — it must not bounce the operator to the login screen.
  assert.equal(sessionExpiredFired, false);
});
