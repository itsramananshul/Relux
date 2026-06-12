// Pure, framework-free helpers for the Work board status MOVE controls
// (docs/relix-dashboard-design.md §6 "Drag a card to a column → status mutation,
// with transition validation").
//
// THE BOARD MIRRORS THE BACKEND ALLOWLIST. The kernel exposes exactly ONE safe
// operator status move (relux-kernel `KernelState::set_task_status`): a NON-terminal
// task may be set to an operator-settable status — `blocked` or `cancelled`. The
// run lifecycle's machine-driven states (`running`/`completed`/`failed`) are never
// decreed from the board, and a terminal task is never edited. These helpers compute
// the offered moves from those SAME two rules, so the UI never offers a move the
// route would then reject. Kept dependency-free (no React/DOM) so it runs under
// `node --strip-types` (see the docs note dashboard-test-tsx-vs-ts-split).

// One offered status move: the wire status the route accepts + its short button label.
export interface StatusMove {
  // The snake_case TaskStatus the POST /v1/relux/tasks/:id/status body carries.
  status: string;
  // The compact operator-facing label (a verb, not the resulting state).
  label: string;
}

// The operator-settable targets, mirroring relux-kernel `SETTABLE_STATUSES`
// (`blocked` / `cancelled`). Order is the menu order.
const SETTABLE_MOVES: StatusMove[] = [
  { status: "blocked", label: "Block" },
  { status: "cancelled", label: "Cancel" },
];

// The terminal statuses, mirroring relux-kernel `is_terminal_status`. A task in one
// of these is finished and can never be moved from the board.
const TERMINAL_STATUSES = new Set(["completed", "failed", "cancelled", "expired"]);

// True for a status the board can never move out of (a finished task).
export function isTerminalStatus(status: string): boolean {
  return TERMINAL_STATUSES.has(status);
}

// The status moves the board offers for a task in `status`. Empty for a terminal
// task (no move is possible); otherwise the operator-settable targets EXCLUDING the
// task's current status (moving a card to where it already is is not offered). This
// is exactly the set the kernel's set_task_status would accept, so the menu never
// shows a move the route rejects.
export function operatorStatusMoves(status: string): StatusMove[] {
  if (isTerminalStatus(status)) return [];
  return SETTABLE_MOVES.filter((m) => m.status !== status);
}

// Whether the board should render a move control for a task in `status` at all
// (it has at least one offered move). Used to keep the card/detail compact — no
// empty "Move…" affordance on a finished task.
export function canMoveStatus(status: string): boolean {
  return operatorStatusMoves(status).length > 0;
}
