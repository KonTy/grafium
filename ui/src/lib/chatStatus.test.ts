import { describe, it, expect } from "vitest";
import {
  initialState,
  reduce,
  reduceAll,
  isStalled,
  statusDisplay,
  STALL_TIMEOUT_MS,
  HARD_CAP_MS,
  type StreamEvent,
  type StreamState,
} from "./chatStatus";

// Convenience: build a state by folding events from t=0.
function fold(events: StreamEvent[]): StreamState {
  return reduceAll(events, 0);
}

describe("chatStatus reducer", () => {
  it("start moves idle → active and resets counters", () => {
    const s = reduce(initialState(0), { type: "start", at: 100 });
    expect(s.kind).toBe("active");
    expect(s.phase).toBeNull();
    expect(s.tokens).toBe(0);
    expect(s.startedAt).toBe(100);
    expect(s.lastEventAt).toBe(100);
  });

  it("advances phase monotonically through the pipeline", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "retrieving", at: 10 },
      { type: "phase", phase: "processing_prompt", at: 50 },
      { type: "phase", phase: "generating", at: 300 },
    ]);
    expect(s.phase).toBe("generating");
    expect(s.lastEventAt).toBe(300);
  });

  it("does not rewind the displayed phase on an out-of-order event, but counts it as liveness", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "generating", at: 100 },
      // A stray, late lower-ranked phase arrives after tokens started.
      { type: "phase", phase: "retrieving", at: 150 },
    ]);
    expect(s.phase).toBe("generating"); // not rewound to "retrieving"
    expect(s.lastEventAt).toBe(150); // still treated as evidence of life
  });

  it("treats duplicate phase events idempotently but refreshes liveness", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "processing_prompt", at: 20 },
      { type: "phase", phase: "processing_prompt", at: 90 },
    ]);
    expect(s.phase).toBe("processing_prompt");
    expect(s.lastEventAt).toBe(90);
  });

  it("delta sets firstTokenAt once, counts tokens, and forces the generating phase", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "processing_prompt", at: 20 },
      { type: "delta", chars: 5, at: 200 },
      { type: "delta", chars: 3, at: 220 },
    ]);
    expect(s.phase).toBe("generating");
    expect(s.tokens).toBe(2);
    expect(s.chars).toBe(8);
    expect(s.firstTokenAt).toBe(200);
    expect(s.lastEventAt).toBe(220);
  });

  it("keeps terminal states sticky against late events", () => {
    const done = fold([
      { type: "start", at: 0 },
      { type: "delta", chars: 4, at: 100 },
      { type: "done", at: 200 },
    ]);
    // A late cancel/phase must not resurrect or overwrite the finished answer.
    const after = reduce(done, { type: "cancel", at: 300 });
    expect(after.kind).toBe("done");
  });
});

describe("chatStatus reasoning isolation", () => {
  it("thinking phase contributes no answer text", () => {
    // The backend never sends reasoning as a delta; a thinking phase carries no
    // chars, so the accumulated answer stays empty until real tokens arrive.
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "thinking", at: 50 },
      { type: "phase", phase: "thinking", at: 120 },
    ]);
    expect(s.chars).toBe(0);
    expect(s.tokens).toBe(0);
    expect(s.firstTokenAt).toBeNull();
  });
});

describe("chatStatus stall detection", () => {
  it("flags a generation stall when tokens stop for too long", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "delta", chars: 4, at: 1_000 },
    ]);
    expect(isStalled(s, 1_000 + STALL_TIMEOUT_MS - 1)).toBe(false);
    expect(isStalled(s, 1_000 + STALL_TIMEOUT_MS + 1)).toBe(true);
    const d = statusDisplay(s, 1_000 + STALL_TIMEOUT_MS + 1);
    expect(d.kind).toBe("stalled");
    expect(d.animate).toBe(false);
    expect(d.showStop).toBe(true);
    expect(d.label).toMatch(/loading or overloaded/i);
  });

  it("does not flag a quiet pre-token phase until the hard cap", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "processing_prompt", at: 30 },
    ]);
    // 30s of prompt processing with no token is legitimate, not a stall.
    expect(isStalled(s, 30_000)).toBe(false);
    expect(statusDisplay(s, 30_000).kind).toBe("active");
    // But it can't spin forever.
    expect(isStalled(s, HARD_CAP_MS + 1)).toBe(true);
  });
});

describe("chatStatus display", () => {
  it("stops animating and preserves partial answer on error mid-stream", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "delta", chars: 12, at: 100 },
      { type: "error", at: 150, message: "boom" },
    ]);
    expect(s.chars).toBe(12); // partial text preserved
    const d = statusDisplay(s, 200);
    expect(d.kind).toBe("error");
    expect(d.animate).toBe(false);
    expect(d.label).toBe("boom");
  });

  it("stops animating on cancel mid-stream", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "delta", chars: 6, at: 80 },
      { type: "cancel", at: 120 },
    ]);
    const d = statusDisplay(s, 130);
    expect(d.kind).toBe("cancelled");
    expect(d.animate).toBe(false);
    expect(d.showStop).toBe(false);
  });

  it("says so plainly when the stream finishes with zero tokens", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "phase", phase: "processing_prompt", at: 20 },
      { type: "done", at: 500 },
    ]);
    const d = statusDisplay(s, 500);
    expect(d.kind).toBe("done");
    expect(d.label).toMatch(/no answer/i);
  });

  it("does not animate when the user prefers reduced motion", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "delta", chars: 4, at: 100 },
    ]);
    expect(statusDisplay(s, 200, false).animate).toBe(true);
    expect(statusDisplay(s, 200, true).animate).toBe(false);
  });

  it("shows a phase label with elapsed time while generating", () => {
    const s = fold([
      { type: "start", at: 0 },
      { type: "delta", chars: 4, at: 1_000 },
    ]);
    const d = statusDisplay(s, 3_000);
    expect(d.label).toMatch(/Generating/);
    expect(d.label).toMatch(/3s/);
  });
});
