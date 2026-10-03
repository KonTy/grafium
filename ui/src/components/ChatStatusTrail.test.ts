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

describe("Status trail action icons", () => {
  it("gives every step a distinct, decorative icon instead of a bullet", async () => {
    const { STEP_ICONS } = await import("../lib/chatStepIcons");
    const drawings = Object.values(STEP_ICONS).map((paths) => paths.join(" "));
    expect(drawings.every((drawing) => drawing.trim().length > 0)).toBe(true);
    expect(new Set(drawings).size).toBe(drawings.length);
    expect(trail).not.toContain("trail-mark");
    expect(trail).toContain('data-step-icon={name}');
    expect(trail).toContain('aria-hidden="true" focusable="false"');
    expect(trail).toContain("{@render icon(row.phase)}");
    expect(trail).toContain('{@render icon("elapsed")}');
  });

  it("renders phase icons with readable labels for live and finished rows", async () => {
    const { mount, unmount, flushSync } = await import("svelte");
    const { default: ChatStatusTrail } = await import("./ChatStatusTrail.svelte");
    const host = document.createElement("div");
    document.body.append(host);
    const rows = [
      { key: "0", phase: "retrieving", label: "Searching your notes", note: "", meta: "", state: "done", shimmer: false },
      { key: "1", phase: "generating", label: "Generating", note: "", meta: "", state: "active", shimmer: true },
    ] as const;
    const component = mount(ChatStatusTrail, {
      target: host,
      props: { trail: { rows: [...rows], elapsed: "3s", finishedIn: "3s", running: true, any: true } },
    });
    flushSync();
    try {
      const items = [...host.querySelectorAll("li")];
      expect(items.map((item) => item.querySelector("svg")?.dataset.stepIcon))
        .toEqual(["retrieving", "generating", "elapsed"]);
      expect(items.map((item) => item.textContent?.trim())).toEqual(["Searching your notes", "Generating", "3s"]);
      expect([...host.querySelectorAll("svg")].every((svg) => svg.getAttribute("aria-hidden") === "true")).toBe(true);
      expect(items[1].classList.contains("active")).toBe(true);
    } finally {
      await unmount(component);
      host.remove();
    }
  });
});
