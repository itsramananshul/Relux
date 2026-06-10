import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { useAuth } from "../auth";
import { AccountPanel } from "./AccountPanel";
import { session, type SessionMetaResponse } from "../api";
import { sessionWarning } from "../account";

// The standalone Relux product shell (RELUX_MASTER_PLAN section 11 Dashboard,
// section 21 Final Product Feeling). This is what relux-kernel serves at /dashboard:
// a Relux-branded surface whose routes are backed ONLY by the local /v1/relux
// control plane (state, prime, work, crew, plugins, approvals, health) - no Relix
// web bridge. Access is gated by the local operator login (RELUX_MASTER_PLAN
// "Local operator login v1"): the kernel protects /v1/relux/* behind the
// relux_session cookie, and the shell shows who is signed in plus a sign-out
// control. The old bridge-backed Relix pages are not part of this shell and do
// not appear in its navigation; they remain reachable at their legacy paths only
// for continuity (see App.tsx LegacyDashboard).

interface NavEntry {
  to: string;
  label: string;
  icon: string;
}

// Relux-local destinations: each is served by the kernel itself and needs no
// bridge or login. This is the entire standalone product navigation.
const RELUX_NAV: NavEntry[] = [
  { to: "/", label: "Home", icon: "◈" },
  { to: "/prime", label: "Prime", icon: "✦" },
  { to: "/work", label: "Work", icon: "⚙" },
  { to: "/crew", label: "Crew", icon: "⨈" },
  { to: "/plugins", label: "Plugins", icon: "#" },
  { to: "/approvals", label: "Approvals", icon: "✔" },
  { to: "/health", label: "Health", icon: "♥" },
];

const TITLES: Record<string, { title: string; sub: string }> = {
  "/": { title: "Relux", sub: "Local control plane - Prime, plugins, tasks, runs" },
  "/prime": { title: "Prime", sub: "Talk to your local operator" },
  "/work": { title: "Work", sub: "Manage tasks and view execution history" },
  "/crew": { title: "Crew", sub: "Manage your agent workforce" },
  "/plugins": { title: "Plugins", sub: "Capabilities installed in the control plane" },
  "/approvals": { title: "Approvals", sub: "Manage pending approvals and agent permissions" },
  "/health": { title: "Health", sub: "Relux kernel health and readiness" },
};

function Group({ label, items }: { label: string; items: NavEntry[] }) {
  return (
    <div className="nav-group">
      <div className="nav-label">{label}</div>
      {items.map((it) => (
        <NavLink
          key={it.to}
          to={it.to}
          end={it.to === "/"}
          title={it.label}
          className={({ isActive }) => "nav-item" + (isActive ? " active" : "")}
        >
          <span className="ico">{it.icon}</span>
          <span>{it.label}</span>
        </NavLink>
      ))}
    </div>
  );
}

export function ReluxShell({ children }: { children: ReactNode }) {
  const loc = useLocation();
  const meta = TITLES[loc.pathname] ?? { title: "Relux", sub: "" };
  const { status, logout } = useAuth();
  const who = status?.username ?? "admin";
  const [accountOpen, setAccountOpen] = useState(false);

  // Passive, low-noise session-expiry warning (RELUX_MASTER_PLAN "Local operator
  // login v1"). We read the safe, non-sliding /v1/auth/me metadata SPARSELY —
  // once on mount and again only on cheap, event-driven moments (the tab
  // becoming visible, the Account panel closing) — never a busy poll. Polling it
  // would be pointless anyway: /v1/auth/me does not slide the session, so it
  // cannot keep an idle console alive. A single per-minute timer counts the
  // deadlines down locally so the chip can appear or clear between fetches with
  // no extra round trips. A fetch failure (older kernel, transient) just leaves
  // the chip hidden — the rest of the shell is unaffected.
  const [sessionMeta, setSessionMeta] = useState<SessionMetaResponse | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const anchorMs = useRef<number>(Date.now());

  const loadMeta = useCallback(() => {
    session
      .me()
      .then((m) => {
        anchorMs.current = Date.now();
        setSessionMeta(m);
        setNowMs(Date.now());
      })
      .catch(() => {
        /* no chip — the shell stays fully usable without the readout */
      });
  }, []);

  // One fetch on mount, plus a re-anchor whenever the operator returns to the
  // tab: their idle deadline may have slid forward (active use elsewhere) or
  // genuinely lapsed while the tab was hidden. Event-driven and non-sliding —
  // not a steady poll.
  useEffect(() => {
    loadMeta();
    const onVisible = () => {
      if (document.visibilityState === "visible") loadMeta();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [loadMeta]);

  // Re-anchor after the Account panel closes — the operator likely just acted
  // (which slides the idle deadline), so refresh the chip's basis once.
  const prevAccountOpen = useRef(accountOpen);
  useEffect(() => {
    if (prevAccountOpen.current && !accountOpen) loadMeta();
    prevAccountOpen.current = accountOpen;
  }, [accountOpen, loadMeta]);

  // A single per-minute local countdown — only while there is actually a window
  // to count down (never under the dev bypass, which sends no deadlines). The
  // windows are hours/days, so 60s keeps the chip honest without churn.
  const hasWindows =
    !!sessionMeta &&
    (typeof sessionMeta.idle_expires_in_secs === "number" ||
      typeof sessionMeta.absolute_expires_in_secs === "number");
  useEffect(() => {
    if (!hasWindows) return;
    const id = setInterval(() => setNowMs(Date.now()), 60_000);
    return () => clearInterval(id);
  }, [hasWindows]);

  const elapsedSecs = sessionMeta ? Math.max(0, Math.floor((nowMs - anchorMs.current) / 1000)) : 0;
  const warn = sessionMeta ? sessionWarning(sessionMeta, elapsedSecs) : null;

  return (
    <div className="app">
      <aside className="sidebar" id="app-sidebar">
        <div className="brand">
          <div className="logo">R</div>
          <div className="name">Relux</div>
          <div className="env">local</div>
        </div>
        <Group label="Control plane" items={RELUX_NAV} />
        <div className="sidebar-foot">
          <div className="muted" style={{ fontSize: 11, padding: "0 12px", lineHeight: 1.5 }}>
            Served by <span className="mono">relux-kernel</span>. Signed in as{" "}
            <span className="mono">{who}</span>.
          </div>
        </div>
      </aside>
      <div className="main">
        <header className="topbar">
          <div className="titlewrap">
            <h1>{meta.title}</h1>
            <span className="sub">{meta.sub}</span>
          </div>
          <div className="spacer" style={{ flex: 1 }} />
          {warn && (
            <button
              className={"session-warn-chip" + (warn.kind === "absolute" ? " hard" : "")}
              title={
                warn.kind === "absolute"
                  ? "This session reaches its hard 7-day limit soon. Sign out and back in to continue — open Account for details."
                  : "This session will sign out for inactivity soon. Any action keeps it alive — open Account for details."
              }
              onClick={() => setAccountOpen(true)}
            >
              <span className="dot" aria-hidden="true" />
              {warn.message}
            </button>
          )}
          <NavLink to="/prime" title="Talk to Prime">
            <button className="btn sm">Ask Prime →</button>
          </NavLink>
          <button
            className="btn ghost sm"
            style={{ margin: "0 4px 0 12px" }}
            title="Account — change your password"
            onClick={() => setAccountOpen(true)}
          >
            {who}
          </button>
          <button
            className="btn ghost sm"
            title="Sign out of the Relux dashboard"
            onClick={() => void logout()}
          >
            Sign out
          </button>
        </header>
        <div className="workspace">{children}</div>
      </div>
      {accountOpen && <AccountPanel who={who} onClose={() => setAccountOpen(false)} />}
    </div>
  );
}
