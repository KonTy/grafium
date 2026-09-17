/**
 * Honest busy indicators for plain `loading = true` flags.
 *
 * Most busy states in the app are a boolean set before an `await` and cleared
 * after. That is fine for disabling a button, but it is a bad basis for an
 * animation: if the backend never answers, the flag never clears and the
 * animation runs forever, telling the user "this is fine" about a wedged
 * request. Chat avoids that by reducing real backend events (see chatStatus.ts),
 * but a one-shot call like "save settings" has no events to reduce.
 *
 * So the deadline is the evidence. While a run is younger than its deadline the
 * label shimmers, which is what the user reads as "still going". Once it
 * overruns, the shimmer stops and the label says so. The animation therefore
 * still means something: motion is a claim that the work is progressing
 * normally, and the claim is withdrawn as soon as it stops being credible.
 */

/** How long a routine action may run before we stop claiming it is healthy. */
export const BUSY_STALL_MS = 20_000;

export interface BusyDisplay {
  /** Run the sweep. Never true for stalled runs or under reduced motion. */
  shimmer: boolean;
  /** The run has outlived its deadline; say so rather than keep animating. */
  stalled: boolean;
  /** Whole seconds elapsed, for callers that want to show a count. */
  seconds: number;
}

const IDLE: BusyDisplay = { shimmer: false, stalled: false, seconds: 0 };

export function busyDisplay(
  startedAt: number | null,
  now: number,
  reducedMotion = false,
  stallAfterMs = BUSY_STALL_MS
): BusyDisplay {
  if (startedAt === null) return IDLE;
  const elapsed = Math.max(0, now - startedAt);
  // A non-positive deadline means "no deadline" — used by callers that already
  // have real progress evidence and don't want a second opinion.
  const stalled = stallAfterMs > 0 && elapsed > stallAfterMs;
  return { shimmer: !stalled && !reducedMotion, stalled, seconds: Math.floor(elapsed / 1000) };
}
