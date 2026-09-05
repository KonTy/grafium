// Evidence-driven status for a streaming Chat answer.
//
// The hard rule: the animated "working" indicator must reflect *actual backend
// progress*, never a timer. A spinner that animates on a clock keeps spinning
// after a silent failure, a dead subprocess, or a dropped event — and teaches
// the user the UI lies. So this is a pure reducer over the real events the
// backend emits (phase transitions, token deltas, done/error) plus the wall
// clock, and the display layer *stops* animating and tells the truth when the
// evidence dries up.
//
// It's a pure function of (events, now) so it can be unit-tested exhaustively,
// including out-of-order and duplicate events, without a running app.

export type StreamPhase =
  | "retrieving"
  | "loading_model"
  | "processing_prompt"
  | "thinking"
  | "generating";

export type StatusKind =
  | "idle"
  | "active"
  | "stalled"
  | "done"
  | "error"
  | "cancelled";

// Monotonic ordering of phases: the displayed phase only advances, so a late /
// out-of-order lower-ranked phase event (e.g. a stray `retrieving` after tokens
// have started) can't rewind the UI to "Searching". It still counts as
// liveness — see `reduce`.
const PHASE_RANK: Record<StreamPhase, number> = {
  retrieving: 0,
  loading_model: 1,
  processing_prompt: 2,
  thinking: 3,
  generating: 4,
};

// No token for this long *while generating* means generation stalled (tokens
// were flowing and stopped) — stop animating and say so.
export const STALL_TIMEOUT_MS = 25_000;

// Absolute ceiling for the pre-token phases (retrieving / processing_prompt /
// thinking), which can legitimately be silent for a while on a big prompt but
// must not spin forever if something went quietly wrong.
export const HARD_CAP_MS = 120_000;

export interface StreamState {
  kind: StatusKind;
  phase: StreamPhase | null;
  /** Number of answer deltas received (≈ tokens; the local loop emits one piece/token). */
  tokens: number;
  /** Total answer characters received (partial answer is preserved on error/cancel). */
  chars: number;
  /** Wall-clock ms when the request started. */
  startedAt: number;
  /** Wall-clock ms of the most recent real backend event — the liveness signal. */
  lastEventAt: number;
  /** Wall-clock ms the first answer token arrived, or null if none yet. */
  firstTokenAt: number | null;
  errorMessage: string | null;
}

export type StreamEvent =
  | { type: "start"; at: number }
  | { type: "phase"; phase: StreamPhase; at: number }
  | { type: "delta"; chars: number; at: number }
  | { type: "done"; at: number }
  | { type: "error"; at: number; message: string }
  | { type: "cancel"; at: number };

export function initialState(now = 0): StreamState {
  return {
    kind: "idle",
    phase: null,
    tokens: 0,
    chars: 0,
    startedAt: now,
    lastEventAt: now,
    firstTokenAt: null,
    errorMessage: null,
  };
}

function isTerminal(kind: StatusKind): boolean {
  return kind === "done" || kind === "error" || kind === "cancelled";
}

// Fold one event into the state. Pure and total: unknown-order and duplicate
// events are handled without throwing, and terminal states are sticky (a late
// event never resurrects or overwrites a finished answer).
export function reduce(s: StreamState, e: StreamEvent): StreamState {
  switch (e.type) {
    case "start":
      return { ...initialState(e.at), kind: "active" };

    case "phase": {
      // Ignore phase chatter once we've reached a terminal state.
      if (isTerminal(s.kind)) return s;
      const nextRank = PHASE_RANK[e.phase];
      const curRank = s.phase === null ? -1 : PHASE_RANK[s.phase];
      // Advance the displayed phase monotonically, but always treat the event
      // as liveness evidence (bump lastEventAt) even when it's a duplicate or
      // out-of-order regression.
      const phase = nextRank >= curRank ? e.phase : s.phase;
      return { ...s, kind: "active", phase, lastEventAt: e.at };
    }

    case "delta": {
      if (isTerminal(s.kind)) return s;
      // A real token is the strongest possible evidence of "generating".
      return {
        ...s,
        kind: "active",
        phase: "generating",
        tokens: s.tokens + 1,
        chars: s.chars + Math.max(0, e.chars),
        firstTokenAt: s.firstTokenAt ?? e.at,
        lastEventAt: e.at,
      };
    }

    case "done":
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "done", lastEventAt: e.at };

    case "error":
      // An error can arrive at any time and always wins over "active"; but a
      // stray error after a clean finish shouldn't rewrite it.
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "error", errorMessage: e.message, lastEventAt: e.at };

    case "cancel":
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "cancelled", lastEventAt: e.at };

    default:
      return s;
  }
}

export function reduceAll(events: StreamEvent[], now = 0): StreamState {
  return events.reduce(reduce, initialState(now));
}

// Whether an *active* stream should now be treated as stalled: no evidence for
// long enough that continuing to animate would be dishonest.
export function isStalled(s: StreamState, now: number): boolean {
  if (s.kind !== "active") return false;
  if (s.phase === "generating") {
    // Tokens were flowing and stopped.
    return now - s.lastEventAt > STALL_TIMEOUT_MS;
  }
  // Pre-token phases may legitimately be quiet, but not indefinitely.
  return now - s.startedAt > HARD_CAP_MS;
}

const PHASE_LABEL: Record<StreamPhase, string> = {
  retrieving: "Searching your notes",
  loading_model: "Loading model",
  processing_prompt: "Processing context",
  thinking: "Thinking",
  generating: "Generating",
};

export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${s % 60}s`;
}

export interface StatusDisplay {
  kind: StatusKind;
  phase: StreamPhase | null;
  /** Human-facing status text (already includes elapsed time where relevant). */
  label: string;
  /** Whether to run the "working" animation. False for terminal/stalled states
   *  and whenever the user prefers reduced motion. */
  animate: boolean;
  /** Whether to show the Stop button. */
  showStop: boolean;
  elapsedMs: number;
}

// Derive everything the UI renders from state + clock. `reducedMotion` folds
// straight in, so "reduced motion never animates" is a property of this pure
// function and is unit-tested rather than left to CSS alone.
export function statusDisplay(
  s: StreamState,
  now: number,
  reducedMotion = false
): StatusDisplay {
  const elapsedMs = Math.max(0, now - s.startedAt);
  const base = { phase: s.phase, elapsedMs };

  if (s.kind === "idle") {
    return { ...base, kind: "idle", label: "", animate: false, showStop: false };
  }
  if (s.kind === "error") {
    return {
      ...base,
      kind: "error",
      label: s.errorMessage ?? "Something went wrong.",
      animate: false,
      showStop: false,
    };
  }
  if (s.kind === "cancelled") {
    return { ...base, kind: "cancelled", label: "Stopped.", animate: false, showStop: false };
  }
  if (s.kind === "done") {
    // Finished, but the model never emitted a token and left no message — say
    // so plainly rather than silently ending on an empty bubble.
    const empty = s.firstTokenAt === null;
    return {
      ...base,
      kind: "done",
      label: empty ? "The model returned no answer." : "",
      animate: false,
      showStop: false,
    };
  }

  // Active.
  if (isStalled(s, now)) {
    return {
      ...base,
      kind: "stalled",
      label: "No response from the model yet — it may be loading or overloaded.",
      animate: false,
      showStop: true,
    };
  }

  let label = `${s.phase ? PHASE_LABEL[s.phase] : "Working"}… ${formatElapsed(elapsedMs)}`;
  if (s.phase === "generating" && s.firstTokenAt !== null) {
    const secs = (now - s.firstTokenAt) / 1000;
    if (secs >= 1 && s.tokens > 0) {
      label += ` · ${Math.round(s.tokens / secs)} tok/s`;
    } else {
      label += ` · ${s.tokens} tok`;
    }
  }
  return { ...base, kind: "active", label, animate: !reducedMotion, showStop: true };
}
