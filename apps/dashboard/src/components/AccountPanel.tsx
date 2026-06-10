import { useEffect, useRef, useState, type FormEvent } from "react";
import { useAuth } from "../auth";
import { MIN_PASSWORD_LEN, validatePasswordChange } from "../account";

// The signed-in operator's Account modal (RELUX_MASTER_PLAN "Local operator
// login v1" — the in-product password change that complements the local
// `reset-admin` CLI recovery). Opened from the Relux shell's Account control; it
// changes the local admin password without any CLI for the normal case. The
// kernel verifies the current password, stores a fresh Argon2id hash, and
// invalidates every OTHER session — so this tab stays signed in while any other
// browser/device is booted. "Forgot password" still points at `reset-admin`.

export function AccountPanel({ who, onClose }: { who: string; onClose: () => void }) {
  const { changePassword } = useAuth();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);
  const firstFieldRef = useRef<HTMLInputElement>(null);

  // Focus the first field on open and close on Escape — standard modal manners.
  useEffect(() => {
    firstFieldRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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
