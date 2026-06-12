import { lazy, Suspense } from "react";
import { Link, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { isLegacyPath } from "./routing";
import { useAuth } from "./auth";
import { Login } from "./pages/Login";
import { Layout } from "./components/Layout";
import { ReluxShell } from "./components/ReluxShell";
import { ErrorBoundary } from "./components/ErrorBoundary";

// Route-level code splitting. The shell (App + ReluxShell/Layout + auth gate)
// loads eagerly; every page is fetched on demand the first time its route is
// visited. This keeps the initial JS chunk well under Vite's 500 kB warning
// threshold and means a user who only touches the Relux surfaces never downloads
// the legacy dashboard pages (or vice-versa). Behavior is unchanged: the same
// components render at the same paths — they just arrive in per-route chunks.
//
// Pages export named symbols, so each import is mapped to a `default` for
// React.lazy; ReluxApprovals is already a default export.
const ReluxHome = lazy(() => import("./pages/ReluxHome").then((m) => ({ default: m.ReluxHome })));
const Prime = lazy(() => import("./pages/Prime").then((m) => ({ default: m.Prime })));
const Overview = lazy(() => import("./pages/Overview").then((m) => ({ default: m.Overview })));
const Briefs = lazy(() => import("./pages/Briefs").then((m) => ({ default: m.Briefs })));
const Mandates = lazy(() => import("./pages/Mandates").then((m) => ({ default: m.Mandates })));
const Agents = lazy(() => import("./pages/Agents").then((m) => ({ default: m.Agents })));
const Lattice = lazy(() => import("./pages/Lattice").then((m) => ({ default: m.Lattice })));
const Company = lazy(() => import("./pages/Company").then((m) => ({ default: m.Company })));
const Costs = lazy(() => import("./pages/Costs").then((m) => ({ default: m.Costs })));
const Assign = lazy(() => import("./pages/Assign").then((m) => ({ default: m.Assign })));
const Runs = lazy(() => import("./pages/Runs").then((m) => ({ default: m.Runs })));
const Approvals = lazy(() => import("./pages/Approvals").then((m) => ({ default: m.Approvals })));
const Chat = lazy(() => import("./pages/Chat").then((m) => ({ default: m.Chat })));
const Scheduled = lazy(() => import("./pages/Scheduled").then((m) => ({ default: m.Scheduled })));
const Plugins = lazy(() => import("./pages/Plugins").then((m) => ({ default: m.Plugins })));
const Work = lazy(() => import("./pages/Work").then((m) => ({ default: m.Work })));
const Inbox = lazy(() => import("./pages/Inbox").then((m) => ({ default: m.Inbox })));
const Settings = lazy(() => import("./pages/Settings").then((m) => ({ default: m.Settings })));
const Crew = lazy(() => import("./pages/Crew").then((m) => ({ default: m.Crew })));
const ReluxApprovals = lazy(() => import("./pages/ReluxApprovals"));
const Health = lazy(() => import("./pages/Health").then((m) => ({ default: m.Health })));

// Shown while a route's chunk is in flight. Kept lightweight so the shell chrome
// (sidebar/header) stays painted and only the content area shows a brief hint.
function RouteFallback() {
  return (
    <div className="muted" style={{ padding: 16, fontSize: 13 }}>
      Loading…
    </div>
  );
}

// Route ownership (Relux shell as the default, legacy paths as the exception)
// lives in ./routing as a pure, testable module. Making the Relux shell the
// DEFAULT (rather than an allow-list) is what prevents a blank page: a deep link
// or a stray sub-path (e.g. /crew/<id>) can never silently fall into the
// bridge-gated dashboard and render nothing under `relux-kernel serve`.

// Shown for any unknown path inside the Relux shell, so a bad link renders a real,
// navigable view instead of a blank page.
function ReluxNotFound() {
  return (
    <div className="grid">
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Page not found</h3>
        <p className="muted" style={{ fontSize: 13, lineHeight: 1.6 }}>
          That path is not part of the Relux control plane. Use the sidebar, or
          jump to a known surface:
        </p>
        <div className="row wrap" style={{ gap: 8 }}>
          <Link to="/"><button className="btn sm">Home</button></Link>
          <Link to="/prime"><button className="btn ghost sm">Prime</button></Link>
          <Link to="/work"><button className="btn ghost sm">Work</button></Link>
          <Link to="/crew"><button className="btn ghost sm">Crew</button></Link>
          <Link to="/plugins"><button className="btn ghost sm">Plugins</button></Link>
          <Link to="/health"><button className="btn ghost sm">Health</button></Link>
        </div>
      </div>
    </div>
  );
}

export function App() {
  const loc = useLocation();
  const { loading, status } = useAuth();

  // Local operator login (RELUX_MASTER_PLAN "Local operator login v1"). The
  // standalone Relux surfaces now require a session too: the kernel protects
  // /v1/relux/* behind the relux_session cookie. The static SPA still loads;
  // it renders the setup/login screen here (never a blank page) until a
  // session exists. When auth is disabled (dev/test) the kernel reports
  // authenticated:true, so this gate is transparent.
  if (loading) {
    return <div className="center-spinner">Loading Relux…</div>;
  }
  if (!status?.authenticated) {
    return <Login />;
  }

  // Legacy bridge-backed pages keep their exact paths behind the same gate.
  if (isLegacyPath(loc.pathname)) {
    return <LegacyDashboard />;
  }

  // The Relux-local surfaces are the default product. A catch-all renders an
  // in-shell "not found" so no path is ever blank.
  return (
    <ReluxShell>
      {/* A render crash in any one page renders an error card inside the shell
          instead of white-screening the whole SPA (§17.6; the reported blank
          pages). Keyed on the path so navigating away clears the error. */}
      <ErrorBoundary resetKey={loc.pathname}>
        <Suspense fallback={<RouteFallback />}>
          <Routes>
            <Route path="/" element={<ReluxHome />} />
            <Route path="/prime" element={<Prime />} />
            <Route path="/inbox" element={<Inbox />} />
            <Route path="/work" element={<Work />} />
            <Route path="/plugins" element={<Plugins />} />
            <Route path="/crew" element={<Crew />} />
            <Route path="/approvals" element={<ReluxApprovals />} />
            <Route path="/health" element={<Health />} />
            <Route path="*" element={<ReluxNotFound />} />
          </Routes>
        </Suspense>
      </ErrorBoundary>
    </ReluxShell>
  );
}

// The legacy bridge-backed dashboard (the original Relix operator console). It
// stays available at its existing paths for continuity, still behind the bridge
// auth gate. When the bridge is absent (e.g. running purely under
// `relux-kernel serve`), these pages degrade honestly — but the user never lands
// here first.
function LegacyDashboard() {
  const loc = useLocation();
  const { loading, status } = useAuth();

  if (loading) {
    return <div className="center-spinner">Loading Relix…</div>;
  }

  // Not logged in (or first-run setup needed) → the auth screen.
  if (!status?.authenticated) {
    return <Login />;
  }

  return (
    <Layout>
      <ErrorBoundary resetKey={loc.pathname}>
        <Suspense fallback={<RouteFallback />}>
          <Routes>
          <Route path="/overview" element={<Overview />} />
          <Route path="/mandates" element={<Mandates />} />
          <Route path="/briefs" element={<Briefs />} />
          <Route path="/agents" element={<Agents />} />
          <Route path="/lattice" element={<Lattice />} />
          <Route path="/company" element={<Company />} />
          <Route path="/costs" element={<Costs />} />
          <Route path="/assign" element={<Assign />} />
          <Route path="/runs" element={<Runs />} />
          <Route path="/approvals" element={<Approvals />} />
          <Route path="/chat" element={<Chat />} />
          <Route path="/scheduled" element={<Scheduled />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="*" element={<Navigate to="/overview" replace />} />
          </Routes>
        </Suspense>
      </ErrorBoundary>
    </Layout>
  );
}
