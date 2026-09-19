import { describe, expect, it } from "vitest";
import conversation from "./AssistantConversation.svelte?raw";
import research from "./ResearchConversation.svelte?raw";
import trail from "./ChatStatusTrail.svelte?raw";
import bubble from "./ChatMessageBubble.svelte?raw";

describe("Status trail wiring", () => {
  it("puts the live status in the transcript instead of under the composer", () => {
    for (const source of [conversation, research]) {
      expect(source).toContain("<ChatStatusTrail");
      // The old line lived in .conversation-controls, below the input, where
      // the eye isn't. Its replacement must not quietly come back.
      expect(source).not.toContain('{#if running}<p class="status-message" role="status">{status.announce}');
    }
  });

  it("keeps each finished answer's steps next to that answer", () => {
    for (const source of [conversation, research]) {
      expect(source).toContain("finishedTrail(message.steps)");
      expect(source).toContain("collapsed");
    }
  });

  it("follows the trail when scrolling, not just new messages", () => {
    for (const source of [conversation, research]) {
      expect(source).toContain("trail.rows.length;");
    }
  });

  it("does not print the phase twice under the same answer", () => {
    for (const source of [conversation, research]) {
      expect(source).toContain("trailed={view");
    }
    expect(bubble).toContain("&& !trailed");
    expect(bubble).toContain("{#if !hideBubble}");
  });

  it("animates one row at most, using the shared app-wide sweep", () => {
    expect(trail).toContain("class:shimmer={row.shimmer}");
    // The trail's shimmer is evidence-backed — `statusTrail` already withholds
    // it for stalled runs — so it opts out of the global sweep's expiry.
    expect(trail).toContain("class:shimmer-endless={row.shimmer}");
    // The sweep and its reduced-motion override now live once, in global.css.
    expect(trail).not.toContain("@keyframes");
    expect(trail).not.toContain("prefers-reduced-motion");
  });

  it("shimmers the initial AI status before the first trail step arrives", () => {
    expect(bubble).toContain("class:shimmer={animateCursor}");
    expect(bubble).toContain("class:shimmer-endless={animateCursor}");
    // The ticking total must never be re-announced by a screen reader.
    expect(trail).toContain('<span class="trail-meta" aria-hidden="true">{meta || trail.elapsed}</span>');
    expect(trail).toContain('aria-live="polite"');
  });

  it("keeps throughput visible now that the composer line is gone", () => {
    for (const source of [conversation, research]) {
      expect(source).toContain("meta={status.meta}");
    }
  });

  it("only shows the elapsed clock while the run is actually going", () => {
    expect(trail).toContain("{#if trail.running}");
  });
});
