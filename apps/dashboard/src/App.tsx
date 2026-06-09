import { Navigate, Route, Routes, useLocation } from "react-router-dom";
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
import { Settings } from "./pages/Settings";

// Routes served entirely by the local Relux control plane (/v1/relux). They need
// no web bridge and no login, so they render in the standalone ReluxShell — this
// is what makes `relux-kernel serve` open into a usable product instead of an
// old Relix login wall.
const RELUX_LOCAL = new Set(["/", "/prime", "/plugins"]);

export function App() {
  const loc = useLocation();

  // The Relux-local surfaces are the default product. They are deliberately
  // OUTSIDE the bridge auth gate: opening /dashboard lands on Relux Home, talks
  // to Prime, and manages plugins without ever touching the old bridge.
  if (RELUX_LOCAL.has(loc.pathname)) {
    return (
      <ReluxShell>
        <Routes>
          <Route path="/" element={<ReluxHome />} />
          <Route path="/prime" element={<Prime />} />
          <Route path="/plugins" element={<Plugins />} />
        </Routes>
      </ReluxShell>
    );
  }

  return <LegacyDashboard />;
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
