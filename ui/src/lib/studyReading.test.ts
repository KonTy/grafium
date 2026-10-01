import { afterEach, describe, expect, it, vi } from "vitest";
import { readingProgress, restoreReadingProgress, trackReading } from "./studyReading";

function fixture() {
  const main = document.createElement("main");
  main.innerHTML = '<div data-block-id="first"></div><div data-source-block-id="second"></div>';
  document.body.append(main);
  Object.defineProperties(main, { scrollHeight: { value: 1400 }, clientHeight: { value: 400 } });
  main.getBoundingClientRect = () => ({ top: 0 } as DOMRect);
  const [first, second] = [...main.children] as HTMLElement[];
  first.getBoundingClientRect = () => ({ top: -300, bottom: -10 } as DOMRect);
  second.getBoundingClientRect = () => ({ top: -10, bottom: 300 } as DOMRect);
  main.scrollTop = 310;
  return main;
}
afterEach(() => { document.body.replaceChildren(); vi.useRealTimers(); });

describe("study page resume", () => {
  it("stores a stable visible block plus offset and restores that position", () => {
    const main = fixture();
    const progress = readingProgress(main);
    expect(progress.position).toBe(310);
    expect(progress.total).toBe(1000);
    expect(JSON.parse(progress.anchor)).toEqual({ id: "second", offset: 10 });
    expect(restoreReadingProgress(main, progress)).toBe(true);
    expect(main.scrollTop).toBe(310);
  });
  it("uses proportional position when a saved block has been removed", () => {
    const main = fixture();
    const progress = readingProgress(main);
    main.children[1].remove();
    expect(restoreReadingProgress(main, { ...progress, total: 2000, position: 1000 })).toBe(false);
    expect(main.scrollTop).toBe(500);
  });
  it("does not save during restore and detaches activity when leaving the page", () => {
    vi.useFakeTimers();
    const main = fixture();
    const saved = readingProgress(main);
    const progress = vi.fn(), activity = vi.fn();
    const cleanup = trackReading(main, saved, progress, activity);
    main.dispatchEvent(new Event("scroll"));
    expect(progress).not.toHaveBeenCalled();
    vi.advanceTimersByTime(30);
    main.dispatchEvent(new Event("scroll"));
    expect(progress).toHaveBeenCalledOnce();
    cleanup();
    main.dispatchEvent(new Event("scroll"));
    expect(progress).toHaveBeenCalledOnce();
  });
});
