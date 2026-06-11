// Thin fetch wrapper for the Relix web bridge.
//
// Every request rides the HTTP-only `relix_session` cookie via
// `credentials: "include"`, so the dashboard never handles a bearer
// token directly — the bridge auth middleware admits the session.

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

// ── Session-expired signal ────────────────────────────────────────────────
// When a PROTECTED API call comes back 401/403, the operator's session cookie
// has lapsed (or was never minted). Rather than let every page render a broken
// "Could not load …" card, we fire a single signal the AuthProvider listens
// for, so the app can flip back to the login screen with a clear message.
//
// This is a CLIENT-SIDE reaction only — it never makes a protected route
// public; it just routes an honest 401 to the login path instead of a dead end.
type SessionExpiredHandler = () => void;
const sessionExpiredHandlers = new Set<SessionExpiredHandler>();

export function onSessionExpired(cb: SessionExpiredHandler): () => void {
  sessionExpiredHandlers.add(cb);
  return () => {
    sessionExpiredHandlers.delete(cb);
  };
}

function notifySessionExpired(): void {
  for (const cb of sessionExpiredHandlers) {
    try {
      cb();
    } catch {
      /* a misbehaving listener must not break the request path */
    }
  }
}

// The auth endpoints self-gate (a wrong password is a legitimate 401 on the
// login form, NOT an expired session) — never treat them as a lapsed session.
function isAuthPath(path: string): boolean {
  return path.startsWith("/v1/auth/");
}

async function parse(res: Response): Promise<unknown> {
  const text = await res.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

async function request(method: string, path: string, body?: unknown): Promise<unknown> {
  const res = await fetch(path, {
    method,
    credentials: "include",
    headers: body !== undefined ? { "content-type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const data = await parse(res);
  if (!res.ok) {
    // A 401/403 on any non-auth route means the session lapsed — signal the
    // app to reauthenticate instead of leaving the page on a broken card.
    if ((res.status === 401 || res.status === 403) && !isAuthPath(path)) {
      notifySessionExpired();
    }
    const msg =
      (data && typeof data === "object" && "error" in data
        ? String((data as Record<string, unknown>).error)
        : typeof data === "string" && data
          ? data
          : `HTTP ${res.status}`) || `HTTP ${res.status}`;
    throw new ApiError(res.status, msg);
  }
  return data;
}

export const api = {
  get: <T = unknown>(path: string) => request("GET", path) as Promise<T>,
  post: <T = unknown>(path: string, body?: unknown) => request("POST", path, body) as Promise<T>,
  put: <T = unknown>(path: string, body?: unknown) => request("PUT", path, body) as Promise<T>,
  patch: <T = unknown>(path: string, body?: unknown) => request("PATCH", path, body) as Promise<T>,
  del: <T = unknown>(path: string) => request("DELETE", path) as Promise<T>,
};

// ── Current session metadata (`GET /v1/auth/me`) ──────────────────────────
// The signed-in operator + safe session-expiry metadata for the Account control
// (idle/absolute deadlines + remaining seconds; never the session id or hash).
// `/v1/auth/me` is an auth path, so a 401 here is just "not signed in" — it does
// NOT fire the session-expired signal (see `isAuthPath`). Reading it does not
// slide the session, so the Account modal can poll it without keeping an idle
// console alive. Throws an ApiError on 401 so the caller can choose to ignore it.
export interface SessionMetaResponse {
  username: string;
  idle_expires_at?: number;
  absolute_expires_at?: number;
  idle_expires_in_secs?: number;
  absolute_expires_in_secs?: number;
  idle_timeout_secs?: number;
  absolute_max_secs?: number;
  server_now?: number;
  auth_disabled?: boolean;
}

export const session = {
  me: () => api.get<SessionMetaResponse>("/v1/auth/me"),
};

// Best-effort GET that resolves to a fallback instead of throwing, so a
// single unavailable surface degrades to an empty/placeholder state
// rather than blanking the whole page. Use this ONLY for genuinely-optional
// surfaces — for core data prefer `tryGetReport` so a failure is surfaced.
export async function tryGet<T>(path: string, fallback: T): Promise<T> {
  try {
    return (await api.get<T>(path)) ?? fallback;
  } catch {
    return fallback;
  }
}

// Like `tryGet`, but ALSO reports the failure so the page can show an
// explicit error state (a banner + retry) instead of a silent empty panel.
// `status` distinguishes 401/403 (session) from 502/503 (bridge can't reach
// the coordinator) so callers can route the user to the right fix.
export interface GetReport<T> {
  data: T;
  error: string | null;
  status: number | null;
}
export async function tryGetReport<T>(path: string, fallback: T): Promise<GetReport<T>> {
  try {
    const data = (await api.get<T>(path)) ?? fallback;
    return { data, error: null, status: 200 };
  } catch (e) {
    if (e instanceof ApiError) return { data: fallback, error: e.message, status: e.status };
    return { data: fallback, error: e instanceof Error ? e.message : String(e), status: null };
  }
}

// ── Run (Shift) control helpers ───────────────────────────────────────────
// One wiring for the Shift lifecycle (review / apply / cancel + the safe-apply
// plan), shared by the Runs page and the Brief workroom so the same operator
// actions aren't parsed two different ways. All hit the existing `/v1/runs/:id`
// routes the bridge already serves.

// One file in a safe-apply plan (`/v1/runs/:id/diff` → plan.items).
export interface ApplyPlanItem {
  rel_path?: string;
  kind?: string;
  action?: string; // create / overwrite / delete / noop / refuse
  can_apply?: boolean;
  conflict?: boolean;
  reason?: string;
}
export interface ApplyPlan {
  project_root?: string;
  items?: ApplyPlanItem[];
  applicable?: boolean;
  changes?: number;
  conflicts?: number;
  blocked?: number;
  note?: string;
}
// Safe-apply preview (`/v1/runs/:id/diff`).
export interface RunDiff {
  run_id?: string;
  status?: string;
  review?: string;
  apply_status?: string;
  eligible?: boolean;
  reason?: string;
  plan?: ApplyPlan;
}
export interface ApplyResult {
  apply_status?: string;
  applied_files?: number;
  failed_files?: number;
  brief_status?: string;
}

// One transcript event from the durable, capped, redacted `run_events`
// table (`/v1/runs/:id/events`). `kind`/`source` classify the line; `message`
// is the redacted, length-bounded text; `payload_json` is the optional bounded
// detail (e.g. a tool-call's input). Shared by the Runs page and the Brief
// workroom so the same transcript renders identically in both places.
export interface RunEvent {
  event_id?: number;
  ts?: number;
  kind?: string;
  source?: string;
  message?: string;
  payload_json?: string;
}

export const runControls = {
  // The chronological transcript for a run (oldest first). Optional surface →
  // degrades to [] so an unavailable transcript never blanks the embedding view.
  // With `since` (an exclusive `event_id` cursor) it fetches only the new tail —
  // the efficient incremental live-tail the transcript polls while a Shift runs,
  // instead of re-reading the whole transcript on every tick.
  events: (runId: string, since?: number) => {
    const q = since && since > 0 ? `?since=${since}` : "";
    return tryGet<RunEvent[]>(`/v1/runs/${encodeURIComponent(runId)}/events${q}`, []);
  },
  // Record an operator accept/reject of a done run.
  review: (runId: string, decision: "accepted" | "rejected", note = "") =>
    api.post(`/v1/runs/${encodeURIComponent(runId)}/review`, { decision, note }),
  // Copy an accepted run's changed files into the project root.
  apply: (runId: string) =>
    api.post<ApplyResult>(`/v1/runs/${encodeURIComponent(runId)}/apply`, {}),
  // Request cancellation of an in-flight run.
  cancel: (runId: string) =>
    api.post<{ active?: boolean; note?: string }>(
      `/v1/runs/${encodeURIComponent(runId)}/cancel`,
      {},
    ),
  // The safe-apply PLAN for a run (per-file actions + applicability). Optional
  // surface → resolves to null on failure so the panel degrades, not blanks.
  diff: (runId: string) =>
    tryGet<RunDiff | null>(`/v1/runs/${encodeURIComponent(runId)}/diff`, null),
};

// ── Brief thread interactions (answerable cards) ──────────────────────────
// The ask/confirm cards an Operative/companion raises on a Brief
// (relix-execution-and-issue-design §1.9; relix-dashboard-design §7). The
// operator answers them inline; the answer writes a Chronicle event and
// flips the card's status. All hit `/v1/spine/briefs/:id/interactions`.

// One proposed child Brief inside a `suggest_tasks` card.
export interface SuggestChild {
  title: string;
  priority?: string | null;
  // Optional intra-proposal dependency: the 0-based index of an earlier
  // sibling this child depends on (§1.6). On accept it becomes a Snag
  // (blocked_on) — the referenced sibling must reach `done` first.
  after?: number | null;
  // Optional explicit assignee hint (§1.9). Mutually exclusive: a child
  // names an Operative by id (precise) OR by role (resolved to the oldest
  // active same-role Operative), never both. On accept the hint is
  // validated through the existing assign-Key gate (same-Guild, active)
  // and the child is assigned; absent ⇒ the child opens unassigned.
  assignee_agent_id?: string | null;
  assignee_role?: string | null;
}

// The bounded proposal a `suggest_tasks` card carries.
export interface BriefProposal {
  summary: string;
  children: SuggestChild[];
}

export interface BriefInteraction {
  interaction_id: string;
  task_id: string;
  kind: string; // ask | confirm | suggest_tasks
  prompt: string;
  choices: string[];
  author: string;
  status: string; // open | resolved | rejected | expired
  response?: string | null;
  created_at?: number;
  resolved_at?: number | null;
  resolved_by?: string | null;
  // Present only on `suggest_tasks` cards.
  proposal?: BriefProposal | null;
  // Approval-bound plan confirm (§1.8): when this `confirm` was opened against
  // a specific `plan` Dossier revision, the bound Dossier id (which IS the
  // revision — Dossiers are immutable) and its kind (`plan`). Present only on a
  // bound confirm; an accept after the plan changed (newer revision or a
  // superseding comment) is refused server-side and the card flips to
  // `expired`.
  bound_doc_id?: string | null;
  bound_doc_kind?: string | null;
  // Plan package (§1.7/§1.8/§3.1): when this `confirm` was opened as part of a
  // plan package, the exact linked `suggest_tasks` interaction id it gates.
  // Present only on a plan-package confirm; accepting such a confirm must go
  // through the safe `briefPlanConfirms.respond` path (not the generic
  // interaction respond) so approval materializes the linked proposal exactly
  // once through the decomposition ledger.
  bound_interaction_id?: string | null;
}

export const briefInteractions = {
  // List a Brief's cards (oldest first). Optional surface → degrades to []
  // so a Brief with no interactions (or a bridge hiccup) never blanks.
  list: (briefId: string) =>
    tryGet<BriefInteraction[]>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/interactions`,
      [],
    ),
  // Raise a new card (used by agents/companion; exposed for completeness).
  open: (
    briefId: string,
    body: { kind: "ask" | "confirm"; prompt: string; choices?: string[]; author: string },
  ) =>
    api.post<{ interaction_id: string }>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/interactions`,
      body,
    ),
  // Answer a card. `status` is the terminal verdict; a duplicate answer
  // surfaces as a typed 400 (ApiError).
  respond: (
    briefId: string,
    interactionId: string,
    body: { responder: string; status: "resolved" | "rejected"; response?: string },
  ) =>
    api.post(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/interactions/${encodeURIComponent(
        interactionId,
      )}/respond`,
      body,
    ),
  // Open an approval-bound plan confirm (§1.8): a `confirm` card bound to the
  // Brief's latest `plan` Dossier revision. The route refuses (4xx) when the
  // Brief has no `plan` Dossier — the caller surfaces that honestly. `author`
  // defaults to the bridge identity when omitted. The card lists/answers
  // through the same interaction routes; an accept after the plan changed is
  // refused as stale and the card flips to `expired`.
  openPlanConfirm: (
    briefId: string,
    body: { author?: string; prompt?: string },
  ) =>
    api.post<{ interaction_id: string }>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/plan-confirm`,
      body,
    ),
};

// ── Brief suggest_tasks cards (proposed child-Brief trees) ────────────────
// An Operative proposes a bounded list of child Briefs on a Brief
// (relix-execution-and-issue-design §1.9). The operator accepts — which
// materializes them as real Sub-briefs — or rejects. The cards list through
// the same `briefInteractions.list` (kind `suggest_tasks`, with a `proposal`).
export const briefSuggestions = {
  // Raise a new suggestion (used by agents/companion; exposed for completeness).
  open: (
    briefId: string,
    body: { author: string; summary?: string; children: SuggestChild[] },
  ) =>
    api.post<{ interaction_id: string }>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/suggestions`,
      body,
    ),
  // Accept (materialize the child Briefs) or reject a suggestion. Accept
  // returns the created child ids; a duplicate answer surfaces as a typed 400.
  respond: (
    briefId: string,
    interactionId: string,
    body: { responder: string; accept: boolean },
  ) =>
    api.post<{ created: string[] }>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/suggestions/${encodeURIComponent(
        interactionId,
      )}/respond`,
      body,
    ),
};

// ── Plan-package confirms (approval-bound, linked to a proposal) ───────────
// A plan package (relix-execution-and-issue-design §1.7/§1.8/§3.1) opens a
// `plan` Dossier + a `suggest_tasks` proposal + an approval-bound `confirm`
// linked to both (the confirm carries `bound_interaction_id`). Accepting the
// confirm must use this safe route, not the generic interaction respond, so the
// linked proposal materializes exactly once through the resumable decomposition
// ledger; rejecting closes the confirm and its still-open proposal.
export const briefPlanConfirms = {
  // Open a plan package. Returns the three artifact ids. (Used by the
  // companion and by the workroom's minimal manual plan-package composer.)
  open: (
    briefId: string,
    body: {
      author: string;
      plan_title?: string;
      plan_body: string;
      summary?: string;
      children: SuggestChild[];
      prompt?: string;
    },
  ) =>
    api.post<{ plan_doc_id: string; suggestion_id: string; confirm_id: string }>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/plan-package`,
      body,
    ),
  // Accept (re-check the plan is latest, then materialize the linked proposal)
  // or reject a plan-package confirm. Returns the typed outcome + created child
  // ids. A duplicate accept is idempotent and returns the SAME ids.
  respond: (
    briefId: string,
    confirmId: string,
    body: { responder: string; accept: boolean },
  ) =>
    api.post<{ outcome: string; suggestion_id: string; created: string[] }>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/plan-confirms/${encodeURIComponent(
        confirmId,
      )}/respond`,
      body,
    ),
};

// ── Brief Dossiers — issue documents (author / revision-lock / fork) ──────
// A Dossier is a durable, append-only artifact on a Brief (plan/design/notes;
// relix-execution-and-issue-design §1.8). v1 authoring: revise under an
// optimistic lock, or explicitly fork a new line from a stale/base revision.
// `revision_number` is derived (1-based within Brief+kind, oldest first).

// A Dossier listing row (metadata only, no body) as carried on the Brief
// detail's `dossiers` array.
export interface DossierMeta {
  doc_id: string;
  kind: string;
  title: string;
  created_at?: number;
  updated_at?: number;
  author?: string | null;
  revision_of_doc_id?: string | null;
  forked_from_doc_id?: string | null;
  revision_number?: number;
}

// A full Dossier (with body) — returned by the latest-load route.
export interface Dossier extends DossierMeta {
  task_id: string;
  body: string;
}

// The successful-author result (mirrors the coordinator's DossierAuthored).
export interface DossierAuthored {
  doc_id: string;
  task_id: string;
  kind: string;
  title: string;
  author?: string | null;
  mode: "create" | "revise" | "fork";
  revision_number: number;
  revision_of_doc_id?: string | null;
  forked_from_doc_id?: string | null;
}

export const briefDossiers = {
  // Load the latest revision of a kind (full body + metadata), or `null` when
  // the Brief has no Dossier of that kind. The editor keeps the returned
  // `doc_id` as the optimistic-lock base for the next save.
  latest: (briefId: string, kind: string) =>
    api.get<Dossier | null>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/dossiers/latest?kind=${encodeURIComponent(
        kind,
      )}`,
    ),
  // Author a Dossier revision. `mode` defaults to `revise`; pass
  // `expected_latest_doc_id` to enforce the optimistic lock (a stale base — a
  // newer revision landed first — rejects with **HTTP 409**, nothing written:
  // the caller reloads or forks, and must NOT retry the 409 blindly). `mode:
  // "fork"` branches a new line from `base_doc_id` even if the latest moved.
  author: (
    briefId: string,
    body: {
      kind: string;
      title: string;
      body: string;
      author: string;
      mode?: "revise" | "fork";
      expected_latest_doc_id?: string;
      base_doc_id?: string;
    },
  ) =>
    api.post<DossierAuthored>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/dossiers/author`,
      body,
    ),
};

// ── Brief-tree cost rollup (brief.cost_rollup) ────────────────────────────
// The §6.6 issue-tree cost rollup: sum the durable `brief_runs` ledger over a
// Brief AND its same-Guild Sub-brief tree, with own-vs-descendant totals, tree
// counts, and a per-billing-code breakdown (dashboard-design §10;
// company-model §6.6). All figures are REAL run cost — micro-USD from the
// ledger, never UI data. Windowed on the canonical Allowance month unless
// since/until (unix SECONDS) are supplied. Hits `GET /v1/spine/briefs/:id/cost`.

// One billing-code's slice of a Brief-tree's cost. `billing_code:""` = unattributed.
export interface BillingCodeCost {
  billing_code: string;
  run_count: number;
  cost_micros: number;
}

export interface BriefCostRollup {
  brief_id: string;
  tenant_id: string;
  // Resolved window the rollup billed against (unix SECONDS).
  since_secs: number;
  until_secs: number;
  // Whole same-Guild tree (root Brief + descendants).
  brief_count: number;
  run_count: number;
  cost_micros: number;
  // Just the root Brief.
  own_run_count: number;
  own_cost_micros: number;
  // Descendant Sub-briefs (= tree − own).
  descendant_run_count: number;
  descendant_cost_micros: number;
  by_billing_code: BillingCodeCost[];
}

// ── Canonical Guild month-to-date spend (guild.spend) ─────────────────────
// THE numeric Guild spend the Costs page reads (company-model §6.6;
// dashboard-design §10). NOT a dashboard-only approximation: it is the EXACT
// ledger figure + UTC-calendar-month window the autonomous Guild hard-stop
// enforces (`heartbeat::guild_spend_micros` over `heartbeat::allowance_window`),
// so the card can never disagree with the gate. Hits `GET /v1/spine/guild/spend`.
//
// `spent_*` are null when no metrics ledger is wired (spend can't be computed
// honestly — never a fabricated 0). The `budget_cents`/`remaining_cents`/
// `over_budget` triplet is null when no positive Guild budget is configured.
export interface GuildSpend {
  tenant_id: string;
  guild_id: string;
  display_name: string | null;
  // Exact integer micro-USD (1,000,000 micros = $1) + a rounded cents view.
  spent_micros: number | null;
  spent_cents: number | null;
  // Configured Guild budget + remaining (cents); null when no budget is set.
  budget_cents: number | null;
  remaining_cents: number | null;
  over_budget: boolean | null;
  // Canonical Allowance window (UTC calendar month) + reset bookkeeping (unix-ms).
  window_start_ms: number;
  resets_at_ms: number;
  now_ms: number;
  source: string;
  computed_from: string;
}

export const guildSpend = {
  // Canonical month-to-date Guild spend. Reports the failure (via
  // `tryGetReport`) so the Costs card shows an honest unavailable state with the
  // route/reason instead of falling back to a fabricated/approximated figure.
  get: () => tryGetReport<GuildSpend | null>("/v1/spine/guild/spend", null),
};

export const briefCost = {
  // The Brief-tree rollup. `since`/`until` are unix SECONDS — omit both for the
  // canonical current-calendar-month window the dispatch gate uses. Reports the
  // failure (via `tryGetReport`) so the Costs page shows an honest unavailable
  // state with the route/reason instead of fabricated zeroes.
  rollup: (briefId: string, since?: number, until?: number) => {
    const qs = new URLSearchParams();
    if (since != null) qs.set("since", String(since));
    if (until != null) qs.set("until", String(until));
    const q = qs.toString();
    return tryGetReport<BriefCostRollup | null>(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/cost${q ? `?${q}` : ""}`,
      null,
    );
  },
};

// ── Live run-event stream (SSE) ───────────────────────────────────────────
// Subscribe to the bridge's `/v1/runs/events/stream` execution feed so the
// Runs page + Brief detail can refresh the moment a Shift starts, finishes,
// is refused, recovered, moved, reviewed, or applied — instead of only at
// fetch time. Cookie auth rides the same-origin EventSource automatically.

export type RunEventConn = "connecting" | "live" | "reconnecting" | "unavailable";

export interface RunStreamEvent {
  // Normalized SSE event name: run_started | run_finished |
  // run_cancel_requested | brief_moved | review_changed | apply_changed.
  name: string;
  // The Brief (task) id carried by the event, when present.
  taskId: string | null;
}

const RUN_EVENT_NAMES = [
  "run_started",
  "run_finished",
  "run_cancel_requested",
  "brief_moved",
  "review_changed",
  "apply_changed",
];

// Open the stream and call `onEvent` per execution transition + `onConn` on
// connection-state changes. Manages reconnect with capped backoff and reports
// honest state (live / reconnecting / unavailable). Returns an unsubscribe fn.
export function subscribeRunEvents(
  onEvent: (ev: RunStreamEvent) => void,
  onConn: (state: RunEventConn) => void,
): () => void {
  let es: EventSource | null = null;
  let closed = false;
  let attempts = 0;
  let backoff = 1000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const handler = (name: string) => (e: MessageEvent) => {
    let taskId: string | null = null;
    try {
      const j = JSON.parse(e.data);
      if (j && typeof j === "object" && "task_id" in j && j.task_id != null) {
        taskId = String((j as Record<string, unknown>).task_id);
      }
    } catch {
      /* non-JSON frame — forward with no taskId */
    }
    onEvent({ name, taskId });
  };

  const connect = () => {
    if (closed) return;
    onConn(attempts === 0 ? "connecting" : "reconnecting");
    es = new EventSource("/v1/runs/events/stream", { withCredentials: true });
    es.onopen = () => {
      attempts = 0;
      backoff = 1000;
      onConn("live");
    };
    for (const n of RUN_EVENT_NAMES) {
      es.addEventListener(n, handler(n) as EventListener);
    }
    es.onerror = () => {
      // The browser would auto-reconnect, but we manage it so we can surface
      // honest state + cap reconnect storms. Persistent failure → unavailable
      // (still retrying, so it can recover to live).
      es?.close();
      es = null;
      if (closed) return;
      attempts += 1;
      onConn(attempts >= 3 ? "unavailable" : "reconnecting");
      timer = setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
  };

  connect();
  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    es?.close();
    es = null;
  };
}

// ── Dedicated Active Runs snapshot stream (SSE) ────────────────────────────
// Subscribe to the bridge's `/v1/runs/stream` feed so the Active Runs table
// refreshes the moment the recent-run ledger changes — a run started, finished,
// refused, reviewed, applied, or retried — instead of re-fetching on each
// run-event. Tenant-scoped server-side (it proxies the SAME `brief.runs` read
// the `/v1/runs` list route serves); polling-backed (~2.5s) + fingerprint-gated,
// so an unchanged ledger pushes nothing. Honest polling-backed SSE — NOT a true
// event bus/websocket. Cookie auth rides the same-origin EventSource. The page
// falls back to its mount-load + manual Refresh whenever this never reaches
// `live` (see Runs.tsx).

export type RunsStreamConn = "connecting" | "live" | "reconnecting" | "unavailable";

// Open the runs snapshot stream. `onRuns` receives the full recent-run array on
// the initial snapshot + every change (same shape as `GET /v1/runs`, truncated
// to `limit`); `onConn` reports honest connection state. Manages reconnect with
// capped backoff. Returns an unsubscribe fn.
export function subscribeRuns(
  onRuns: (runs: unknown[]) => void,
  onConn: (state: RunsStreamConn) => void,
  limit = 100,
): () => void {
  let es: EventSource | null = null;
  let closed = false;
  let attempts = 0;
  let backoff = 1000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (closed) return;
    onConn(attempts === 0 ? "connecting" : "reconnecting");
    es = new EventSource(`/v1/runs/stream?limit=${limit}`, { withCredentials: true });
    es.onopen = () => {
      attempts = 0;
      backoff = 1000;
      onConn("live");
    };
    es.addEventListener("runs", (e: MessageEvent) => {
      try {
        const arr = JSON.parse(e.data);
        if (Array.isArray(arr)) onRuns(arr as unknown[]);
      } catch {
        /* malformed frame — ignore, the next snapshot corrects it */
      }
    });
    // NB: the server's transient `event: error` frames just precede the next
    // snapshot; EventSource's own connection `error` is handled by `onerror`.
    es.onerror = () => {
      es?.close();
      es = null;
      if (closed) return;
      attempts += 1;
      onConn(attempts >= 3 ? "unavailable" : "reconnecting");
      timer = setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
  };

  connect();
  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    es?.close();
    es = null;
  };
}

// ── Prime guided driver v1 (next governed step + one-step advance) ──────────
// The READ-ONLY next step for a Prime work session, plus the bounded one-step
// advance. The advance runs AT MOST ONE safe governed step through the existing
// gated route; a stale request returns HTTP 409 (re-read and try again). It
// never auto-approves a strategy / hire / spawn / budget gate.

export interface PrimeNextStep {
  phase: string;
  label: string;
  reason: string;
  route: string;
  action_api: string;
  can_advance: boolean;
  advance_action: string | null;
  proposal_id: string | null;
  mandate_id: string | null;
  plan_id: string | null;
  strategy_status: string | null;
  missing_roles: string[];
  pending_hires: unknown[];
  pending_clearances: unknown[];
  counts: Record<string, number>;
}

export interface PrimeAdvanceResult {
  advanced: boolean;
  refused?: string;
  requested_action?: string;
  action?: string;
  reason?: string;
  mandate_id?: string;
  result?: unknown;
  next_step: PrimeNextStep | null;
}

export const primeDriver = {
  nextStep: (proposalId: string) =>
    api.get<PrimeNextStep>(
      `/v1/spine/prime/proposals/${encodeURIComponent(proposalId)}/next-step`,
    ),
  advance: (proposalId: string, action: string) =>
    api.post<PrimeAdvanceResult>(
      `/v1/spine/prime/proposals/${encodeURIComponent(proposalId)}/advance`,
      { action },
    ),
  // Mandate-level twins of the two above — the SAME guided-driver routes, keyed
  // by a Mandate id instead of a proposal id (company-model §5.4/§8.2 + §12.5;
  // bridge `mandate_next_step` / `mandate_advance`). Same shapes, same
  // guarantees: `nextStep` is READ-ONLY; a stale one-step `advance` returns
  // HTTP 409 (re-read and try again, never retry the 409 blindly); the driver
  // never auto-approves a strategy / hire / spawn / budget gate.
  mandateNextStep: (mandateId: string) =>
    api.get<PrimeNextStep>(
      `/v1/spine/mandates/${encodeURIComponent(mandateId)}/next-step`,
    ),
  mandateAdvance: (mandateId: string, action: string) =>
    api.post<PrimeAdvanceResult>(
      `/v1/spine/mandates/${encodeURIComponent(mandateId)}/advance`,
      { action },
    ),
};

// ── Dedicated Prime Shift-Room status stream (SSE) ─────────────────────────
// Subscribe to the bridge's dedicated `/v1/spine/prime/proposals/:id/status/
// stream` feed so the Shift Room renders the live session status pushed by the
// server (initial snapshot + on every change), instead of polling. Cookie auth
// rides the same-origin EventSource. Falls back to polling at the call site
// whenever this never reaches `live`.

export type StatusStreamConn = "connecting" | "live" | "reconnecting" | "unavailable";

// Open the dedicated status stream for one proposal. `onStatus` receives the
// full session-status JSON on the initial snapshot + every change; `onConn`
// reports honest connection state; `onGone` fires once when the server emits a
// terminal `not_found` (the proposal is unknown / cross-Guild). Manages
// reconnect with capped backoff. Returns an unsubscribe fn.
export function subscribePrimeStatus(
  proposalId: string,
  onStatus: (status: unknown) => void,
  onConn: (state: StatusStreamConn) => void,
  onGone?: () => void,
): () => void {
  let es: EventSource | null = null;
  let closed = false;
  let attempts = 0;
  let backoff = 1000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (closed) return;
    onConn(attempts === 0 ? "connecting" : "reconnecting");
    es = new EventSource(`/v1/spine/prime/proposals/${proposalId}/status/stream`, {
      withCredentials: true,
    });
    es.onopen = () => {
      attempts = 0;
      backoff = 1000;
      onConn("live");
    };
    es.addEventListener("status", (e: MessageEvent) => {
      try {
        onStatus(JSON.parse(e.data));
      } catch {
        /* malformed frame — ignore, the next snapshot corrects it */
      }
    });
    // Terminal: the proposal is gone / cross-Guild. Stop cleanly — no reconnect.
    es.addEventListener("not_found", () => {
      closed = true;
      es?.close();
      es = null;
      onConn("unavailable");
      onGone?.();
    });
    // NB: we intentionally do NOT listen for a custom `error` event — the
    // server's transient `event: error` frames just precede the next snapshot,
    // and EventSource's own connection `error` is handled by `onerror` below.
    es.onerror = () => {
      es?.close();
      es = null;
      if (closed) return;
      attempts += 1;
      onConn(attempts >= 3 ? "unavailable" : "reconnecting");
      timer = setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
  };

  connect();
  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    es?.close();
    es = null;
  };
}

// ── Dedicated Brief interaction-card stream (SSE) ──────────────────────────
// Subscribe to the bridge's dedicated `/v1/spine/briefs/:id/interactions/
// stream` feed so the Brief workroom's ask/confirm/suggest/plan-package cards
// refresh the moment the card list changes — a card raised, answered, or
// superseded — even when NO run event fires (the run-event stream above misses
// those). Tenant-scoped server-side (it proxies the same `brief.interactions`
// read the list route serves); polling-backed (~2.5s) + fingerprint-gated, so
// an unchanged list pushes nothing. Cookie auth rides the same-origin
// EventSource. Complements `subscribeRunEvents` — it does not replace it.

export type InteractionStreamConn = "connecting" | "live" | "reconnecting" | "unavailable";

// Open the interaction stream for one Brief. `onInteractions` receives the full
// card array on the initial snapshot + every change (same shape as
// `briefInteractions.list`); `onConn` reports honest connection state; `onGone`
// fires once when the server emits a terminal `not_found` (the Brief is unknown
// / cross-Guild). Manages reconnect with capped backoff. Returns an unsubscribe.
export function subscribeBriefInteractions(
  briefId: string,
  onInteractions: (interactions: BriefInteraction[]) => void,
  onConn: (state: InteractionStreamConn) => void,
  onGone?: () => void,
): () => void {
  let es: EventSource | null = null;
  let closed = false;
  let attempts = 0;
  let backoff = 1000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (closed) return;
    onConn(attempts === 0 ? "connecting" : "reconnecting");
    es = new EventSource(
      `/v1/spine/briefs/${encodeURIComponent(briefId)}/interactions/stream`,
      { withCredentials: true },
    );
    es.onopen = () => {
      attempts = 0;
      backoff = 1000;
      onConn("live");
    };
    es.addEventListener("interactions", (e: MessageEvent) => {
      try {
        const arr = JSON.parse(e.data);
        if (Array.isArray(arr)) onInteractions(arr as BriefInteraction[]);
      } catch {
        /* malformed frame — ignore, the next snapshot corrects it */
      }
    });
    // Terminal: the Brief is gone / cross-Guild. Stop cleanly — no reconnect.
    es.addEventListener("not_found", () => {
      closed = true;
      es?.close();
      es = null;
      onConn("unavailable");
      onGone?.();
    });
    // NB: the server's transient `event: error` frames just precede the next
    // snapshot; EventSource's own connection `error` is handled by `onerror`.
    es.onerror = () => {
      es?.close();
      es = null;
      if (closed) return;
      attempts += 1;
      onConn(attempts >= 3 ? "unavailable" : "reconnecting");
      timer = setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
  };

  connect();
  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    es?.close();
    es = null;
  };
}

// ── Approvals hub (Clearances + the governance action feed) ───────────────
// The operator-decision surface (dashboard-design §10). Two REAL backends:
//
//  1. Pending Clearances — `GET /v1/spine/clearances` (the unified
//     `coord.approval.pending` queue: spawn-hire Clearances, strategy gates,
//     budget overrides, high-risk approvals — distinguished by `method`).
//     Decided inline via `POST /v1/spine/clearances/:id/decide`, which forwards
//     to `coord.approval.decide` under the bridge's verified identity (the
//     runtime cap enforces the real authorisation — never fabricated here).
//
//  2. Direct pending hires + budget alerts — the `hire`/`budget` items in the
//     `GET /v1/spine/company/actions` feed (a `route=direct` hire carries no
//     Clearance, so it is activated via `POST /v1/agents/:id/approve-hire`).
//
// No other approval type has a decide route today, so the hub surfaces those as
// honest "decide on <route>" pointers rather than a fake inline action.

// One pending Clearance row (the bridge parses `coord.approval.pending`'s TSV).
// `requested_at`/`expires_at` arrive as string columns (unix seconds); coerce at
// render. The typed fields (`subject_id`, `capability_category`, `expires_at`,
// `task_id`) are surfaced verbatim from the runtime approval row — an empty
// string means the runtime did not record that field for this Clearance (treat
// as absent). Nothing is fabricated; there is no free-form resource/scope/
// payload editor field because the runtime does not store one.
export interface Clearance {
  approval_id: string;
  agent_id: string;
  method: string;
  reason: string;
  requested_at: string;
  // Typed payload fields (optional / possibly empty — see above).
  subject_id?: string;
  capability_category?: string;
  expires_at?: string;
  task_id?: string;
}

export const clearances = {
  // Pending Clearances (best-effort report so the hub shows an honest
  // unavailable state with the route/reason instead of a blank panel).
  list: (limit = 50) =>
    tryGetReport<Clearance[]>(`/v1/spine/clearances?limit=${limit}`, []),
  // Greenlight / refuse a Clearance. `decision` is `approve`|`reject`; the
  // runtime refuses to re-decide an already-terminal approval, so side effects
  // (e.g. activating a spawn hire) apply exactly once.
  decide: (approvalId: string, decision: "approve" | "reject", note = "") =>
    api.post<{ ok: boolean; approval_id: string; decision: string; approval_token?: string }>(
      `/v1/spine/clearances/${encodeURIComponent(approvalId)}/decide`,
      { decision, note },
    ),
};

// ── Dedicated pending-Clearance stream (SSE) ──────────────────────────────
// Subscribe to the bridge's `/v1/spine/clearances/stream` feed so the Approvals
// hub refreshes the moment the pending queue changes — a Clearance raised,
// decided, or expired — instead of only on manual Refresh. Tenant-scoped
// server-side (it proxies the SAME `coord.approval.pending` read the list route
// serves); polling-backed (~2.5s) + fingerprint-gated, so an unchanged queue
// pushes nothing. Honest polling-backed SSE — NOT a true event bus/websocket.
// Cookie auth rides the same-origin EventSource. The page falls back to bounded
// polling whenever this never reaches `live` (see Approvals.tsx).

export type ClearanceStreamConn = "connecting" | "live" | "reconnecting" | "unavailable";

// Open the Clearance stream. `onClearances` receives the full pending array on
// the initial snapshot + every change (same shape as `clearances.list`'s data);
// `onConn` reports honest connection state. Manages reconnect with capped
// backoff. Returns an unsubscribe fn.
export function subscribeClearances(
  onClearances: (clearances: Clearance[]) => void,
  onConn: (state: ClearanceStreamConn) => void,
): () => void {
  let es: EventSource | null = null;
  let closed = false;
  let attempts = 0;
  let backoff = 1000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (closed) return;
    onConn(attempts === 0 ? "connecting" : "reconnecting");
    es = new EventSource("/v1/spine/clearances/stream?limit=50", { withCredentials: true });
    es.onopen = () => {
      attempts = 0;
      backoff = 1000;
      onConn("live");
    };
    es.addEventListener("clearances", (e: MessageEvent) => {
      try {
        const arr = JSON.parse(e.data);
        if (Array.isArray(arr)) onClearances(arr as Clearance[]);
      } catch {
        /* malformed frame — ignore, the next snapshot corrects it */
      }
    });
    // NB: the server's transient `event: error` frames just precede the next
    // snapshot; EventSource's own connection `error` is handled by `onerror`.
    es.onerror = () => {
      es?.close();
      es = null;
      if (closed) return;
      attempts += 1;
      onConn(attempts >= 3 ? "unavailable" : "reconnecting");
      timer = setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
  };

  connect();
  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    es?.close();
    es = null;
  };
}

// One row of the server-computed `company.actions` feed (company-model §5.4 /
// §8.2). Read-only; the Approvals hub consumes only the `hire`/`budget` items
// (the `approval` items duplicate the Clearance list, which has the real
// decide route + the true approval_id).
export interface CompanyActionItem {
  id?: string;
  category?: string;
  severity?: string;
  title?: string;
  reason?: string;
  target_type?: string;
  target_id?: string;
  target_title?: string;
  action_label?: string;
  route?: string;
  action_api?: string;
  suggested_rig?: string;
  // Recovery diagnosis (execution-and-issue §3.3b) — set on `failed_or_refused`
  // cards built from a run's durable diagnosis.
  failure_class?: string;
  retryable?: boolean;
  retry_budget_remaining?: number;
  // Guarded retry target — set on a `failed_or_refused` card ONLY when the
  // source run is retry-eligible (retryable + budget + no existing child). Pairs
  // with `action_api` = `POST /v1/runs/<run_id>/retry` so the Action Center can
  // open one guarded retry directly. Absent when not safely retryable from here.
  run_id?: string;
}
export interface CompanyActionsFeed {
  actions?: CompanyActionItem[];
  counts?: { total?: number; by_category?: Record<string, number>; by_severity?: Record<string, number> };
  truncated?: boolean;
}

export const companyActions = {
  list: () => tryGetReport<CompanyActionsFeed | null>("/v1/spine/company/actions", null),
};

// ── Dedicated Action Center snapshot stream (SSE) ──────────────────────────
// Subscribe to the bridge's `/v1/spine/company/actions/stream` feed so the
// Command Center action feed refreshes the moment it changes — an approval,
// hire, blocker, needs-review, or recovery card appears or clears — instead of
// only on the run-event trigger / 20s poll. Tenant-scoped server-side (it
// proxies the SAME `company.actions` read the list route serves); polling-backed
// (~2.5s) + fingerprint-gated, so an unchanged feed pushes nothing. Honest
// polling-backed SSE — NOT a true event bus/websocket. Cookie auth rides the
// same-origin EventSource. The page keeps its bounded poll + invalidation-bus
// fallback so the feed still converges when this never reaches `live`.

export type CompanyActionsConn = "connecting" | "live" | "reconnecting" | "unavailable";

// Open the actions snapshot stream. `onActions` receives the full feed on the
// initial snapshot + every change (same shape as `GET /v1/spine/company/actions`);
// `onConn` reports honest connection state. Manages reconnect with capped
// backoff. Returns an unsubscribe fn.
export function subscribeCompanyActions(
  onActions: (feed: CompanyActionsFeed) => void,
  onConn: (state: CompanyActionsConn) => void,
): () => void {
  let es: EventSource | null = null;
  let closed = false;
  let attempts = 0;
  let backoff = 1000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (closed) return;
    onConn(attempts === 0 ? "connecting" : "reconnecting");
    es = new EventSource("/v1/spine/company/actions/stream", { withCredentials: true });
    es.onopen = () => {
      attempts = 0;
      backoff = 1000;
      onConn("live");
    };
    es.addEventListener("actions", (e: MessageEvent) => {
      try {
        const feed = JSON.parse(e.data);
        if (feed && typeof feed === "object") onActions(feed as CompanyActionsFeed);
      } catch {
        /* malformed frame — ignore, the next snapshot corrects it */
      }
    });
    es.onerror = () => {
      es?.close();
      es = null;
      if (closed) return;
      attempts += 1;
      onConn(attempts >= 3 ? "unavailable" : "reconnecting");
      timer = setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
  };

  connect();
  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    es?.close();
    es = null;
  };
}

// ── Adapter runtime state (admin / session recovery) ──────────────────────
// The persisted adapter runtime rows for ONE Operative (resumable session id,
// accumulated usage/cost, last run status) — the recovery pointer the Settings
// hub exposes (dashboard-design §10). Tenant-scoped, newest first. Reset forgets
// the rows (optionally scoped to one Brief) so a wedged resumable session can be
// cleared without touching the durable run ledger.
export interface RuntimeStateRow {
  agent_id?: string;
  rig?: string;
  brief_key?: string;
  session_id?: string;
  provider?: string;
  model?: string;
  input_tokens?: number;
  output_tokens?: number;
  cost_micros?: number;
  last_run_id?: string;
  last_status?: string;
  last_error?: string;
  updated_at?: number;
  [k: string]: unknown;
}

export const runtimeState = {
  // Global recovery list: every persisted adapter session in the Guild across
  // ALL Operatives, newest first (`GET /v1/runs/runtime-state/list`). Reports
  // failure so the panel shows the bridge's own error, not a blank.
  list: (limit?: number) =>
    tryGetReport<{ rows?: RuntimeStateRow[] } | RuntimeStateRow[] | null>(
      `/v1/runs/runtime-state/list${limit ? `?limit=${limit}` : ""}`,
      null,
    ),
  // Per-agent lookup (the route requires an agent_id). Reports failure so the
  // panel shows the bridge's own error, not a blank.
  get: (agentId: string) =>
    tryGetReport<RuntimeStateRow[] | { rows?: RuntimeStateRow[] } | null>(
      `/v1/runs/runtime-state?agent_id=${encodeURIComponent(agentId)}`,
      null,
    ),
  // Forget persisted runtime state for one agent (optionally one Brief).
  reset: (agentId: string, briefKey?: string) =>
    api.post<{ removed?: number }>("/v1/runs/runtime-state/reset", {
      agent_id: agentId,
      ...(briefKey ? { brief_key: briefKey } : {}),
    }),
};

// ── Operative Skills — procedural memory (read-only) ──────────────────────
// The `memory.skill_*` catalogue, surfaced read-only on the Operative
// workbench (relix-dashboard-design §9 — the agent detail includes Skills).
// The bridge proxies `GET /v1/skills` → `memory.skill_search` on the AI node;
// when the `[skills]` capability is DISABLED the bridge replies with the calm
// shell `{ available:false, reason }` (HTTP 200) instead of a 502, so the panel
// renders an honest unavailable state. Every field is optional — the search
// SUMMARY carries id/name/description/source_agent/confidence/usage_count/
// version/tags/status (no timestamps), but the helper stays defensive so a
// thinner OR fuller row still renders.
export interface SkillSummary {
  id?: string;
  name?: string;
  title?: string;
  description?: string;
  summary?: string;
  body?: string;
  source_agent?: string;
  agent?: string;
  source?: string;
  confidence?: number;
  usage_count?: number;
  version?: number;
  status?: string;
  tags?: string[];
  // Timestamps are not on the search summary today, but accepted defensively
  // (seconds or `_ms`) so a future/fuller shape renders without a change here.
  updated_at?: number;
  updated_at_ms?: number;
  created_at?: number;
  created_at_ms?: number;
  [k: string]: unknown;
}

// The parsed skills-search outcome: either the bridge's `{available:false}`
// shell, or a list pulled defensively from `results`/`items`/`skills`/`rows`/
// a raw array.
export interface SkillSearchResult {
  available: boolean;
  reason: string | null;
  items: SkillSummary[];
}

// Normalize whatever the skills route returned into a `SkillSearchResult`.
// Recognizes the unavailable shell, the documented `{results}` list, the other
// common list keys, and a bare array; anything else degrades to empty.
function parseSkillResult(raw: unknown): SkillSearchResult {
  if (Array.isArray(raw)) {
    return { available: true, reason: null, items: raw as SkillSummary[] };
  }
  if (raw && typeof raw === "object") {
    const o = raw as Record<string, unknown>;
    if (o.available === false) {
      return {
        available: false,
        reason: typeof o.reason === "string" ? o.reason : null,
        items: [],
      };
    }
    for (const k of ["results", "items", "skills", "rows", "list"]) {
      if (Array.isArray(o[k])) {
        return { available: true, reason: null, items: o[k] as SkillSummary[] };
      }
    }
  }
  return { available: true, reason: null, items: [] };
}

export const skills = {
  // Search the catalogue for skills relevant to one Operative. `agent` scopes
  // to the Operative's id; a trimmed `q` adds a substring match. Never throws —
  // a bridge/route failure degrades to an empty (available) result so the tab
  // shows a calm empty state, while the backend's explicit `{available:false}`
  // is surfaced verbatim.
  search: async (opts: {
    agent?: string;
    q?: string;
    limit?: number;
  }): Promise<SkillSearchResult> => {
    const qs = new URLSearchParams();
    const q = opts.q?.trim();
    if (q) qs.set("q", q);
    if (opts.agent) qs.set("agent", opts.agent);
    qs.set("limit", String(opts.limit ?? 20));
    const raw = await tryGet<unknown>(`/v1/skills?${qs.toString()}`, null);
    return parseSkillResult(raw);
  },
  // Aggregate catalogue counts (optional context — degrades to null).
  stats: () => tryGet<unknown>("/v1/skills/stats", null),
};

// Outcome of probing one health dimension. `status` is the HTTP code (null
// when the request never reached the bridge — a network/DNS/TLS failure).
// -- Relux plugins (the /v1/relux plugin API server) -----------------------
// These talk to the local `relux-kernel serve` process (default
// 127.0.0.1:19891), routed by the dev proxy's `/v1/relux` rule (and, in a
// hosted setup, by whatever fronts that prefix). It is a SEPARATE backend from
// the bridge's `/v1` routes: no session cookie is required, so a failure here
// is a plain unavailable Relux API, not a lapsed login. The Plugins page reads
// the installed list and drives install/remove through these helpers.

// One installed plugin, flattened for the Plugins table. `protected`/`bundled`
// mark the shipped fixtures that cannot be removed.
export interface ReluxPlugin {
  id: string;
  name: string;
  description: string;
  kind: string;
  version: string;
  enabled: boolean;
  source_kind: string;
  source_label: string;
  install_dir: string;
  protected: boolean;
  bundled: boolean;
  // True when Relux scaffolded the manifest because the source had no
  // relux-plugin.json. The plugin is installed as metadata only and runs nothing
  // until a runtime/tools are configured.
  generated?: boolean;
  // Number of tools the manifest declares. Zero for a generated wrapper, so the
  // UI can be honest that there is nothing to make runnable until tools are added.
  tool_count?: number;
  trust_level?: string | null;
  health?: string | null;
}

// Concise Relux control-plane state (the JSON twin of `relux-kernel state`).
export interface ReluxState {
  db_path: string;
  plugins: number;
  installed_plugins: number;
  namespaces: number;
  agents: number;
  tasks: number;
  runs: number;
  approvals: number;
  open_tasks: number;
  active_runs: number;
  waiting_approval: number;
  blocked: number;
  failed: number;
  pending_approvals: number;
}

// Read an honest JSON error body from a failed Relux API response, falling back
// to the raw text or the status line. Mirrors `request`'s error extraction.
async function reluxError(res: Response): Promise<ApiError> {
  const data = await parse(res);
  const msg =
    (data && typeof data === "object" && "error" in data
      ? String((data as Record<string, unknown>).error)
      : typeof data === "string" && data
        ? data
        : `HTTP ${res.status}`) || `HTTP ${res.status}`;
  return new ApiError(res.status, msg);
}

// One Prime turn over the local Relux control plane (POST /v1/relux/prime).
// Mirrors the kernel's grounded `prime_turn`: Prime classifies intent, then
// either answers, acts, proposes a risky action behind approval, or asks to
// clarify. `disposition` is the durable outcome; `created_task`/`started_run`/
// `approval` name what (if anything) landed. `state` is a fresh control-plane
// summary so the chat can show updated counts without a second round trip.
export interface ReluxPrimeAction {
  type: string;
  [k: string]: unknown;
}
// One next-step button Prime offers in chat (RELUX_MASTER_PLAN §11.1 "Prime
// suggested next actions"). Acting on it routes `message` through the SAME
// /v1/relux/prime turn, so a button can do nothing the user could not type.
// `send: true` sends immediately; `send: false` pre-fills the input for the user
// to confirm or edit before sending.
export interface ReluxPrimeSuggestion {
  label: string;
  message: string;
  send: boolean;
}

// One proposed step of a reviewable plan (RELUX_MASTER_PLAN §10 planning layer,
// §11.1). Purely descriptive: a 1-based position, the brief title, the role it
// needs, and the agent it would land on ("prime" when no specialist fits). There
// is no action here — a proposal is a preview, never a command.
export interface ReluxPrimeProposalStep {
  index: number;
  title: string;
  role: string;
  agent: string;
}

// One polished step title (§10 planning layer, §17.1). `index` keys it to an
// existing authoritative step; `title` is presentation-only wording the LLM brain
// suggested. The kernel only ever emits an index that matches a real step.
export interface ReluxPrimePolishedStep {
  index: number;
  title: string;
}

// An advisory, PRESENTATION-ONLY overlay the optional LLM brain may attach to a
// plan preview (§10 planning layer, §11.1, §17.1). It refines WORDING only — it
// never changes the number of steps, their order, the agent each lands on, or the
// goal (which the commit re-wraps as `orchestrate <goal>`). The kernel validates
// it against the authoritative proposal before attaching, so polished titles align
// 1:1 with the real steps or are absent entirely. Render it on top of the
// authoritative fields; the deterministic values remain the source of truth.
export interface ReluxPrimeProposalPolish {
  summary?: string;
  step_titles?: ReluxPrimePolishedStep[];
  questions?: string[];
  risks?: string[];
  model?: string;
}

// A reviewable, ACTION-FREE plan preview attached to a PlanRequest turn so the
// chat can render a card instead of parsing the prose reply (§10 planning layer,
// §11.1). `multi_step` is true for a genuine split (with `steps`/`agents`); false
// steers to the one-task path with empty steps. Nothing is committed by showing
// this — the explicit "Create these tasks" suggestion is the only commit path.
// Omitted on every non-plan turn. `polish`, when present, is advisory wording only
// (see ReluxPrimeProposalPolish) and never alters what the commit creates.
export interface ReluxPrimeProposal {
  goal: string;
  multi_step: boolean;
  steps: ReluxPrimeProposalStep[];
  agents: string[];
  polish?: ReluxPrimeProposalPolish;
}

// Brain-assisted, VALIDATED task slots that shaped a created task, present ONLY on
// a task-creation turn the brain genuinely sharpened. Provenance/presentation only
// — the kernel validated every field (title sanitized/clamped, assignee checked
// against existing agents, priority clamped) before the task was created.
export interface ReluxPrimeTaskSlots {
  title: string;
  details?: string;
  assignee?: string;
  priority?: number;
  // The model id / CLI brain label that produced these slots, for provenance.
  source?: string;
}

// Brain-assisted, VALIDATED agent-creation slots that shaped a created agent, present
// ONLY on an agent-creation turn the brain genuinely sharpened. The kernel validated
// every field (name normalized into a non-colliding id, adapter checked against the
// live roster) before the agent was created.
export interface ReluxPrimeAgentSlots {
  name: string;
  id: string;
  description?: string;
  adapter?: string;
  notes?: string;
  // A bounded, validated starter persona / operating style the brain proposed and the
  // kernel wrote to the created agent (and shows in Crew). Omitted when none.
  persona?: string;
  source?: string;
}

// Provenance for a brain-polished clarify / brainstorm reply, present ONLY when a
// configured brain re-worded the turn's wording through the VALIDATED path. The turn
// stays action-free; this is the small chip's label source.
export interface ReluxReplyPolish {
  // "clarification" | "brainstorm"
  kind: string;
  // The OpenRouter model id / CLI brain label that produced the wording.
  source: string;
}

// Brain-assisted, VALIDATED subject of a risky admin action (a plugin install or a
// permission grant), present ONLY on a Propose turn the brain sharpened. The action
// ALWAYS stays gated behind a human approval; this is advisory provenance only.
export interface ReluxPrimeAdminSlots {
  // "plugin_install" | "permission_grant"
  kind: string;
  plugin_id?: string;
  subject_kind?: string;
  subject_id?: string;
  permission?: string;
  source?: string;
}

// Brain-resolved assignment slots, present ONLY on an AssignTask turn the brain resolved
// (where the deterministic extractors could not, but a validated proposal supplied the
// missing task/agent). Both ids were validated against the live state before the
// assignment happened; this is provenance for a small chip. Omitted on every other turn.
export interface ReluxPrimeAssignSlots {
  task_id: string;
  agent_id: string;
  source?: string;
}

// One field a by-id task update changed (field name + applied display value).
export interface ReluxPrimeTaskChange {
  field: string;
  value: string;
}

// The summary of a by-id task UPDATE Prime applied this turn, present on every
// successful TaskUpdate turn so the chat can show a clean "what changed" card. `source`
// is present ONLY when a configured brain resolved the change (the chip); a
// deterministically-parsed update omits it. Every field was validated by the kernel.
export interface ReluxPrimeTaskUpdate {
  task_id: string;
  changes: ReluxPrimeTaskChange[];
  source?: string;
}

// A bounded, provenance-only record of one READ-ONLY context tool Prime consulted
// before answering this turn (the governed read-only tool loop in `prime_tools`). The
// brain could only ever REQUEST an allowlisted read-only tool, never a mutating one (an
// off-allowlist name is refused, never executed). `ok` is false for an honest MISS (an
// unknown id / empty result) — Prime never fabricates a record. The full result body
// stays server-side grounding; only this short summary ships. Present on a turn ONLY
// when a configured brain ran the loop and gathered at least one read.
export interface ReluxPrimeContextRead {
  // The read-only tool that ran, e.g. "get_task" / "list_agents" / "board_summary".
  tool: string;
  // Whether the read found what was asked. false is an honest miss, never fabricated.
  ok: boolean;
  // A short, human one-line summary of what was read.
  summary: string;
}

// A small, bounded record of a clarifying question Prime is still waiting on the user
// to answer for an actionable request (multi-turn clarify memory). Present on the
// response ONLY while a clarification is pending; the chat shows a "waiting for: <needs>"
// chip with a cancel action, and the next message is read as the answer. Plain,
// non-secret user text only.
export interface ReluxPendingClarification {
  original_message: string;
  intent: string;
  // A short label for what is still missing, e.g. "task id" / "agent" / "task description".
  needs: string;
  question: string;
  created_at_secs: number;
  expires_at_secs: number;
  source: string;
}

export interface ReluxPrimeTurn {
  intent: string;
  reply: string;
  disposition: string;
  action: ReluxPrimeAction | null;
  created_task: string | null;
  started_run: string | null;
  created_agent: string | null;
  approval: string | null;
  // One-click next actions Prime suggests for this turn (§11.1). Omitted when
  // there are none.
  suggested_actions?: ReluxPrimeSuggestion[];
  // A reviewable, action-free plan preview, present ONLY on a plan-request turn
  // (§10 planning layer, §11.1). Omitted on every other turn.
  proposal?: ReluxPrimeProposal;
  // Brain-assisted, validated task slots, present ONLY on a create turn the brain
  // sharpened (§10.1, §10.2, §17.1). Omitted on every other turn.
  slots?: ReluxPrimeTaskSlots;
  // Brain-assisted, validated agent slots, present ONLY on an agent-creation turn the
  // brain sharpened. Omitted on every other turn.
  agent_slots?: ReluxPrimeAgentSlots;
  // Brain-assisted, validated subject of a risky admin action (plugin install /
  // permission grant), present ONLY on a Propose turn the brain sharpened. The action
  // stays approval-gated. Omitted on every other turn.
  admin_slots?: ReluxPrimeAdminSlots;
  // Brain-resolved assignment slots, present ONLY on an AssignTask turn the brain
  // resolved (both ids validated against the live state). Omitted on every other turn.
  assign_slots?: ReluxPrimeAssignSlots;
  // The summary of a by-id task update this turn applied, present ONLY on a successful
  // TaskUpdate turn (carries a brain `source` chip when the brain resolved the change).
  // Omitted on every other turn.
  update?: ReluxPrimeTaskUpdate;
  // Provenance for a brain-polished clarify / brainstorm reply, present ONLY when a
  // configured brain re-worded this turn's wording through the validated path. The turn
  // stays action-free; this is advisory provenance for the small chip. Omitted otherwise.
  reply_polish?: ReluxReplyPolish;
  // The READ-ONLY context tools Prime consulted before answering, in the order it looked
  // (the governed read-only tool loop). Present ONLY when a configured brain ran the loop
  // and gathered at least one read; omitted on every other turn, so existing clients see
  // the same JSON they did before. Provenance only — every read was a deterministic,
  // fabricate-nothing inspection of live state, and none of it is an action.
  context_reads?: ReluxPrimeContextRead[];
  // Tool fields: present only when Prime ran (or honestly refused) a tool this
  // turn. `invoked_tool` is "<plugin_id>/<tool_name>"; `tool_output` carries the
  // real kernel output; `tool_error` is an honest reason a tool did NOT run.
  invoked_tool?: string | null;
  tool_output?: unknown;
  tool_error?: string | null;
  state: ReluxState;
  /// Which path produced the reply (local deterministic, OpenRouter, or a local
  /// CLI brain). `claude_cli`/`codex_cli` mean the answer came from that CLI.
  ai_mode:
    | "deterministic"
    | "deterministic_for_action"
    | "openrouter"
    | "claude_cli"
    | "codex_cli";
  /// The model (OpenRouter) or brain label (CLI) that produced the reply.
  ai_model?: string;
  /// A safe, non-secret note (e.g. why a CLI brain fell back, with the next step).
  ai_note?: string;
  /// Present (as "brain") only when a configured brain genuinely shaped this
  /// turn's INTENT (not just the reply wording). Absent for deterministic turns —
  /// including a brain proposal the safety gate vetoed — so the UI attributes the
  /// brain only when it actually decided. Provenance only; never affects state.
  intent_source?: string;
  /// Present ONLY while Prime is still waiting on the user to answer a clarifying
  /// question for an actionable request (multi-turn clarify memory). The chat renders a
  /// small "waiting for: …" chip with a cancel action; the next message is read as the
  /// answer and continues the original request. Absent when nothing is pending.
  pending_clarification?: ReluxPendingClarification;
  /// Present ONLY when a single UNIFIED brain decision carried more than one proposal this
  /// turn (intent + slots + wording answered in one provider call). The value is the model id
  /// / CLI brain label. The chat renders one concise "one brain decision · <source>" chip; the
  /// per-section chips still attribute each piece. Provenance only; never affects state.
  decision_source?: string;
}

export const reluxPrime = {
  // Send one message to Prime. Throws an ApiError on failure so the chat can
  // show the real reason (e.g. "relux-kernel serve" not running).
  send: (message: string) => api.post<ReluxPrimeTurn>("/v1/relux/prime", { message }),
};

// -- Relux Prime Autonomy --------------------------------------------------

// For /v1/relux/prime/autonomy GET response
export interface ReluxPrimeAutonomyConfig {
  enabled: boolean;
  interval_seconds: number;
  max_tasks_per_tick: number;
  auto_assign_unassigned: boolean;
  last_tick_at: string | null; // ISO 8601 string
  last_tick_summary: string | null;
}

// For /v1/relux/prime/autonomy GET response
export interface ReluxPrimeAutonomyTickResult {
  tick_at: string; // ISO 8601 string
  tasks_run: number;
  tasks_assigned: number;
  actions_taken: number;
  summary: string;
  skipped_reasons: string[];
}

// For /v1/relux/prime/autonomy GET response
export interface ReluxPrimeAutonomyStatusResponse {
  config: ReluxPrimeAutonomyConfig;
  last_tick_result: ReluxPrimeAutonomyTickResult | null;
}

// For /v1/relux/prime/autonomy PUT/PATCH request
export interface UpdateReluxPrimeAutonomyConfigReq {
  enabled?: boolean;
  interval_seconds?: number;
  max_tasks_per_tick?: number;
  auto_assign_unassigned?: boolean;
}

export const reluxPrimeAutonomy = {
  // Get current autonomy configuration and last tick result.
  getStatus: () => api.get<ReluxPrimeAutonomyStatusResponse>("/v1/relux/prime/autonomy"),
  // Update autonomy configuration.
  updateConfig: (config: UpdateReluxPrimeAutonomyConfigReq) =>
    api.patch<ReluxPrimeAutonomyConfig>("/v1/relux/prime/autonomy", config),
  // Trigger one autonomy tick manually.
  runTick: () => api.post<ReluxPrimeAutonomyTickResult>("/v1/relux/prime/autonomy/tick"),
};

// -- Relux Orchestration (multi-agent autonomy) ----------------------------

// The specialist role Prime assigns a brief to (mirrors relux_core OrchestrationRole).
export type ReluxOrchestrationRole =
  | "research"
  | "implementation"
  | "testing"
  | "review"
  | "documentation"
  | "operations"
  | "general";

// The outcome of one brief's most recent run inside a governed batch.
export type ReluxStepOutcome = "pending" | "completed" | "failed" | "blocked";

// Overall orchestration lifecycle.
export type ReluxOrchestrationStatus =
  | "planned"
  | "running"
  | "completed"
  | "needs_attention";

// A planned (uncommitted) brief from the preview endpoint.
export interface ReluxPlannedStep {
  title: string;
  role: ReluxOrchestrationRole;
  agent_id: string | null;
  // Indices (into the plan's steps) of the briefs this brief waits on. Empty for
  // an independent step (the backward-compatible default).
  depends_on?: number[];
}

export interface ReluxOrchestrationPlan {
  goal: string;
  steps: ReluxPlannedStep[];
  notes: string[];
}

// A committed brief: a real task assigned to a real agent, linked to its run.
export interface ReluxOrchestrationStep {
  task_id: string;
  agent_id: string;
  role: ReluxOrchestrationRole;
  title: string;
  outcome: ReluxStepOutcome;
  // Indices (into the orchestration's steps) of the briefs this brief waits on.
  // The run loop only runs a brief once every dependency has completed.
  depends_on?: number[];
  run_id?: string | null;
  note?: string | null;
  // When this brief's most recent run started/finished, and which batch round it
  // ran in (1-based). Absent until the brief has run.
  started_at?: string | null;
  finished_at?: string | null;
  round?: number | null;
}

export interface ReluxOrchestration {
  id: string;
  goal: string;
  created_by: string;
  namespace_id: string;
  status: ReluxOrchestrationStatus;
  steps: ReluxOrchestrationStep[];
  notes: string[];
  created_at: string;
  updated_at: string;
  last_batch_summary?: string | null;
}

export interface ReluxOrchestrationBatchResult {
  orchestration_id: string;
  ran: number;
  completed: number;
  failed: number;
  blocked: number;
  pending: number;
  // Round-size cap used (1..=4), how many dependency-gated rounds ran, briefs
  // still waiting on a dependency, and briefs blocked because an upstream brief
  // failed. Optional for forward/backward compatibility with older servers.
  concurrency?: number;
  rounds?: number;
  waiting?: number;
  dependency_blocked?: number;
  skipped_reasons: string[];
  per_agent: string[];
  summary: string;
  next_action: string;
  status: ReluxOrchestrationStatus;
}

// --- Non-blocking orchestration jobs --------------------------------------

// The lifecycle of a background orchestration run. Distinct from the
// orchestration's own status: a job is "completed" once its worker finished its
// rounds, even if the orchestration itself ended "needs_attention".
// "canceled" is reached when a cancel was requested and honored: the worker
// finished any in-flight round, then stopped before the next one (see the cancel
// endpoint). It is terminal, like completed/failed.
// "interrupted" is reported only by the poll-by-orchestration-id endpoint when no
// live job exists but the durable record proves a prior run happened (e.g. the
// server restarted mid-job — the in-memory registry is lost, the record is not).
// It is terminal for that job; its pending briefs can be resumed with a fresh run.
export type ReluxJobState =
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "canceled"
  | "interrupted";

// One brief's status as the job last observed it. `outcome` is the durable step
// outcome, except briefs the worker is about to run this round are reported as
// "running" so a mid-batch poll shows real in-flight work.
export interface ReluxJobStepStatus {
  task_id: string;
  agent_id: string;
  title: string;
  outcome: ReluxStepOutcome | "running";
  round?: number | null;
  note?: string | null;
}

// A pollable record of one non-blocking orchestration run. Live jobs are held
// in-memory and lost on a server restart; the poll-by-orchestration-id endpoint
// then RECONSTRUCTS a restart-honest status from the durable record (state
// "completed" or "interrupted", id prefixed "durable:"). Only the raw
// poll-by-job-id endpoint 404s for a lost job (job ids are process-local).
export interface ReluxOrchestrationJob {
  id: string;
  orchestration_id: string;
  state: ReluxJobState;
  max: number;
  concurrency: number;
  current_round: number;
  ran: number;
  completed: number;
  failed: number;
  blocked: number;
  started_at_ms?: number | null;
  completed_at_ms?: number | null;
  last_event?: string | null;
  error?: string | null;
  // True once a cancel has been requested. While the job is still "running" this
  // means "canceling — finishing the in-flight round, then stopping"; the worker
  // flips state to "canceled" once that round completes.
  cancel_requested?: boolean;
  steps: ReluxJobStepStatus[];
  result?: ReluxOrchestrationBatchResult | null;
}

// The run-async response: the queued job plus the URL to poll it.
export interface ReluxStartJobResponse extends ReluxOrchestrationJob {
  status_url: string;
}

export const reluxOrchestration = {
  // Preview a multi-agent plan for a goal WITHOUT committing anything (read-only).
  preview: (goal: string) =>
    api.post<ReluxOrchestrationPlan>("/v1/relux/prime/orchestrate/preview", { goal }),
  // Create (plan + assign) an orchestration from a goal. Creates briefs but does
  // not run them.
  create: (goal: string) =>
    api.post<ReluxOrchestration>("/v1/relux/prime/orchestrations", { goal }),
  // List all orchestrations (newest-first ordering is applied client-side).
  list: () => api.get<ReluxOrchestration[]>("/v1/relux/prime/orchestrations"),
  // Fetch one orchestration with its full step chain.
  get: (id: string) =>
    api.get<ReluxOrchestration>(`/v1/relux/prime/orchestrations/${encodeURIComponent(id)}`),
  // Run a governed multi-agent batch for one orchestration. `max` caps how many
  // briefs run this batch (kernel clamps to 1..=25); `concurrency` caps the
  // round size (kernel clamps to 1..=4, defaults to 2). Omit both to run the
  // whole plan two-at-a-time.
  run: (id: string, opts?: { max?: number; concurrency?: number }) =>
    api.post<ReluxOrchestrationBatchResult>(
      `/v1/relux/prime/orchestrations/${encodeURIComponent(id)}/run`,
      opts ?? {},
    ),
  // Start a NON-BLOCKING run: returns immediately with a queued job + status_url.
  // The dashboard polls the job until it finishes instead of waiting on one long
  // request. Rejects a duplicate concurrent job (409) or an over-cap fleet (429).
  runAsync: (id: string, opts?: { max?: number; concurrency?: number }) =>
    api.post<ReluxStartJobResponse>(
      `/v1/relux/prime/orchestrations/${encodeURIComponent(id)}/run-async`,
      opts ?? {},
    ),
  // The latest job for an orchestration (poll target). A live job wins; with none
  // (e.g. after a server restart) it reconstructs a restart-honest status from the
  // durable record ("completed"/"interrupted"). 404 only when the orchestration
  // never ran a brief — the caller then shows the planned record's progress.
  latestJob: (id: string) =>
    api.get<ReluxOrchestrationJob>(
      `/v1/relux/prime/orchestrations/${encodeURIComponent(id)}/job`,
    ),
  // Poll one job by its (process-local) id. 404 when unknown — never started, or
  // lost to a server restart; poll by orchestration id for restart-honest status.
  job: (jobId: string) =>
    api.get<ReluxOrchestrationJob>(
      `/v1/relux/orchestration-jobs/${encodeURIComponent(jobId)}`,
    ),
  // Request cancellation of an active job. Cooperative and honest: it does NOT
  // kill a running adapter process — the worker finishes the in-flight round,
  // then stops before the next one and marks the job "canceled". Returns the
  // updated job (200); 404 when unknown; 409 when the job already finished.
  cancelJob: (jobId: string) =>
    api.post<ReluxOrchestrationJob>(
      `/v1/relux/orchestration-jobs/${encodeURIComponent(jobId)}/cancel`,
      {},
    ),
};

// -- Relux AI (OpenRouter) -------------------------------------------------

// The four Prime brains (conversational reply providers).
export type ReluxPrimeBrain = "local" | "openrouter" | "claude_cli" | "codex_cli";

export interface ReluxAiStatus {
  mode:
    | "deterministic"
    | "deterministic_for_action"
    | "openrouter"
    | "claude_cli"
    | "codex_cli";
  /// The selected Prime brain.
  brain: ReluxPrimeBrain;
  /// Whether an OpenRouter API key is present (never the key itself).
  configured: boolean;
  disabled: boolean;
  model: string;
  timeout_ms: number;
  /// A human-readable, secret-free explanation of the current mode.
  reason: string;
}

export const reluxAi = {
  // Current AI configuration/status (key-free; never returns the API key).
  status: () => api.get<ReluxAiStatus>("/v1/relux/ai/status"),
  // Configure Prime's AI provider/brain from the dashboard (no env vars). Only
  // OpenRouter takes a key; Claude/Codex adapters use local CLI auth. The key is
  // stored locally (gitignored) and never returned — the response is the
  // key-free status. Pass api_key:"" to clear the stored key. Pass `brain` to
  // pick which provider answers Prime's conversational turns.
  setConfig: (body: {
    provider?: string;
    api_key?: string;
    model?: string;
    disabled?: boolean;
    brain?: ReluxPrimeBrain | "";
  }) => api.put<ReluxAiStatus>("/v1/relux/ai/config", body),
  // Clear the dashboard-stored AI config entirely (falls back to env, then to
  // deterministic Prime).
  clearConfig: () => api.del<ReluxAiStatus>("/v1/relux/ai/config"),
};

// -- Relux Work (tasks + runs) ---------------------------------------------

export interface ReluxTask {
  id: string;
  title: string;
  input: any;
  status: string;
  priority: number;
  created_by: string;
  assigned_agent?: string;
  assignee_name?: string; // New field for assignee's name
  namespace_id: string;
  created_at: string;
  updated_at: string;
}

export interface ReluxAgent {
  id: string;
  name: string;
  description: string;
  adapter_plugin: string;
  namespace: string;
  status: string;
  permissions_summary: string;
  // The agent's starter persona / operating style, when one was set (today via the
  // brain-assisted agent-creation path). Omitted when none.
  persona?: string;
  created_at: string;
}

// One read-only artifact reference captured from an adapter's structured result
// envelope (master plan §9.6 / §15). This is a REFERENCE the adapter declared —
// name/type/summary/source (+ optional sanitized relative path + size) — NOT a
// workspace diff or an apply plan. Relux records it read-only and never reads the
// underlying file; capturing references does NOT enable apply (the Relux run
// model still has no diff/apply). Distinct from the legacy `RunArtifact`.
export interface ReluxRunArtifact {
  name: string;
  // "file" | "diff" | "patch" | "log" | "url" | "note" | "other" — unknown kinds
  // degrade to "other" on the backend.
  type: string;
  summary?: string;
  // The adapter that produced the reference, e.g. "claude-cli".
  source: string;
  // A sanitized, relative path. Absent when none was declared or the declared
  // path was unsafe (absolute / drive / UNC / `..`) and was dropped.
  path?: string;
  // Reported size in bytes, display-only.
  bytes?: number;
  // True when a captured field was truncated to its cap.
  truncated?: boolean;
}

// One reviewable, applyable proposed file change captured from an adapter's
// structured result envelope (master plan §15 / §9.6). Unlike a read-only
// `ReluxRunArtifact`, this carries the full proposed `new_content` of one text
// file plus the agent's `baseline_sha256`, and can be reviewed (approve/reject)
// and — once approved — explicitly applied into the run's controlled workspace
// root with a baseline-conflict check. Capturing it NEVER applies it.
export interface ReluxProposedChange {
  // Safe, relative, `/`-separated target path inside the run's workspace root.
  path: string;
  // The filesystem action: "replace" (over an existing baseline file),
  // "create" (a new file that must not already exist), "rename" (move `path`
  // to `dest_path`), or "delete" (remove `path`). Absent on older records; a
  // missing action is "replace".
  action?: string;
  // For a "rename" (move) action, the destination path the source `path` is
  // moved to. Absent for replace/create/delete.
  dest_path?: string;
  // The full proposed new content of the file (text). Empty for a rename (which
  // moves the file intact) and a delete (which removes it).
  new_content: string;
  // SHA-256 (hex) of the content the agent based its edit on. Absent for a
  // create (no prior file) or when a replace/rename/delete declared none — a
  // replace, rename, or delete apply refuses without it (no force in v1).
  baseline_sha256?: string;
  // SHA-256 (hex) of `new_content`, computed at capture (integrity/display).
  new_sha256: string;
  // Byte length of `new_content`.
  bytes: number;
  // The adapter that produced the change (e.g. "claude-cli").
  source: string;
  // Lifecycle: "proposed" | "approved" | "rejected" | "applied".
  status: string;
  // A bounded operator note recorded at review time.
  review_note?: string;
  // The honest reason the last apply attempt was refused (conflict / no baseline
  // / no workspace root / unsafe target). Cleared on a successful apply.
  refused_reason?: string;
  // The logical-clock stamp recorded when the change was applied.
  applied_at?: string;
}

// The result of a successful proposed-change apply.
export interface ReluxApplyResult {
  run_id: string;
  index: number;
  path: string;
  bytes: number;
  applied_at: string;
}

// The result of a successful transactional (multi-file) proposed-change apply
// (master plan §15). Either every selected change was applied or none were, so
// `applied` lists exactly the files written and `applied_at` is the one shared
// stamp recorded for the whole transaction.
export interface ReluxApplySetResult {
  run_id: string;
  applied: ReluxApplyResult[];
  applied_at: string;
}

export interface ReluxRun {
  id: string;
  task_id: string;
  agent_id: string;
  adapter_plugin: string;
  status: string;
  started_at?: string;
  ended_at?: string;
  summary?: string;
  error?: string;
  // Real measured wall-clock duration of the adapter subprocess (ms). Only
  // present for CLI adapter runs; absent for the deterministic local echo path.
  duration_ms?: number;
  // Token/usage data, only when the adapter emitted a structured result
  // envelope we could parse. Never synthesized.
  usage?: Record<string, unknown>;
  // Reported cost in USD, only when the adapter result envelope carried it.
  cost?: number;
  // When this run was created by retrying an earlier run, that run's id.
  retried_from?: string;
  // Read-only artifact references the adapter declared. Absent/empty when none.
  artifacts?: ReluxRunArtifact[];
  // Reviewable proposed file changes the adapter declared (full-content
  // replacements with a baseline hash). Absent/empty when none.
  proposed_changes?: ReluxProposedChange[];
}

// One Relux run-transcript event from `/v1/relux/runs/:id/events`. This is the
// kernel's own shape (distinct from the legacy bridge `RunEvent`): `ts` is a
// logical-clock ISO string (ordering, not wall time) and `payload` is a parsed
// JSON object, not a string.
export interface ReluxRunEvent {
  id: string;
  run_id: string;
  ts: string;
  kind: string;
  source: string;
  message: string;
  payload?: Record<string, unknown> | null;
}

export interface ReluxAuditEntry {
  id: string;
  ts: string; // Assuming timestamp as string for now, could be number
  actor: string;
  action: string;
  target: string;
  namespace: string;
  result: string;
  metadata?: Record<string, unknown>; // Optional metadata object
  hash?: string; // Optional hash/chain metadata
}

export interface ReluxTaskDetail extends ReluxTask {
  // Potentially more fields for a detailed view, e.g., full input, events
}

export interface ReluxRunDetail extends ReluxRun {
  // The parent task's title, for the run header.
  task_title?: string;
  // The latest transcript event kind, i.e. the current/last phase.
  phase?: string;
  // A bounded, already-redacted excerpt of the adapter's last output.
  output_excerpt?: string;
  // The honest failure reason for a failed run.
  failure_reason?: string;
  // Whether the dashboard should offer a Retry action.
  retryable?: boolean;
}

export const reluxWork = {
  // All tasks, sorted by id.
  listTasks: () => api.get<ReluxTask[]>("/v1/relux/tasks"),
  // Get a specific task by id.
  getTask: (id: string) => api.get<ReluxTaskDetail>(`/v1/relux/tasks/${encodeURIComponent(id)}`),
  // All runs, sorted by id.
  listRuns: () => api.get<ReluxRun[]>("/v1/relux/runs"),
  // Get a specific run by id.
  getRun: (id: string) => api.get<ReluxRunDetail>(`/v1/relux/runs/${encodeURIComponent(id)}`),
  // Get the durable, capped, redacted transcript for a specific run. With
  // `since` (an event id), fetches only the tail STRICTLY AFTER that cursor —
  // the incremental live-tail the Work Run Detail merges onto what it already
  // has. Omitting `since` returns the full transcript (first load / recovery).
  getRunEvents: (id: string, since?: string) => {
    const q = since ? `?since=${encodeURIComponent(since)}` : "";
    return api.get<ReluxRunEvent[]>(`/v1/relux/runs/${encodeURIComponent(id)}/events${q}`);
  },
  // All agents, sorted by id.
  listAgents: () => api.get<ReluxAgent[]>("/v1/relux/agents"),
  // Create a new task and assign it to Prime.
  createTask: (title: string) => api.post<ReluxTask>("/v1/relux/tasks", { title }),
  // Start an execution attempt for a task.
  startTask: (id: string) =>
    api.post<{ task: ReluxTask; run: ReluxRun }>(
      `/v1/relux/tasks/${encodeURIComponent(id)}/start`,
    ),
  // Assign a task to an agent.
  assignTask: (taskId: string, agentId: string) =>
    api.post<ReluxTask>(`/v1/relux/tasks/${encodeURIComponent(taskId)}/assign`, { agent_id: agentId }),

  // Execute a running task locally as its assigned agent.
  executeAssignedTask: (id: string) =>
    api.post<{ run_id: string }>(`/v1/relux/tasks/${encodeURIComponent(id)}/execute-assigned`),

  // Retry a failed run as a fresh run on the same task (master plan section 10.2
  // prime.retry_run). Returns the new run's id.
  retryRun: (id: string) =>
    api.post<{ run_id: string }>(`/v1/relux/runs/${encodeURIComponent(id)}/retry`),

  // Record an operator accept/reject of a proposed change (master plan §15).
  // Returns the updated run detail so the panel can refresh in one round trip.
  // Never applies anything — apply is a separate, explicit action.
  reviewProposedChange: (
    runId: string,
    index: number,
    decision: "approve" | "reject",
    note?: string,
  ) =>
    api.post<ReluxRunDetail>(
      `/v1/relux/runs/${encodeURIComponent(runId)}/proposed-changes/${index}/review`,
      { decision, ...(note ? { note } : {}) },
    ),

  // Apply an APPROVED proposed change into the run's controlled workspace root
  // (master plan §15). Throws an ApiError on an honest refusal (409 not-approved
  // / baseline conflict; 422 no-baseline / no-workspace / unsafe target) so the
  // UI can show the real reason. Never fabricates success.
  applyProposedChange: (runId: string, index: number) =>
    api.post<ReluxApplyResult>(
      `/v1/relux/runs/${encodeURIComponent(runId)}/proposed-changes/${index}/apply`,
    ),

  // Apply a SET of APPROVED proposed changes for one run as a single
  // all-or-nothing transaction (master plan §15). The backend validates every
  // selected change together first (approved, baseline still matching, safe
  // distinct path, existing target) and writes ALL or NONE. Throws an ApiError on
  // an honest refusal (409 baseline conflict; 422 not-approved / no-baseline /
  // no-workspace / unsafe or duplicate target) so the UI can show the real reason.
  applyProposedChangeSet: (runId: string, indices: number[]) =>
    api.post<ReluxApplySetResult>(
      `/v1/relux/runs/${encodeURIComponent(runId)}/proposed-changes/apply`,
      { indices },
    ),
};

export const reluxAudit = {
  // Get audit entries.
  list: (limit = 20) => api.get<ReluxAuditEntry[]>(`/v1/relux/audit?limit=${limit}`),
};

export const reluxPlugins = {
  // The installed plugin list (array). Throws an ApiError on failure so the page
  // can show the real reason (e.g. "relux-kernel serve" not running).
  list: () => api.get<ReluxPlugin[]>("/v1/relux/plugins"),
  // Concise control-plane state summary.
  state: () => api.get<ReluxState>("/v1/relux/state"),
  // Install from a local folder path (resolved on the Relux process host).
  installDir: (path: string) =>
    api.post<ReluxPlugin>("/v1/relux/plugins/install-dir", { path }),
  // Install from a GitHub repository URL.
  installGithub: (url: string) =>
    api.post<ReluxPlugin>("/v1/relux/plugins/install-github", { url }),
  // Upload a .zip archive (multipart field `file`) and install it. Uses a raw
  // fetch so the browser sets the multipart boundary; errors surface as ApiError.
  installZip: async (file: File): Promise<ReluxPlugin> => {
    const form = new FormData();
    form.append("file", file, file.name);
    const res = await fetch("/v1/relux/plugins/install-zip", {
      method: "POST",
      credentials: "include",
      body: form,
    });
    if (!res.ok) throw await reluxError(res);
    return (await parse(res)) as ReluxPlugin;
  },
  // Remove an installed plugin by id. Bundled plugins are refused (HTTP 409).
  remove: (id: string) =>
    api.del<{ removed: string }>(`/v1/relux/plugins/${encodeURIComponent(id)}`),
  // A starter relux-plugin.json for an installed plugin (primarily a generated
  // metadata-only wrapper). The honest next step for a wrapper: it has no tool
  // definitions, so a runtime alone surfaces nothing - the operator fills this in,
  // re-installs, then points a loopback runtime at a local server.
  manifestTemplate: (id: string) =>
    api.get<ReluxManifestTemplate>(
      `/v1/relux/plugins/${encodeURIComponent(id)}/manifest-template`,
    ),
};

export interface ReluxManifestTemplate {
  plugin_id: string;
  filename: string;
  install_dir: string;
  generated: boolean;
  manifest_json: string;
}

// -- Relux plugin tool runtime (HTTP loopback) ------------------------------
// A ToolSet plugin becomes executable only when an operator points it at a
// loopback HTTP server they run themselves. Relux never auto-runs downloaded
// plugin code. The config carries no secrets - only the loopback base URL, the
// enabled flag, and the per-call timeout.

export interface ReluxPluginRuntime {
  plugin_id: string;
  configured: boolean;
  kind?: string | null;
  base_url?: string | null;
  enabled: boolean;
  timeout_ms?: number | null;
}

export const reluxPluginRuntime = {
  // Current runtime config/status for one plugin (404 if the plugin is not
  // installed; `configured: false` when installed with no runtime yet).
  get: (id: string) =>
    api.get<ReluxPluginRuntime>(
      `/v1/relux/plugins/${encodeURIComponent(id)}/runtime`,
    ),
  // Configure (or update) the HTTP loopback runtime. `base_url` is validated as
  // loopback-only; bundled plugins are refused (HTTP 400).
  set: (
    id: string,
    body: { base_url?: string; enabled?: boolean; timeout_ms?: number },
  ) =>
    api.put<ReluxPluginRuntime>(
      `/v1/relux/plugins/${encodeURIComponent(id)}/runtime`,
      body,
    ),
  // Clear the runtime config entirely.
  remove: (id: string) =>
    api.del<ReluxPluginRuntime>(
      `/v1/relux/plugins/${encodeURIComponent(id)}/runtime`,
    ),
};

// -- Relux adapter runtime (local coding-agent CLIs) ------------------------
// An Adapter plugin drives an assigned task. The local-prime adapter runs the
// deterministic echo path; a CLI adapter (Claude/Codex/generic command) spawns
// a local binary in a non-interactive, non-bypass mode. CLI adapters are
// DISABLED BY DEFAULT and carry no secrets - only how to launch the binary,
// whether it is enabled, the timeout, and the output cap.

export interface ReluxAdapterStatus {
  plugin_id: string;
  adapter_name: string;
  kind: string | null;
  configured: boolean;
  enabled: boolean;
  command: string | null;
  available_on_path: boolean;
  resolved_path: string | null;
  timeout_seconds: number | null;
  max_output_bytes: number | null;
  working_dir: string | null;
  // local_deterministic | available | missing_binary | disabled | needs_configuration
  state:
    | "local_deterministic"
    | "available"
    | "missing_binary"
    | "disabled"
    | "needs_configuration";
  detail: string;
}

export const reluxAdapters = {
  // All installed Adapter plugins with their honest runtime status.
  list: () => api.get<ReluxAdapterStatus[]>("/v1/relux/adapters"),
  // One adapter's runtime status (404 if not an installed Adapter).
  get: (id: string) =>
    api.get<ReluxAdapterStatus>(
      `/v1/relux/adapters/${encodeURIComponent(id)}/runtime`,
    ),
  // Configure (or update) the CLI runtime. CLI adapters are disabled by
  // default; pass enabled:true to turn one on. No secrets are accepted.
  set: (
    id: string,
    body: {
      enabled?: boolean;
      command?: string;
      timeout_seconds?: number;
      max_output_bytes?: number;
      working_dir?: string;
    },
  ) =>
    api.put<ReluxAdapterStatus>(
      `/v1/relux/adapters/${encodeURIComponent(id)}/runtime`,
      body,
    ),
  // Clear the runtime config entirely.
  remove: (id: string) =>
    api.del<ReluxAdapterStatus>(
      `/v1/relux/adapters/${encodeURIComponent(id)}/runtime`,
    ),
};

// -- Relux Tools (the honest tool-invocation surface) ----------------------
// Installed plugin tools, surfaced with an honest executable status. Only
// built-in deterministic kernel handlers run; an installed-but-unimplemented
// tool is listed as `not_implemented`, never faked. Invocation is permission-
// checked and audited through the same kernel path as the CLI.

// One discovered tool with its executable status.
export interface ReluxToolDescriptor {
  plugin_id: string;
  tool_name: string;
  description: string;
  permission: string;
  risk: string;
  source_kind: string;
  installed: boolean;
  enabled: boolean;
  protected: boolean;
  // "ready" → invocable (built-in handler or an enabled HTTP loopback runtime);
  // "runtime_not_configured" → installed, but needs a loopback endpoint set;
  // "runtime_disabled" → has a loopback runtime configured but it is disabled;
  // "not_implemented" → no supported runtime exists at all;
  // "missing_permission" → the scoped agent lacks the permission.
  executable:
    | "ready"
    | "runtime_not_configured"
    | "runtime_disabled"
    | "not_implemented"
    | "missing_permission";
}

// The structured result of a successful tool invocation.
export interface ReluxToolInvocationResult {
  plugin_id: string;
  tool_name: string;
  agent_id: string;
  permission: string;
  output: unknown;
}

export const reluxTools = {
  // List installed tools + executable status. Pass `agent` to scope the status
  // to one agent's permissions. Throws an ApiError on failure so the page can
  // show the real reason (e.g. "relux-kernel serve" not running).
  list: (agent?: string) =>
    api.get<ReluxToolDescriptor[]>(
      `/v1/relux/tools${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`,
    ),
  // Invoke a supported built-in tool. `input` defaults to {} server-side; the
  // actor defaults to Prime when `agent_id` is omitted. A not-implemented tool
  // surfaces as an ApiError (HTTP 501); a permission denial as HTTP 403.
  invoke: (body: {
    plugin_id: string;
    tool_name: string;
    input?: unknown;
    agent_id?: string;
  }) => api.post<ReluxToolInvocationResult>("/v1/relux/tools/invoke", body),
};

// -- Relux Approvals --------------------------------------------------------

export interface ReluxApproval {
  id: string;
  requested_by: string;
  status: "Pending" | "Approved" | "Rejected";
  created_at: string;
  resolved_at?: string;
  approver?: string;
  note?: string;
}

export const reluxApprovals = {
  // List all approvals, pending first.
  list: () => api.get<ReluxApproval[]>("/v1/relux/approvals"),
  // Decide on an approval.
  decide: (id: string, decision: "approved" | "rejected", note?: string) =>
    api.post<ReluxApproval>(`/v1/relux/approvals/${encodeURIComponent(id)}/decide`, {
      decision,
      note,
    }),
};

// -- Relux Permissions ------------------------------------------------------

export interface ReluxAgentPermissions {
  agent_id: string;
  permissions: string[];
}

export const reluxPermissions = {
  // List all agents and their permissions.
  list: () => api.get<ReluxAgentPermissions[]>("/v1/relux/permissions"),
  // Grant a permission to an agent.
  grant: (agentId: string, permission: string) =>
    api.post<ReluxAgentPermissions>(
      `/v1/relux/agents/${encodeURIComponent(agentId)}/permissions`,
      { permission },
    ),
};

export interface Probe {
  ok: boolean;
  status: number | null;
  detail: string;
  tenant?: string | null;
}

// Low-level health probe used by the diagnostics panel. Never throws: a
// down bridge resolves to `{ ok:false, status:null }` so the panel itself
// can never blank. Reads the `x-relix-tenant` response header when present
// so the panel can show the current Guild/tenant.
export async function probe(path: string): Promise<Probe> {
  try {
    const res = await fetch(path, { method: "GET", credentials: "include" });
    const tenant = res.headers.get("x-relix-tenant");
    if (res.ok) return { ok: true, status: res.status, detail: "ok", tenant };
    const text = await res.text().catch(() => "");
    let detail = `HTTP ${res.status}`;
    if (text) {
      try {
        const j = JSON.parse(text);
        detail = (j && typeof j === "object" && "error" in j ? String(j.error) : text) || detail;
      } catch {
        detail = text.slice(0, 200);
      }
    }
    return { ok: false, status: res.status, detail, tenant };
  } catch (e) {
    return {
      ok: false,
      status: null,
      detail: e instanceof Error ? e.message : "bridge unreachable",
    };
  }
}

export async function fetchJson<T>(path: string): Promise<T> {
  return await api.get<T>(path);
}

export async function postJson<T>(path: string, body: unknown): Promise<T> {
  return await api.post<T>(path, body);
}
