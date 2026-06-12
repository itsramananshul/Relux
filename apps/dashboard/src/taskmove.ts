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
// empty "Move…" affordance on a finished task. Also gates whether a card is
// draggable: a terminal card has no move, so it is not draggable.
export function canMoveStatus(status: string): boolean {
  return operatorStatusMoves(status).length > 0;
}

// ---------------------------------------------------------------------------
// Drag-to-column status movement (design §6 "Drag a card to a column → status
// mutation, with transition validation; an invalid drop shows a toast").
//
// Drag is an ADDITIVE affordance over the StatusMoveControl select (which stays
// for keyboard/accessibility). The drop resolves a TARGET COLUMN to the SAME
// operator-settable status the select would offer — never a looser path. Only
// two of the four board columns map to a settable status (Blocked → `blocked`,
// Done → `cancelled`); the Open and Running columns are machine-driven lanes, so
// a drop there is REJECTED with an honest reason rather than silently failing.
// ---------------------------------------------------------------------------

// The four board columns, mirroring oversight.ts::WorkBucket. Kept as a local
// union so this module stays dependency-free (no runtime import of oversight).
export type BoardColumn = "open" | "running" | "blocked" | "done";

// The operator-settable status a column maps to, or null for a machine-driven
// lane the board can never decree. Blocked → `blocked` (Block), Done → `cancelled`
// (Cancel — the one settable terminal); Open/Running are run-lifecycle lanes.
const COLUMN_SETTABLE: Record<BoardColumn, StatusMove | null> = {
  blocked: { status: "blocked", label: "Block" },
  done: { status: "cancelled", label: "Cancel" },
  open: null,
  running: null,
};

// The outcome of dropping a card on a column: either an allowed settable move or
// an honest rejection reason to show inline (never a silent no-op).
export type ColumnDropResult =
  | { ok: true; status: string; label: string }
  | { ok: false; reason: string };

// Resolve a drop of a task in `currentStatus` onto `column`. Returns the settable
// move to apply, or a clear reason it is not allowed. This mirrors the select
// allowlist (operatorStatusMoves) EXACTLY, so a valid drop never hits a 4xx and an
// invalid one is explained, not swallowed.
export function columnDropTarget(
  column: BoardColumn,
  currentStatus: string,
): ColumnDropResult {
  if (isTerminalStatus(currentStatus)) {
    return { ok: false, reason: "This task is finished and can't be moved." };
  }
  const target = COLUMN_SETTABLE[column];
  if (!target) {
    return {
      ok: false,
      reason:
        column === "running"
          ? "Running is set by the run lifecycle, not by a board move — start the task to run it."
          : "Re-opening a task is a run action, not a board move.",
    };
  }
  if (currentStatus === target.status) {
    return { ok: false, reason: "This task is already in this column." };
  }
  // Defence in depth: only proceed if the select would also offer this move.
  const offered = operatorStatusMoves(currentStatus).some((m) => m.status === target.status);
  if (!offered) {
    return { ok: false, reason: "That move isn't allowed for this task." };
  }
  return { ok: true, status: target.status, label: target.label };
}

// The custom MIME type the drag payload travels under. A private type (not
// `text/plain`) so the board only reacts to its OWN task drags, never to text or
// files dropped from elsewhere (mirrors the kanban reference's `text/x-hermes-task`).
export const TASK_DRAG_MIME = "application/x-relux-task";

// What a dragged card carries: its id and live status (the status is needed at the
// drop site to resolve the move, since dataTransfer data is unreadable mid-dragover).
export interface TaskDragPayload {
  id: string;
  status: string;
}

// Encode a card's identity for dataTransfer.setData (drag start).
export function encodeTaskDrag(p: TaskDragPayload): string {
  return JSON.stringify({ id: p.id, status: p.status });
}

// Decode a dataTransfer payload at the drop site. Returns null for anything that is
// not our well-formed task payload (a foreign drop is ignored, never throws).
export function parseTaskDrag(raw: string | null | undefined): TaskDragPayload | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as unknown;
    if (
      v &&
      typeof v === "object" &&
      typeof (v as TaskDragPayload).id === "string" &&
      (v as TaskDragPayload).id.length > 0 &&
      typeof (v as TaskDragPayload).status === "string"
    ) {
      return { id: (v as TaskDragPayload).id, status: (v as TaskDragPayload).status };
    }
  } catch {
    // Not our JSON payload — ignore.
  }
  return null;
}
