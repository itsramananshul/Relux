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
// Keyboard-accessible board movement (design §6 "status (clickable to change)";
// §6.7 still-pending "keyboard drag — today keyboard users use the §6.4 select,
// which is the accessible equivalent"). Drag is a POINTER affordance; the select
// is the keyboard/screen-reader path. A tiny unlabelled "Move…" select is not
// clear enough for a non-pointer user, so these pure helpers produce the human
// description of the moves — the descriptive aria-label, the visible helper text
// that explains the Block/Cancel semantics, and the honest reason a finished task
// can't be moved — all derived from the SAME operatorStatusMoves allowlist so the
// words never disagree with what the control actually offers. Kept dependency-free.
// ---------------------------------------------------------------------------

// The human meaning of each operator-settable move: the short phrase for the
// aria-label and the sentence for the visible helper text. Keyed by wire status so
// it stays in lockstep with SETTABLE_MOVES (a move with no entry is a bug, surfaced
// by the unit test that every offered move has guidance).
const MOVE_SEMANTICS: Record<string, { phrase: string; sentence: string }> = {
  blocked: { phrase: "Block to hold this task", sentence: "Block holds the task" },
  cancelled: { phrase: "Cancel to stop it", sentence: "Cancel stops it" },
};

// The clear reason a task that offers no move can't be moved from the board (a
// finished task). Mirrors the columnDropTarget terminal reason word-for-word.
const TERMINAL_REASON = "This task is finished and can't be moved.";

// Why the Open / Running columns are not operator-settable, appended to the helper
// so a screen-reader user learns the machine-driven lanes without seeing the board.
const MACHINE_LANE_NOTE =
  "Open and Running are set by the run lifecycle, not by a board move.";

// The accessible description of the board moves available for a task in `status`.
export interface StatusMoveGuidance {
  // True when at least one move is offered (the control renders a select).
  canMove: boolean;
  // The descriptive label for the move select (names the allowed verbs + effects),
  // so a screen reader announces the semantics, not just "Move…". Empty when no
  // move is offered.
  ariaLabel: string;
  // Visible helper text: the Block/Cancel semantics for a movable task, or the clear
  // reason a finished task can't be moved. Always present so the control is never a
  // bare, unexplained select.
  helper: string;
}

// Build the accessible guidance for a task in `status`, derived from the SAME
// operatorStatusMoves allowlist the control renders — so the announced/visible words
// always match the offered options (no move is described that isn't offered, and a
// finished task gets an honest "can't be moved" reason, not an empty/dead label).
export function statusMoveGuidance(status: string): StatusMoveGuidance {
  const moves = operatorStatusMoves(status);
  if (moves.length === 0) {
    return { canMove: false, ariaLabel: "", helper: TERMINAL_REASON };
  }
  const phrases = moves.map((m) => MOVE_SEMANTICS[m.status]?.phrase ?? m.label);
  const sentences = moves.map((m) => MOVE_SEMANTICS[m.status]?.sentence ?? m.label);
  return {
    canMove: true,
    ariaLabel: `Move task status — ${phrases.join(", ")}`,
    helper: `${sentences.join("; ")}. ${MACHINE_LANE_NOTE}`,
  };
}

// ---------------------------------------------------------------------------
// Reopen a blocked task (design §6.9). The board status allowlist deliberately
// refuses the machine-driven lanes (Open/Running), so re-opening held work is NOT a
// status set — it is a separate run-LIFECYCLE action that re-queues the task
// (Blocked -> Queued) through `POST /v1/relux/tasks/:id/reopen`, after which the
// existing Run path runs it. This pure helper mirrors the kernel `reopen_task`
// eligibility (only a blocked task, and only one with an assigned operative), so the
// control offers Reopen only when the route would accept it, and otherwise shows the
// honest reason — never a dead button. Kept dependency-free.
// ---------------------------------------------------------------------------

// The whole status the reopen action targets: only a BLOCKED task is reopenable
// (mirrors the kernel `TaskNotReopenable` guard — a terminal/running/queued task is
// not held work).
const REOPENABLE_STATUS = "blocked";

// The minimal task shape the eligibility check needs (a subset of ReluxTask), kept
// local so this module stays free of the api types.
export interface ReopenableTask {
  status: string;
  assigned_agent?: string | null;
}

// Whether (and why) a task can be reopened from the board, and the human reason when
// it can't. `applicable` is true only for a blocked task — the control renders nothing
// for any other status (no dead affordance; non-blocked work moves through the normal
// run lifecycle, not a reopen). For a blocked task, `eligible` mirrors the kernel:
// it needs an assigned operative (a run needs an assignee), else the honest reason.
export interface ReopenEligibility {
  // True only for a blocked task — the only status the control appears for.
  applicable: boolean;
  // True when the reopen route would accept it (blocked + assigned).
  eligible: boolean;
  // The honest reason it can't be reopened (shown when applicable && !eligible),
  // or empty when eligible / not applicable.
  reason: string;
}

// Compute reopen eligibility for a task, mirroring kernel `reopen_task`: a blocked
// task with an assigned operative is eligible; a blocked task with no assignee is
// not (assign one first — a run needs an assignee); any non-blocked task is not
// applicable (the control is not shown). A task waiting on an approval is not blocked,
// so it is handled by the Approvals surface, not here.
export function reopenEligibility(task: ReopenableTask): ReopenEligibility {
  if (task.status !== REOPENABLE_STATUS) {
    return { applicable: false, eligible: false, reason: "" };
  }
  const assigned = !!(task.assigned_agent && task.assigned_agent.length > 0);
  if (!assigned) {
    return {
      applicable: true,
      eligible: false,
      reason: "Assign an operative before reopening — a run needs an assignee.",
    };
  }
  return { applicable: true, eligible: true, reason: "" };
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
