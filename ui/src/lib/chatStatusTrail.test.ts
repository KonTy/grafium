import { describe, expect, it } from "vitest";
import {
  MAX_TRAIL_STEPS,
  finishedTrail,
  initialState,
  reduce,
  statusTrail,
  type StreamEvent,
  type StreamState,
} from "./chatStatus";

function fold(events: StreamEvent[]): StreamState {
  return events.reduce(reduce, initialState(0));
}

const start: StreamEvent = { type: "start", at: 0 };

describe("Status trail accumulation", () => {
  it("records one step per stretch of work, not one per event", () => {
    const s = fold([
      start,
      { type: "phase", phase: "retrieving", at: 100 },
      { type: "phase", phase: "retrieving", at: 200 },
      { type: "phase", phase: "thinking", at: 300 },
    ]);
    expect(s.steps.map((step) => step.phase)).toEqual(["retrieving", "thinking"]);
    // The finished step keeps the moment the next one began.
    expect(s.steps[0].endedAt).toBe(300);
    expect(s.steps[1].endedAt).toBeNull();
  });

  it("opens a generating step on the first token and keeps it across the rest", () => {
    const s = fold([
      start,
      { type: "phase", phase: "thinking", at: 100 },
      { type: "delta", chars: 4, at: 200 },
      { type: "delta", chars: 4, at: 300 },
      { type: "delta", chars: 4, at: 400 },
    ]);
    expect(s.steps.map((step) => step.phase)).toEqual(["thinking", "generating"]);
    expect(s.steps[1].startedAt).toBe(200);
  });

  it("does not open a step for a phase the monotonic guard rejected", () => {
    // A stray late "retrieving" after tokens is liveness, not new work. Showing
    // a second "Searching your notes" row would claim work that never happened.
    const s = fold([
      start,
      { type: "delta", chars: 4, at: 100 },
      { type: "phase", phase: "retrieving", at: 200 },
    ]);
    expect(s.steps.map((step) => step.phase)).toEqual(["generating"]);
    expect(s.lastEventAt).toBe(200);
  });

  it("gives each research round its own rows", () => {
    const s = fold([
      start,
      { type: "phase", phase: "planning", at: 100 },
      { type: "phase", phase: "searching_web", at: 200 },
      { type: "phase", phase: "reading_sources", at: 300 },
      { type: "phase", phase: "refining", at: 400 },
      { type: "phase", phase: "searching_web", at: 500 },
      { type: "phase", phase: "synthesizing", at: 600 },
    ]);
    expect(s.steps.map((step) => step.phase)).toEqual([
      "planning", "searching_web", "reading_sources", "refining", "searching_web", "synthesizing",
    ]);
  });

  it("closes the open step on done, error and cancel", () => {
    for (const ending of [
      { type: "done", at: 900 },
      { type: "error", at: 900, message: "boom" },
      { type: "cancel", at: 900 },
    ] as StreamEvent[]) {
      const s = fold([start, { type: "phase", phase: "thinking", at: 100 }, ending]);
      expect(s.steps[0].endedAt).toBe(900);
    }
  });

  it("attaches progress notes to the step they describe and ignores them afterwards", () => {
    const s = fold([
      start,
      { type: "phase", phase: "reading_sources", at: 100 },
      { type: "note", at: 200, text: "Reading source 2 of 5" },
    ]);
    expect(s.steps[0].note).toBe("Reading source 2 of 5");

    const after = reduce(fold([start, { type: "phase", phase: "thinking", at: 100 }, { type: "done", at: 200 }]),
      { type: "note", at: 300, text: "late" });
    expect(after.steps[0].note).toBe("");
  });

  it("bounds the trail so a flapping backend cannot grow it without limit", () => {
    const events: StreamEvent[] = [start];
    for (let i = 0; i < MAX_TRAIL_STEPS * 3; i += 1) {
      events.push({ type: "phase", phase: i % 2 ? "searching_web" : "reading_sources", at: 100 + i });
    }
    expect(fold(events).steps.length).toBe(MAX_TRAIL_STEPS);
  });
});

describe("Status trail display", () => {
  const working = fold([
    start,
    { type: "phase", phase: "retrieving", at: 0 },
    { type: "phase", phase: "thinking", at: 4_000 },
  ]);

  it("animates only the step that is still running", () => {
    const trail = statusTrail(working, 6_000);
    expect(trail.rows.map((row) => row.state)).toEqual(["done", "active"]);
    expect(trail.rows.filter((row) => row.shimmer)).toHaveLength(1);
    expect(trail.rows[1].label).toBe("Thinking");
    expect(trail.running).toBe(true);
  });

  it("ticks the total while the run is live", () => {
    expect(statusTrail(working, 6_000).elapsed).toBe("6s");
    expect(statusTrail(working, 65_000).elapsed).toBe("1m 5s");
  });

  it("never shimmers under reduced motion", () => {
    const trail = statusTrail(working, 6_000, true);
    expect(trail.rows.some((row) => row.shimmer)).toBe(false);
    // The row is still reported as the live one; only the animation is dropped.
    expect(trail.rows[1].state).toBe("active");
  });

  it("stops animating and marks the row stalled when the evidence dries up", () => {
    const trail = statusTrail(working, 200_000);
    expect(trail.rows[1].state).toBe("stalled");
    expect(trail.rows[1].shimmer).toBe(false);
  });

  it("treats every row as done once the run ends", () => {
    const done = reduce(working, { type: "done", at: 7_000 });
    const trail = statusTrail(done, 9_000);
    expect(trail.rows.every((row) => row.state === "done")).toBe(true);
    expect(trail.rows.some((row) => row.shimmer)).toBe(false);
    expect(trail.running).toBe(false);
  });

  it("does not leave a dangling step animating when the done event never arrived", () => {
    // Backend dropped `done`; the reducer still shows kind "active" but the
    // display must not imply work is continuing beyond the stall window.
    const trail = statusTrail(working, 200_000);
    expect(trail.rows.some((row) => row.shimmer)).toBe(false);
  });

  it("times steps that took a moment and stays quiet about instant ones", () => {
    const trail = statusTrail(working, 6_000);
    expect(trail.rows[0].meta).toBe("4s");
    // The live row has no duration of its own; the ticking total covers it.
    expect(trail.rows[1].meta).toBe("");

    const quick = fold([
      start,
      { type: "phase", phase: "retrieving", at: 0 },
      { type: "phase", phase: "thinking", at: 300 },
    ]);
    expect(statusTrail(quick, 1_000).rows[0].meta).toBe("");
  });

  it("shows nothing at all before the backend has reported anything", () => {
    const trail = statusTrail(initialState(0), 1_000);
    expect(trail.any).toBe(false);
    expect(trail.rows).toEqual([]);
  });

  it("keeps row keys stable across ticks so animations do not restart", () => {
    const first = statusTrail(working, 6_000).rows.map((row) => row.key);
    const second = statusTrail(working, 9_000).rows.map((row) => row.key);
    expect(second).toEqual(first);
  });
});

describe("Finished trail", () => {
  it("rebuilds a past answer's steps without animating any of them", () => {
    const done = reduce(
      fold([start, { type: "phase", phase: "retrieving", at: 0 }, { type: "phase", phase: "generating", at: 3_000 }]),
      { type: "done", at: 8_000 }
    );
    const trail = finishedTrail(done.steps);
    expect(trail.rows.map((row) => row.label)).toEqual(["Searching your notes", "Generating"]);
    expect(trail.rows.every((row) => row.state === "done" && !row.shimmer)).toBe(true);
    expect(trail.running).toBe(false);
    expect(trail.finishedIn).toBe("8s");
  });

  it("reports nothing for an answer that recorded no steps", () => {
    expect(finishedTrail([]).any).toBe(false);
  });
});
