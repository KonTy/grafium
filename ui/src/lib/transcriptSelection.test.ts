import { describe, it, expect, beforeEach } from "vitest";
import { selectionIntersectsTranscript } from "./transcriptSelection";

// The helper is what decides whether a finished drag actually selected
// transcript text (so the composer must NOT steal focus back). The regression
// it guards: a drag that *starts outside* .chat-log and ends inside it — that
// used to be missed, collapsing the user's selection.

let beforeText: Text;
let pText: Text;
let afterText: Text;
let transcript: HTMLElement;

beforeEach(() => {
  document.body.innerHTML =
    '<div id="before">before text</div>' +
    '<div id="transcript"><p id="p">hello world</p></div>' +
    '<div id="after">after text</div>';
  transcript = document.getElementById("transcript") as HTMLElement;
  beforeText = document.getElementById("before")!.firstChild as Text;
  pText = document.getElementById("p")!.firstChild as Text;
  afterText = document.getElementById("after")!.firstChild as Text;
});

function fakeSelection(ranges: Range[]): Selection {
  const collapsed = ranges.length === 0 || ranges.every((r) => r.collapsed);
  return {
    isCollapsed: collapsed,
    rangeCount: ranges.length,
    getRangeAt: (i: number) => ranges[i],
  } as unknown as Selection;
}

function range(startNode: Node, startOffset: number, endNode: Node, endOffset: number): Range {
  const r = document.createRange();
  r.setStart(startNode, startOffset);
  r.setEnd(endNode, endOffset);
  return r;
}

describe("selectionIntersectsTranscript", () => {
  it("returns false for a null selection", () => {
    expect(selectionIntersectsTranscript(null, transcript)).toBe(false);
  });

  it("returns false for a null transcript", () => {
    const sel = fakeSelection([range(pText, 0, pText, 5)]);
    expect(selectionIntersectsTranscript(sel, null)).toBe(false);
  });

  it("returns false for a collapsed selection inside the transcript", () => {
    const sel = fakeSelection([range(pText, 2, pText, 2)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(false);
  });

  it("returns true for a selection wholly inside the transcript", () => {
    const sel = fakeSelection([range(pText, 0, pText, 5)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(true);
  });

  it("returns false for a selection wholly before the transcript", () => {
    const sel = fakeSelection([range(beforeText, 0, beforeText, 6)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(false);
  });

  it("returns false for a selection wholly after the transcript", () => {
    const sel = fakeSelection([range(afterText, 0, afterText, 5)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(false);
  });

  it("returns true when a drag starts before the transcript and ends inside it", () => {
    const sel = fakeSelection([range(beforeText, 0, pText, 5)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(true);
  });

  it("returns true when a drag starts inside the transcript and ends after it", () => {
    const sel = fakeSelection([range(pText, 0, afterText, 5)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(true);
  });

  it("returns true when the selection spans across the whole transcript", () => {
    const sel = fakeSelection([range(beforeText, 0, afterText, 5)]);
    expect(selectionIntersectsTranscript(sel, transcript)).toBe(true);
  });
});
