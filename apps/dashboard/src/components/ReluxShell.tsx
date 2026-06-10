import { type ReactNode } from "react";
import { NavLink, useLocation } from "react-router-dom";

// The standalone Relux product shell (RELUX_MASTER_PLAN section 11 Dashboard,
// section 21 Final Product Feeling). This is what relux-kernel serves at /dashboard:
// a Relux-branded surface whose routes are backed ONLY by the local /v1/relux
// control plane (state, prime, work, crew, plugins, approvals, health) - no Relix
// web bridge, no login, no 401. The old bridge-backed Relix pages are not part of
// this shell and do not appear in its navigation; they remain reachable at their
// legacy paths only for continuity (see App.tsx LegacyDashboard).

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
            Served by <span className="mono">relux-kernel</span>. The local control
            plane needs no login.
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
          <NavLink to="/prime" title="Talk to Prime">
            <button className="btn sm">Ask Prime →</button>
          </NavLink>
        </header>
        <div className="workspace">{children}</div>
      </div>
    </div>
  );
}
