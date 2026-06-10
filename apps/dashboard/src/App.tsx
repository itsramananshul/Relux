import { Link, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { isLegacyPath } from "./routing";
import { useAuth } from "./auth";
import { Login } from "./pages/Login";
import { Layout } from "./components/Layout";
import { ReluxShell } from "./components/ReluxShell";
import { ReluxHome } from "./pages/ReluxHome";
import { Prime } from "./pages/Prime";
import { Overview } from "./pages/Overview";
import { Briefs } from "./pages/Briefs";
import { Mandates } from "./pages/Mandates";
import { Agents } from "./pages/Agents";
import { Lattice } from "./pages/Lattice";
import { Company } from "./pages/Company";
import { Costs } from "./pages/Costs";
import { Assign } from "./pages/Assign";
import { Runs } from "./pages/Runs";
import { Approvals } from "./pages/Approvals";
import { Chat } from "./pages/Chat";
import { Scheduled } from "./pages/Scheduled";
import { Plugins } from "./pages/Plugins";
import { Work } from "./pages/Work";
import { Settings } from "./pages/Settings";
import { Crew } from "./pages/Crew"; // Import the new Crew page
import ReluxApprovals from "./pages/ReluxApprovals"; // Import the new ReluxApprovals page
import { Health } from "./pages/Health"; // Import the new Health page

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

  // Legacy bridge-backed pages keep their exact paths behind the auth gate.
  if (isLegacyPath(loc.pathname)) {
    return <LegacyDashboard />;
  }

  // The Relux-local surfaces are the default product, OUTSIDE the bridge auth
  // gate. A catch-all renders an in-shell "not found" so no path is ever blank.
  return (
    <ReluxShell>
      <Routes>
        <Route path="/" element={<ReluxHome />} />
        <Route path="/prime" element={<Prime />} />
        <Route path="/work" element={<Work />} />
        <Route path="/plugins" element={<Plugins />} />
        <Route path="/crew" element={<Crew />} />
        <Route path="/approvals" element={<ReluxApprovals />} />
        <Route path="/health" element={<Health />} />
        <Route path="*" element={<ReluxNotFound />} />
      </Routes>
    </ReluxShell>
  );
}

// The legacy bridge-backed dashboard (the original Relix operator console). It
// stays available at its existing paths for continuity, still behind the bridge
// auth gate. When the bridge is absent (e.g. running purely under
// `relux-kernel serve`), these pages degrade honestly — but the user never lands
// here first.
function LegacyDashboard() {
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
    </Layout>
  );
}
