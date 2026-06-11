// Pure, dependency-free validation for the in-product password change
// (RELUX_MASTER_PLAN "Local operator login v1" — the authenticated change path
// that complements the local `reset-admin` CLI recovery). Kept React-free (like
// ./onboarding and ./routing) so `node --test` can assert the rules without a
// DOM. The AccountPanel renders whatever this returns; it invents nothing.
//
// These checks are a friendly client-side guard only — the kernel re-validates
// every field server-side (verifies the current password, enforces the minimum
// length, hashes with Argon2id), so a bypassed check can never weaken auth.

// Must match the kernel's `MIN_PASSWORD_LEN` (crates/relux-kernel/src/auth.rs).
// The server is the authority; this only spares the operator a round trip.
export const MIN_PASSWORD_LEN = 8;

// Return a human-friendly error string for an invalid change request, or `null`
// when the inputs are well-formed and safe to submit. Order matters: report the
// most actionable problem first.
export function validatePasswordChange(
  current: string,
  next: string,
  confirm: string,
): string | null {
  if (!current) return "Enter your current password.";
  if (!next) return "Enter a new password.";
  if (next.length < MIN_PASSWORD_LEN) {
    return `New password must be at least ${MIN_PASSWORD_LEN} characters.`;
  }
  if (next !== confirm) return "New passwords do not match.";
  if (next === current) return "New password must differ from the current one.";
  return null;
}

// ── Session expiry / idle readout (Account control) ───────────────────────
// The safe, secret-free session metadata `GET /v1/auth/me` returns for the
// signed-in operator (RELUX_MASTER_PLAN "Local operator login v1" — sliding /
// rolling sessions). Every field is optional so an older kernel that only sends
// `{ username }` still renders the panel — the expiry lines just stay hidden.
// NB: reading /v1/auth/me does NOT slide the session (it is a non-mutating
// status read), so these deadlines are the CURRENT pre-refresh values.
export interface SessionMeta {
  username: string;
  // Absolute instants (unix SECONDS).
  idle_expires_at?: number;
  absolute_expires_at?: number;
  // Seconds remaining at the moment the kernel answered (skew-free; clamped ≥0).
  idle_expires_in_secs?: number;
  absolute_expires_in_secs?: number;
  // The configured policy windows (idle timeout, absolute cap), in seconds.
  idle_timeout_secs?: number;
  absolute_max_secs?: number;
  // The kernel's own clock (unix SECONDS) when it answered.
  server_now?: number;
  // True only under the RELUX_AUTH_DISABLED dev/test bypass.
  auth_disabled?: boolean;
}

// Render a whole number of seconds as a short, friendly duration: "12h", "1h
// 5m", "45m", "30s", or "0s". Pure and DOM-free so `node --test` can pin it.
// Used for both the static policy windows and the live countdowns; it shows at
// most two units (the coarsest two that are non-zero) so an hours-scale window
// never spells out seconds. A negative input clamps to "0s" (an already-expired
// window reads as spent, never a negative string).
export function formatDuration(totalSecs: number): string {
  let s = Math.max(0, Math.floor(totalSecs));
  const days = Math.floor(s / 86400);
  s -= days * 86400;
  const hours = Math.floor(s / 3600);
  s -= hours * 3600;
  const mins = Math.floor(s / 60);
  s -= mins * 60;
  const secs = s;
  const parts: string[] = [];
  if (days) parts.push(`${days}d`);
  if (hours) parts.push(`${hours}h`);
  if (mins) parts.push(`${mins}m`);
  if (secs) parts.push(`${secs}s`);
  if (parts.length === 0) return "0s";
  // Coarsest two non-zero units keep it glanceable (e.g. "7d", "1h 5m").
  return parts.slice(0, 2).join(" ");
}

// Whole seconds elapsed between two wall-clock instants (milliseconds): the
// fetch anchor (when /v1/auth/me was last read) and "now". This is the single
// conversion the shell chip and the Account panel use to turn their per-minute
// tick into the `elapsedSecs` the remaining/warning helpers expect, so the
// countdown advances locally between sparse fetches. Floors to whole seconds
// (a 1.9s gap is 1s elapsed, never rounded up) and clamps ≥0 so a clock that
// briefly steps backwards — or a "now" sampled just before the anchor — never
// yields a negative countdown. Re-anchoring on a fresh /v1/auth/me is just
// moving `anchorMs` to the new fetch instant: elapsed resets to 0 and the
// countdown then follows the fresh deadlines.
export function elapsedSince(anchorMs: number, nowMs: number): number {
  return Math.max(0, Math.floor((nowMs - anchorMs) / 1000));
}

// The seconds remaining on the idle window right now, given the metadata and how
// many seconds have elapsed locally since it was fetched. Anchored on the
// kernel-computed `idle_expires_in_secs` (skew-free) and decremented by the local
// elapsed time, so a once-a-minute tick needs no fresh round trip. Returns null
// when the kernel did not send the field (older kernel) so the caller can hide
// the line rather than invent a number. Clamps at 0.
export function idleRemaining(meta: SessionMeta, elapsedSecs = 0): number | null {
  if (typeof meta.idle_expires_in_secs !== "number") return null;
  return Math.max(0, Math.floor(meta.idle_expires_in_secs - Math.max(0, elapsedSecs)));
}

// The seconds remaining before the absolute re-auth cap, same contract as
// `idleRemaining`. The absolute ceiling never slides, so this is a true
// "you'll be signed out by then no matter what" countdown.
export function absoluteRemaining(meta: SessionMeta, elapsedSecs = 0): number | null {
  if (typeof meta.absolute_expires_in_secs !== "number") return null;
  return Math.max(0, Math.floor(meta.absolute_expires_in_secs - Math.max(0, elapsedSecs)));
}

// One human-friendly line describing the idle policy, e.g. "Signs out after 12h
// of inactivity" — or null when the kernel did not send the window.
export function describeIdlePolicy(meta: SessionMeta): string | null {
  if (typeof meta.idle_timeout_secs !== "number") return null;
  return `Signs out after ${formatDuration(meta.idle_timeout_secs)} of inactivity`;
}

// One human-friendly line describing the absolute re-auth cap, e.g.
// "Re-sign-in required after 7d" — or null when the kernel did not send it.
export function describeAbsolutePolicy(meta: SessionMeta): string | null {
  if (typeof meta.absolute_max_secs !== "number") return null;
  return `Re-sign-in required after ${formatDuration(meta.absolute_max_secs)}`;
}

// ── Passive session-expiry warning (shell chip) ───────────────────────────
// A quiet, low-noise heads-up the Relux shell surfaces when the signed-in
// session is close to ending (RELUX_MASTER_PLAN "Local operator login v1").
// Computed from the SAME non-sliding `/v1/auth/me` metadata the Account control
// reads, so the shell can fetch it sparsely (once on mount + on sparse events)
// rather than busy-polling — and since reading `/v1/auth/me` never slides the
// session, a warning is never silenced just by checking. This is a pure
// decision helper: the shell renders whatever it returns and invents nothing.
//
// Two windows, two meanings, two thresholds:
//  - ABSOLUTE: the 7-day hard ceiling. It never slides, so once it is close the
//    operator WILL be signed out and must re-authenticate — nothing they do in
//    the console extends it. Warned earlier (≤30 min) because the only fix is a
//    fresh sign-in.
//  - IDLE: the rolling inactivity timeout. Any authenticated action slides it
//    forward, so it only bites once the console has genuinely gone idle. Warned
//    later (≤10 min) because normal use clears it on its own.
export const ABSOLUTE_WARN_SECS = 30 * 60;
export const IDLE_WARN_SECS = 10 * 60;

export type SessionWarningKind = "absolute" | "idle";

export interface SessionWarning {
  kind: SessionWarningKind;
  // Seconds left on the warned window at the moment evaluated (clamped ≥0).
  secsLeft: number;
  // A short, glanceable line for the chip (already includes the countdown).
  message: string;
}

// Decide whether to show a passive expiry warning, given the metadata and how
// many seconds have elapsed locally since it was fetched. Returns null when
// nothing is close enough to warn about — the common case, where the chip stays
// hidden. When both windows are inside their thresholds the MORE URGENT one
// wins (fewest seconds left); a tie favours `absolute`, since only a fresh
// sign-in clears it. Never warns under the dev bypass (no deadlines are sent),
// and stays silent for an older kernel that omits the remaining-seconds fields.
export function sessionWarning(meta: SessionMeta, elapsedSecs = 0): SessionWarning | null {
  if (meta.auth_disabled) return null;
  const absLeft = absoluteRemaining(meta, elapsedSecs);
  const idleLeft = idleRemaining(meta, elapsedSecs);
  const candidates: SessionWarning[] = [];
  if (absLeft != null && absLeft <= ABSOLUTE_WARN_SECS) {
    candidates.push({
      kind: "absolute",
      secsLeft: absLeft,
      message: `Re-sign-in required in ${formatDuration(absLeft)}`,
    });
  }
  if (idleLeft != null && idleLeft <= IDLE_WARN_SECS) {
    candidates.push({
      kind: "idle",
      secsLeft: idleLeft,
      message: `Signs out for inactivity in ${formatDuration(idleLeft)}`,
    });
  }
  if (candidates.length === 0) return null;
  // Most urgent first; on a tie, absolute (only a fresh sign-in clears it).
  candidates.sort((a, b) => a.secsLeft - b.secsLeft || (a.kind === "absolute" ? -1 : 1));
  return candidates[0];
}

// ── Re-authentication callout (Account control) ────────────────────────────
// The Account panel always offers a "Sign out and sign back in" affordance — the
// one reliable way to clear the hard absolute ceiling, which no in-console action
// can extend (RELUX_MASTER_PLAN "Local operator login v1"). This pure helper
// decides only whether to EMPHASISE that path with an urgent banner: it fires
// when the non-sliding absolute deadline is inside its warning window (the same
// ABSOLUTE_WARN_SECS the shell chip uses), because that is precisely when a fresh
// sign-in is the only thing that helps. Idle expiry is deliberately excluded —
// any ordinary action slides it forward, so it needs no special re-auth prompt.
// Returns null (the common case: the button still renders, just unadorned) under
// the dev bypass, for an older kernel without deadlines, or whenever the ceiling
// is comfortably far off. The button itself is rendered regardless of this; the
// callout is purely the "now it matters" emphasis.
export interface ReauthCallout {
  // Seconds left on the absolute ceiling when evaluated (clamped ≥0).
  secsLeft: number;
  // A short line for the emphasised banner (already includes the countdown).
  message: string;
}

export function reauthCallout(meta: SessionMeta, elapsedSecs = 0): ReauthCallout | null {
  if (meta.auth_disabled) return null;
  const absLeft = absoluteRemaining(meta, elapsedSecs);
  if (absLeft == null || absLeft > ABSOLUTE_WARN_SECS) return null;
  return {
    secsLeft: absLeft,
    message: `Re-sign-in required in ${formatDuration(absLeft)}`,
  };
}
