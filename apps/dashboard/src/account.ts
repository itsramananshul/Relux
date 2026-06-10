// Pure, dependency-free validation for the in-product password change
// (RELUX_MASTER_PLAN "Local operator login v1" — the authenticated change path
// that complements the local `reset-admin` CLI recovery). Kept React-free (like
// ./onboarding and ./routing) so `node --test` can assert the rules without a
// DOM. The AccountPanel renders whatever this returns; it invents nothing.
//
// These checks are a friendly client-side guard only — the kernel re-validates
// every field server-side (verifies the current password, enforces the minimum
// length, hashes with Argon2id), so a bypassed check can never weaken auth.

// Must match the kernel's `MIN_PASSWORD_LEN` (crates/relux-kernel/src/auth.rs).
// The server is the authority; this only spares the operator a round trip.
export const MIN_PASSWORD_LEN = 8;

// Return a human-friendly error string for an invalid change request, or `null`
// when the inputs are well-formed and safe to submit. Order matters: report the
// most actionable problem first.
export function validatePasswordChange(
  current: string,
  next: string,
  confirm: string,
): string | null {
  if (!current) return "Enter your current password.";
  if (!next) return "Enter a new password.";
  if (next.length < MIN_PASSWORD_LEN) {
    return `New password must be at least ${MIN_PASSWORD_LEN} characters.`;
  }
  if (next !== confirm) return "New passwords do not match.";
  if (next === current) return "New password must differ from the current one.";
  return null;
}
