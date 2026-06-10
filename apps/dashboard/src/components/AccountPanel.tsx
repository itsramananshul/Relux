import { useEffect, useRef, useState, type FormEvent } from "react";
import { useAuth } from "../auth";
import { session, type SessionMetaResponse } from "../api";
import {
  MIN_PASSWORD_LEN,
  validatePasswordChange,
  formatDuration,
  idleRemaining,
  absoluteRemaining,
  describeIdlePolicy,
  describeAbsolutePolicy,
  reauthCallout,
  type ReauthCallout,
} from "../account";

// The signed-in operator's Account modal (RELUX_MASTER_PLAN "Local operator
// login v1" — the in-product password change that complements the local
// `reset-admin` CLI recovery). Opened from the Relux shell's Account control; it
// changes the local admin password without any CLI for the normal case. The
// kernel verifies the current password, stores a fresh Argon2id hash, and
// invalidates every OTHER session — so this tab stays signed in while any other
// browser/device is booted. "Forgot password" still points at `reset-admin`.

// The re-authentication affordance in the Account modal (RELUX_MASTER_PLAN
// "Local operator login v1"). Always present when auth is enforced — signing
// out then back in is the one reliable way to clear the hard absolute ceiling —
// and EMPHASISED (primary button + an alert banner) only when `callout` says
// that ceiling is close; unadorned (ghost button, no banner) otherwise. It
// hides entirely under the dev bypass. Pure/presentational so the promoted vs
// unadorned markup can be server-rendered with fixture metadata in tests, not
// just exercised through a live fetch.
export function AccountReauth({
  authDisabled,
  callout,
  reauthErr,
  signingOut,
  onReauth,
}: {
  authDisabled: boolean;
  callout: ReauthCallout | null;
  reauthErr: string | null;
  signingOut: boolean;
  onReauth: () => void;
}) {
  if (authDisabled) return null;
  return (
    <div className="account-reauth" style={{ padding: "0 16px 12px" }}>
      {callout && (
        <div className="banner err" role="alert" style={{ marginBottom: 10 }}>
          {callout.message}. Only a fresh sign-in extends it — sign out and back in to keep
          working.
        </div>
      )}
      {reauthErr && (
        <div className="banner err" role="alert">
          {reauthErr}
        </div>
      )}
      <button
        className={"btn" + (callout ? "" : " ghost") + " sm"}
        style={{ width: "100%" }}
        disabled={signingOut}
        onClick={onReauth}
        title="Sign out now; the sign-in screen appears so you can start a fresh session"
      >
        {signingOut ? "Signing out…" : "Sign out and sign back in"}
      </button>
      <p className="muted" style={{ fontSize: 11, margin: "6px 0 0" }}>
        Ends this session and shows the sign-in screen. Other sessions are unaffected.
      </p>
    </div>
  );
}

export function AccountPanel({ who, onClose }: { who: string; onClose: () => void }) {
  const { changePassword, logout } = useAuth();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);
  // Re-authentication: signing out, then back in, is the one reliable way to
  // clear the hard absolute ceiling. It never auto-submits credentials — it just
  // ends this session so the normal sign-in screen appears.
  const [signingOut, setSigningOut] = useState(false);
  const [reauthErr, setReauthErr] = useState<string | null>(null);
  const firstFieldRef = useRef<HTMLInputElement>(null);

  // Safe session-expiry metadata (GET /v1/auth/me — idle/absolute deadlines, no
  // secret). `anchorMs` is the wall-clock instant the metadata was fetched, so a
  // once-a-minute tick can count down locally without re-fetching (the windows
  // are hours-scale). A failure (older kernel, transient) just hides the readout
  // — the password-change form below still works.
  const [meta, setMeta] = useState<SessionMetaResponse | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const anchorMs = useRef<number>(Date.now());

  // Focus the first field on open and close on Escape — standard modal manners.
  useEffect(() => {
    firstFieldRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Load the session metadata once on open. Reading /v1/auth/me does NOT slide
  // the session, so this is a pure status read.
  useEffect(() => {
    let alive = true;
    session
      .me()
      .then((m) => {
        if (!alive) return;
        anchorMs.current = Date.now();
        setMeta(m);
        setNowMs(Date.now());
      })
      .catch(() => {
        /* no readout — the password-change form still renders */
      });
    return () => {
      alive = false;
    };
  }, []);

  // A single, simple per-minute countdown — only when there is actually a window
  // to count down (never under the dev bypass, which sends no deadlines). The
  // windows are hours/days, so 60s is plenty and keeps the timer un-noisy.
  const hasCountdown =
    !!meta &&
    (typeof meta.idle_expires_in_secs === "number" ||
      typeof meta.absolute_expires_in_secs === "number");
  useEffect(() => {
    if (!hasCountdown) return;
    const id = setInterval(() => setNowMs(Date.now()), 60_000);
    return () => clearInterval(id);
  }, [hasCountdown]);

  const elapsedSecs = meta ? Math.max(0, Math.floor((nowMs - anchorMs.current) / 1000)) : 0;
  const idleLeft = meta ? idleRemaining(meta, elapsedSecs) : null;
  const absLeft = meta ? absoluteRemaining(meta, elapsedSecs) : null;
  const idlePolicy = meta ? describeIdlePolicy(meta) : null;
  const absPolicy = meta ? describeAbsolutePolicy(meta) : null;
  // Emphasise the re-auth path only when the hard ceiling is actually close.
  const callout = meta ? reauthCallout(meta, elapsedSecs) : null;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setErr(null);
    // Friendly client-side guard; the kernel re-validates everything server-side.
    const problem = validatePasswordChange(current, next, confirm);
    if (problem) {
      setErr(problem);
      return;
    }
    setBusy(true);
    try {
      await changePassword(current, next);
      setDone(true);
      setCurrent("");
      setNext("");
      setConfirm("");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Could not change the password.");
    } finally {
      setBusy(false);
    }
  }

  // Sign out so the operator can sign back in with a fresh session — the only
  // thing that clears the hard absolute ceiling. We never auto-submit
  // credentials: on success the app re-renders to the sign-in screen (and this
  // modal unmounts), so the operator types their password themselves. A failure
  // (e.g. the kernel is briefly unreachable) leaves the session intact and
  // surfaces the reason; the topbar Sign out control is the fallback.
  async function reauth() {
    setReauthErr(null);
    setSigningOut(true);
    try {
      await logout();
      // Success unmounts this modal as <App> swaps in <Login>; nothing more to do.
    } catch (e) {
      setReauthErr(
        e instanceof Error ? e.message : "Could not sign out. Try the Sign out control in the topbar.",
      );
      setSigningOut(false);
    }
  }

  return (
    <div className="cmdk-overlay" role="presentation" onMouseDown={onClose}>
      <div
        className="account-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Account"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="account-head">
          <div>
            <h2 style={{ margin: 0, fontSize: 16 }}>Account</h2>
            <p className="muted" style={{ fontSize: 12, margin: "4px 0 0" }}>
              Signed in as <span className="mono">{who}</span>. Change your local admin password.
            </p>
          </div>
          <button className="btn ghost sm" title="Close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>

        {meta && (idlePolicy || absPolicy || meta.auth_disabled) && (
          <div className="account-session" style={{ padding: "0 16px 8px" }}>
            {meta.auth_disabled ? (
              <p className="muted" style={{ fontSize: 12, margin: "4px 0 0" }}>
                Session expiry is disabled on this server (
                <span className="mono">RELUX_AUTH_DISABLED</span>).
              </p>
            ) : (
              <ul style={{ listStyle: "none", margin: "4px 0 0", padding: 0 }}>
                {idlePolicy && (
                  <li
                    className="muted"
                    style={{
                      fontSize: 12,
                      display: "flex",
                      justifyContent: "space-between",
                      gap: 8,
                      lineHeight: 1.7,
                    }}
                  >
                    <span>{idlePolicy}</span>
                    {idleLeft != null && (
                      <span className="mono" title="Time left before idle sign-out">
                        {formatDuration(idleLeft)} left
                      </span>
                    )}
                  </li>
                )}
                {absPolicy && (
                  <li
                    className="muted"
                    style={{
                      fontSize: 12,
                      display: "flex",
                      justifyContent: "space-between",
                      gap: 8,
                      lineHeight: 1.7,
                    }}
                  >
                    <span>{absPolicy}</span>
                    {absLeft != null && (
                      <span className="mono" title="Time left before re-sign-in is required">
                        {formatDuration(absLeft)} left
                      </span>
                    )}
                  </li>
                )}
              </ul>
            )}
          </div>
        )}

        {/* Re-authentication path. Always present (signing out then back in is the
            one reliable way to clear the hard absolute ceiling), and emphasised —
            primary button + an alert banner — only when that ceiling is close. */}
        <AccountReauth
          authDisabled={!!meta?.auth_disabled}
          callout={callout}
          reauthErr={reauthErr}
          signingOut={signingOut}
          onReauth={() => void reauth()}
        />

        {done ? (
          <div style={{ padding: "4px 16px 16px" }}>
            <div className="banner ok">
              Password changed. Other signed-in sessions have been signed out; this one stays active.
            </div>
            <button className="btn" style={{ width: "100%" }} onClick={onClose}>
              Done
            </button>
          </div>
        ) : (
          <form onSubmit={submit} style={{ padding: "4px 16px 16px" }}>
            {err && <div className="banner err">{err}</div>}
            <label className="field">
              <span>Current password</span>
              <input
                ref={firstFieldRef}
                className="input"
                type="password"
                value={current}
                autoComplete="current-password"
                onChange={(e) => setCurrent(e.target.value)}
              />
            </label>
            <label className="field">
              <span>New password (min {MIN_PASSWORD_LEN} chars)</span>
              <input
                className="input"
                type="password"
                value={next}
                autoComplete="new-password"
                onChange={(e) => setNext(e.target.value)}
              />
            </label>
            <label className="field">
              <span>Confirm new password</span>
              <input
                className="input"
                type="password"
                value={confirm}
                autoComplete="new-password"
                onChange={(e) => setConfirm(e.target.value)}
              />
            </label>
            <button className="btn" style={{ width: "100%", marginTop: 2 }} disabled={busy}>
              {busy ? "…" : "Change password"}
            </button>
            <details className="forgot">
              <summary>Forgot your current password?</summary>
              <p className="muted" style={{ fontSize: 12, margin: "8px 0 4px" }}>
                Run the reset on the machine hosting Relux, then restart it and sign in with the new
                password:
              </p>
              <pre className="forgot-cmd">relux-kernel reset-admin</pre>
              <p className="muted" style={{ fontSize: 11, margin: "4px 0 0" }}>
                Local operator recovery only — it rewrites just the admin credential, not your data.
                There is no online reset.
              </p>
            </details>
          </form>
        )}
      </div>
    </div>
  );
}
