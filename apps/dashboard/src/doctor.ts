// Pure, React-free presentation helpers for the operator Doctor report
// (relix-dashboard-design.md §15). The kernel's `relux_kernel::doctor` does ALL
// the diagnostic logic; this module only maps a severity to the dashboard's B&W
// badge vocabulary and orders rows for scanning. Kept dependency-free (like
// ./readiness, ./onboarding) so `node --test` can assert it without a DOM.

import type {
  ReluxDoctorSeverity,
  ReluxDoctorReport,
  ReluxDoctorCheck,
} from "./api";

// Map a doctor severity onto the existing badge classes the dashboard already
// styles (done/in_progress/blocked/backlog). Status color is reserved for
// meaning only, matching the rest of the UI.
export function severityBadgeClass(s: ReluxDoctorSeverity): string {
  switch (s) {
    case "ok":
      return "done";
    case "warn":
      return "in_progress";
    case "fail":
      return "blocked";
    case "info":
    default:
      return "backlog";
  }
}

// Short uppercase tag for a severity, shown in the row badge.
export function severityLabel(s: ReluxDoctorSeverity): string {
  switch (s) {
    case "ok":
      return "OK";
    case "warn":
      return "WARN";
    case "fail":
      return "FAIL";
    case "info":
    default:
      return "INFO";
  }
}

// Sort order for scanning: worst first (fail → warn → info → ok), stable within
// a severity so the kernel's natural check order is otherwise preserved.
const SEVERITY_RANK: Record<ReluxDoctorSeverity, number> = {
  fail: 0,
  warn: 1,
  info: 2,
  ok: 3,
};

export function sortChecksBySeverity(
  checks: ReluxDoctorCheck[],
): ReluxDoctorCheck[] {
  return checks
    .map((c, i) => ({ c, i }))
    .sort((a, b) => {
      const r = SEVERITY_RANK[a.c.severity] - SEVERITY_RANK[b.c.severity];
      return r !== 0 ? r : a.i - b.i;
    })
    .map((x) => x.c);
}

// A compact, scan-friendly one-liner, e.g. "1 fail, 2 warn, 5 ok". Omits any
// zero buckets except when everything is zero (degenerate empty report).
export function doctorHeadline(report: ReluxDoctorReport): string {
  const { fail, warn, info, ok } = report.summary;
  const parts: string[] = [];
  if (fail > 0) parts.push(`${fail} fail`);
  if (warn > 0) parts.push(`${warn} warn`);
  if (info > 0) parts.push(`${info} info`);
  if (ok > 0) parts.push(`${ok} ok`);
  return parts.length > 0 ? parts.join(", ") : "no checks";
}
