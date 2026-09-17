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
  | "generating"
  | "searching_web"
  | "reading_sources"
  // Deep web research phases. Unlike the linear chat phases
  // above, these form a *cycle*: plan → search → read → assess → refine →
  // (search again) → … → synthesize. They're driven explicitly by the research
  // backend, which is why the monotonic guard below doesn't apply to them.
  | "planning"
  | "assessing"
  | "refining"
  | "synthesizing";

export type StatusKind =
  | "idle"
  | "active"
  | "stalled"
  | "done"
  | "error"
  | "cancelled";

// Monotonic ordering of the *linear* chat phases: the displayed phase only
// advances, so a late / out-of-order lower-ranked phase event (e.g. a stray
// `retrieving` after tokens have started) can't rewind the UI to "Searching".
// It still counts as liveness — see `reduce`.
//
// The two web-research phases rank *above* `generating` on purpose: in the
// two-part answer the notes arm streams tokens (→ `generating`) first, and only
// then does the web arm begin. Ranking them higher lets the label advance from
// "Generating" into "Searching the web" without the monotonic guard rejecting
// it as a regression. When the web summary itself starts streaming, a token
// `delta` forces the phase back to `generating` directly (see `reduce`).
//
// The deep-research phases (planning/assessing/refining/synthesizing) also sit
// here so any stray transition from a linear phase resolves sensibly, but among
// *themselves* they are cyclic and bypass this guard entirely — see
// `CYCLIC_PHASES` and `reduce`.
const PHASE_RANK: Record<StreamPhase, number> = {
  retrieving: 0,
  loading_model: 1,
  processing_prompt: 2,
  thinking: 3,
  generating: 4,
  planning: 5,
  searching_web: 6,
  reading_sources: 7,
  assessing: 8,
  refining: 9,
  synthesizing: 10,
};

export function isStreamPhase(phase: string): phase is StreamPhase {
  return Object.prototype.hasOwnProperty.call(PHASE_RANK, phase);
}

// Phases the deep-research workflow drives explicitly and legitimately revisits
// (refine loops back to searching). Transitions *among* these accept the new
// phase directly instead of applying the monotonic guard, so round 2's
// "Searching the web" isn't rejected as a regression from "Refining". A token
// `delta` still overrides them (the strongest evidence), exactly as it does for
// the chat web summary. `searching_web`/`reading_sources` are shared with the
// two-part chat answer, but there they only ever move forward, so nothing about
// that flow changes.
const CYCLIC_PHASES: ReadonlySet<StreamPhase> = new Set<StreamPhase>([
  "planning",
  "searching_web",
  "reading_sources",
  "assessing",
  "refining",
  "synthesizing",
]);

function isCyclicPhase(phase: StreamPhase | null): boolean {
  return phase !== null && CYCLIC_PHASES.has(phase);
}

// No token for this long *while generating* means generation stalled (tokens
// were flowing and stopped) — stop animating and say so.
export const STALL_TIMEOUT_MS = 25_000;

// Liveness window for the web-research phases. The web arm emits a steady
// stream of progress notes (planning, searching, reading each source), and each
// page fetch has its own ~30s network timeout, so we allow a longer gap than
// generation before declaring it stalled — but still bound it, so a wedged
// search or a dead network doesn't animate forever.
export const WEB_STALL_TIMEOUT_MS = 45_000;

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
  /**
   * The work done so far, oldest first, so the UI can stack finished steps in
   * the transcript instead of replacing one line in place. Built here rather
   * than in the view because it must obey the same evidence rules as the rest
   * of this module: a step only appears when the backend actually reported that
   * phase, and only the last open step may animate.
   */
  steps: StreamStep[];
}

/** One reported phase of a single answer. */
export interface StreamStep {
  phase: StreamPhase;
  startedAt: number;
  /** Wall-clock ms the step finished, or null while it is still the live step. */
  endedAt: number | null;
  /** Latest backend progress note for this step ("Reading source 3 of 5"). */
  note: string;
}

/**
 * Upper bound on retained steps. A run is one answer, so this is generous even
 * for multi-round research; it exists only so a backend that flaps between
 * phases can't grow the trail without limit. The oldest are dropped.
 */
export const MAX_TRAIL_STEPS = 40;

export type StreamEvent =
  | { type: "start"; at: number }
  | { type: "phase"; phase: StreamPhase; at: number }
  | { type: "delta"; chars: number; at: number }
  | { type: "note"; at: number; text?: string }
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
    steps: [],
  };
}

function isTerminal(kind: StatusKind): boolean {
  return kind === "done" || kind === "error" || kind === "cancelled";
}

/** Close the open step, if any, without disturbing already-finished ones. */
function closeSteps(steps: StreamStep[], at: number): StreamStep[] {
  if (!steps.length) return steps;
  const last = steps[steps.length - 1];
  if (last.endedAt !== null) return steps;
  return [...steps.slice(0, -1), { ...last, endedAt: Math.max(at, last.startedAt) }];
}

/**
 * Record a transition into `phase`. Re-entering the phase that is already open
 * is not a new step — it's the same work continuing — so the trail shows one
 * row per stretch of work rather than one per event.
 */
function advanceSteps(steps: StreamStep[], phase: StreamPhase, at: number): StreamStep[] {
  const last = steps.length ? steps[steps.length - 1] : null;
  if (last && last.endedAt === null && last.phase === phase) return steps;
  const next = [...closeSteps(steps, at), { phase, startedAt: at, endedAt: null, note: "" }];
  return next.length > MAX_TRAIL_STEPS ? next.slice(next.length - MAX_TRAIL_STEPS) : next;
}

/** Attach the newest progress note to the step it describes. */
function noteSteps(steps: StreamStep[], text: string): StreamStep[] {
  if (!text || !steps.length) return steps;
  const last = steps[steps.length - 1];
  if (last.endedAt !== null || last.note === text) return steps;
  return [...steps.slice(0, -1), { ...last, note: text }];
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
      let phase: StreamPhase | null;
      if (isCyclicPhase(e.phase) && (s.phase === null || isCyclicPhase(s.phase))) {
        // A research-workflow transition among cyclic phases: accept it as-is so
        // the label can move backward (refine → search) on a legitimate new
        // round. Coming *into* the cycle from a linear phase still goes through
        // the monotonic branch below.
        phase = e.phase;
      } else {
        const nextRank = PHASE_RANK[e.phase];
        const curRank = s.phase === null ? -1 : PHASE_RANK[s.phase];
        // Advance the displayed phase monotonically, but always treat the event
        // as liveness evidence (bump lastEventAt) even when it's a duplicate or
        // out-of-order regression.
        phase = nextRank >= curRank ? e.phase : s.phase;
      }
      // A rejected out-of-order phase leaves the displayed phase alone, so it
      // must not open a step either — it was evidence of life, not of new work.
      return {
        ...s,
        kind: "active",
        phase,
        lastEventAt: e.at,
        steps: phase ? advanceSteps(s.steps, phase, e.at) : s.steps,
      };
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
        steps: advanceSteps(s.steps, "generating", e.at),
      };
    }

    case "note":
      // A web-research progress note (source fetched, query issued) carries no
      // answer text, but it is real backend evidence the run is alive. Bump the
      // liveness clock without touching phase or token counts, so an active
      // multi-source research pass isn't mistaken for a stall.
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "active", lastEventAt: e.at, steps: noteSteps(s.steps, e.text ?? "") };

    case "done":
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "done", lastEventAt: e.at, steps: closeSteps(s.steps, e.at) };

    case "error":
      // An error can arrive at any time and always wins over "active"; but a
      // stray error after a clean finish shouldn't rewrite it.
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "error", errorMessage: e.message, lastEventAt: e.at, steps: closeSteps(s.steps, e.at) };

    case "cancel":
      if (isTerminal(s.kind)) return s;
      return { ...s, kind: "cancelled", lastEventAt: e.at, steps: closeSteps(s.steps, e.at) };

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
  if (isCyclicPhase(s.phase)) {
    // The web/research arm reports progress continuously (notes for each query
    // issued and each source read), and every page fetch has its own network
    // timeout, so judge these on liveness (gap since the last note/phase) rather
    // than the pre-token hard cap — a thorough multi-round pass legitimately
    // outlives it. Bounded so a wedged search or dead network can't animate
    // forever.
    return now - s.lastEventAt > WEB_STALL_TIMEOUT_MS;
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
  searching_web: "Searching the web",
  reading_sources: "Reading sources",
  // Deep-research phases. Kept student-plain and free of the trailing "…" —
  // `statusDisplay` appends the ellipsis and elapsed time.
  planning: "Planning searches",
  assessing: "Assessing what's missing",
  refining: "Refining the search",
  synthesizing: "Writing the summary",
};

export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${s % 60}s`;
}

export interface StatusDisplay {
  kind: StatusKind;
  phase: StreamPhase | null;
  /** Human-facing status text (already includes elapsed time where relevant).
   *  Drives the *visible* label only — never the live region, which uses the
   *  split `announce` / `meta` below. */
  label: string;
  /** The stable part of `label`: the phase or terminal/error text with no
   *  ticking elapsed time or throughput. This is what the polite live region
   *  announces, so a screen reader speaks a phase change once instead of every
   *  second (the old ticking `label` re-announced constantly). */
  announce: string;
  /** The volatile part of `label`: elapsed time and tokens/sec. Shown visually
   *  but hidden from assistive tech (aria-hidden) so it never re-announces. */
  meta: string;
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
    return { ...base, kind: "idle", label: "", announce: "", meta: "", animate: false, showStop: false };
  }
  if (s.kind === "error") {
    const label = s.errorMessage ?? "Something went wrong.";
    return {
      ...base,
      kind: "error",
      label,
      announce: label,
      meta: "",
      animate: false,
      showStop: false,
    };
  }
  if (s.kind === "cancelled") {
    return { ...base, kind: "cancelled", label: "Stopped.", announce: "Stopped.", meta: "", animate: false, showStop: false };
  }
  if (s.kind === "done") {
    // Finished, but the model never emitted a token and left no message — say
    // so plainly rather than silently ending on an empty bubble.
    const empty = s.firstTokenAt === null;
    const label = empty ? "The model returned no answer." : "";
    return {
      ...base,
      kind: "done",
      label,
      announce: label,
      meta: "",
      animate: false,
      showStop: false,
    };
  }

  // Active.
  if (isStalled(s, now)) {
    const label = "No response from the model yet — it may be loading or overloaded.";
    return {
      ...base,
      kind: "stalled",
      label,
      announce: label,
      meta: "",
      animate: false,
      showStop: true,
    };
  }

  // Split the phase (stable) from the elapsed/throughput (ticks every second):
  // `label` stays the full visible string, but `announce`/`meta` let the live
  // region speak only the stable phase and keep the timer out of assistive tech.
  const announce = `${s.phase ? PHASE_LABEL[s.phase] : "Working"}…`;
  let meta = formatElapsed(elapsedMs);
  if (s.phase === "generating" && s.firstTokenAt !== null) {
    const secs = (now - s.firstTokenAt) / 1000;
    if (secs >= 1 && s.tokens > 0) {
      meta += ` · ${Math.round(s.tokens / secs)} tok/s`;
    } else {
      meta += ` · ${s.tokens} tok`;
    }
  }
  const label = `${announce} ${meta}`;
  return { ...base, kind: "active", label, announce, meta, animate: !reducedMotion, showStop: true };
}

// ---------------------------------------------------------------------------
// The step trail: the same evidence, stacked in the transcript.
//
// Showing one status line that rewrites itself hides what the assistant
// actually did, and putting it under the composer puts it where the eye isn't.
// The trail keeps each finished step visible next to the answer it produced,
// and animates only the step that is genuinely still running.
// ---------------------------------------------------------------------------

export type TrailRowState = "done" | "active" | "stalled";

export interface TrailRow {
  /** Stable across ticks so the list doesn't re-mount and restart animations. */
  key: string;
  phase: StreamPhase;
  label: string;
  /** Backend progress note for this step, or "". */
  note: string;
  /** How long this step took; "" while it is still running or under a second. */
  meta: string;
  state: TrailRowState;
  /** Whether this row should run the shimmer. Only ever one row, never in
   *  reduced motion, and never once the evidence has dried up. */
  shimmer: boolean;
}

export interface TrailDisplay {
  rows: TrailRow[];
  /** Ticking total for the run, shown while it is still going. */
  elapsed: string;
  /** Total once finished, e.g. for the collapsed summary. */
  finishedIn: string;
  running: boolean;
  /** True when there is something worth rendering. */
  any: boolean;
}

/**
 * Project state + clock into rows. Pure, so "reduced motion never shimmers" and
 * "a stalled run stops animating" are properties that can be unit-tested rather
 * than trusted to CSS.
 */
export function statusTrail(s: StreamState, now: number, reducedMotion = false): TrailDisplay {
  const stalled = isStalled(s, now);
  const running = s.kind === "active" && !stalled;
  const rows: TrailRow[] = s.steps.map((step, index) => {
    const open = step.endedAt === null;
    // Only the final step can still be open; a trailing open step on a finished
    // run (a dropped done event) is reported as done, not left animating.
    const live = open && index === s.steps.length - 1 && (s.kind === "active" || stalled);
    const ended = step.endedAt ?? now;
    const state: TrailRowState = live ? (stalled ? "stalled" : "active") : "done";
    return {
      key: `${index}-${step.phase}-${step.startedAt}`,
      phase: step.phase,
      label: PHASE_LABEL[step.phase],
      note: step.note,
      // Sub-second steps read as noise, so they carry no timing at all.
      meta: !live && ended - step.startedAt >= 1000 ? formatElapsed(ended - step.startedAt) : "",
      state,
      shimmer: state === "active" && !reducedMotion,
    };
  });
  const total = Math.max(0, now - s.startedAt);
  const finishedTotal = Math.max(0, (s.lastEventAt || s.startedAt) - s.startedAt);
  return {
    rows,
    elapsed: formatElapsed(total),
    finishedIn: formatElapsed(finishedTotal),
    running: running || stalled,
    any: rows.length > 0,
  };
}

/**
 * Trail for an answer that has already finished, rebuilt from the steps stored
 * on the message. Nothing here can animate: the run is over.
 */
export function finishedTrail(steps: StreamStep[]): TrailDisplay {
  if (!steps.length) return { rows: [], elapsed: "", finishedIn: "", running: false, any: false };
  const startedAt = steps[0].startedAt;
  const endedAt = steps.reduce((last, step) => Math.max(last, step.endedAt ?? step.startedAt), startedAt);
  return statusTrail(
    { ...initialState(startedAt), kind: "done", lastEventAt: endedAt, steps },
    endedAt
  );
}
