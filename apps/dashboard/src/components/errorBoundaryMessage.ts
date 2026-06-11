// Pure formatter for the route-level ErrorBoundary's human-facing message. Lives
// in its own JSX-free module so it is unit-testable under node's type-stripping
// test runner (importing the .tsx boundary would choke on JSX). Turns whatever
// React threw into a non-empty, readable string.
export function errorBoundaryMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message || error.name || "Unknown error";
  }
  if (typeof error === "string") {
    return error || "Unknown error";
  }
  if (error == null) {
    return "Unknown error";
  }
  try {
    return String(error);
  } catch {
    return "Unknown error";
  }
}
