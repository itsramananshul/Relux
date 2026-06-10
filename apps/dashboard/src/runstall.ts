// Pure, event-model-agnostic "no activity" / stalled-run signal, shared by both
// run-transcript surfaces: the legacy bridge `<RunTranscript>` (numeric
// `event_id` + real wall-clock `ts`) and the Relux Work `RunDetailPanel`
// (string `revent_NNNN` id + a LOGICAL-clock `ts`). The two event models are
// deliberately NOT coupled here — this helper only does time math over two
// real wall-clock millisecond instants, so both surfaces get the identical
// threshold + copy without sharing their incompatible event shapes.
//
// It is NOT a progress bar: it only reports elapsed silence, never fabricated
// forward motion.

// The default quiet-period threshold (seconds) before an in-flight run is
// flagged as showing no activity. Short enough to be honest about a stall, long
// enough not to flicker between two normal polls. Both surfaces use this value
// so the stalled cue reads the same on the Runs page and the Work page.
export const RUN_STALL_SECS = 10;

// An honest "no activity" signal for an in-flight run: when the run is still
// running but no new transcript event (and no phase/status change) has arrived
// for at least `thresholdSecs`, return human text like `No activity for 14s`.
// Returns null while activity is recent (or unknown), so the UI shows the
// normal live indicator instead. `lastActivityAtMs` / `nowMs` are real
// wall-clock millis (injected so the helper stays pure + testable). A now that
// precedes the last activity (clock skew) never produces a bogus signal.
export function noActivityLabel(
  lastActivityAtMs: number | null,
  nowMs: number,
  thresholdSecs: number = RUN_STALL_SECS,
): string | null {
  if (lastActivityAtMs == null) return null;
  const elapsed = Math.floor((nowMs - lastActivityAtMs) / 1000);
  if (elapsed < thresholdSecs) return null;
  if (elapsed < 60) return `No activity for ${elapsed}s`;
  const mins = Math.floor(elapsed / 60);
  const secs = elapsed % 60;
  return `No activity for ${mins}m ${secs}s`;
}
