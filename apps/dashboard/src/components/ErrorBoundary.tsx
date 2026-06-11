import { Component, type ErrorInfo, type ReactNode } from "react";
import { errorBoundaryMessage } from "./errorBoundaryMessage";

// A render-time crash in any one page must never white-screen the whole product
// (RELUX_MASTER_PLAN §17.6 "Dashboard must feel like a product"; prime-processing-
// audit principle #2 "render a real loading/error/empty view, never a white
// screen"). React only catches render errors through a class component's
// error-boundary lifecycle, so this is deliberately a class. It wraps the routed
// workspace INSIDE the shell, so the sidebar/topbar stay usable and the operator
// can navigate away (which resets the boundary via `resetKey`). The human-facing
// message is the pure `errorBoundaryMessage` (its own module, unit-tested).
export { errorBoundaryMessage } from "./errorBoundaryMessage";

type Props = {
  children: ReactNode;
  // When this changes (we pass the current pathname), the boundary clears its
  // error so navigating to another page recovers without a full reload.
  resetKey?: string;
};

type State = { error: unknown | null };

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    // Keep the failure honest and debuggable; never swallow it silently.
    // eslint-disable-next-line no-console
    console.error("Dashboard render error:", error, info.componentStack);
  }

  componentDidUpdate(prev: Props): void {
    if (this.state.error && prev.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  render(): ReactNode {
    if (this.state.error == null) {
      return this.props.children;
    }
    const message = errorBoundaryMessage(this.state.error);
    return (
      <div className="grid">
        <div className="card">
          <h3 style={{ marginTop: 0 }}>This page hit an error</h3>
          <div className="banner err" style={{ fontSize: 12 }}>
            {message}
          </div>
          <p className="muted" style={{ fontSize: 13, lineHeight: 1.6 }}>
            The rest of Relux is still running — use the sidebar to switch pages, or
            reload to try this one again.
          </p>
          <div className="row wrap" style={{ gap: 8 }}>
            <button className="btn sm" onClick={() => this.setState({ error: null })}>
              Try again
            </button>
            <button className="btn ghost sm" onClick={() => window.location.reload()}>
              Reload
            </button>
          </div>
        </div>
      </div>
    );
  }
}
