import { describe, expect, it, vi } from "vitest";
import { StudyTracker } from "./studyTracker";
import type { StudyItem } from "./studies";

const item: StudyItem = {
  id: "one", kind: "page", title: "Health", topic: "Health", source: "page-id",
  progress: { position: 0, total: 0, anchor: "", label: "" }, createdAt: "", updatedAt: "",
};
function clock(kind: StudyItem["kind"] = "page") {
  let now = 0;
  const save = vi.fn().mockResolvedValue(undefined);
  const tracker = new StudyTracker({ ...item, kind }, save, () => now, () => "2026-10-01");
  const advance = (seconds: number) => {
    for (let i = 0; i < seconds; i++) { now += 1000; tracker.tick(); }
  };
  return { tracker, save, advance, jump: (ms: number) => { now += ms; tracker.tick(); } };
}
describe("study active-time clock", () => {
  it("stops exactly at 90 seconds without interaction and resumes without backfill", async () => {
    const c = clock();
    c.tracker.activity(); c.advance(120);
    expect(c.tracker.seconds).toBe(90);
    expect(c.tracker.state).toBe("idle");
    c.tracker.activity(); c.advance(5);
    await c.tracker.stop();
    expect(c.save).toHaveBeenCalledWith(95, "2026-10-01", null, expect.any(String));
  });
  it("does not count hidden, manually paused, stopped, or suspended time", () => {
    const c = clock();
    c.tracker.activity(); c.advance(3);
    c.tracker.setVisible(false); c.advance(10);
    c.tracker.setVisible(true); c.tracker.setPaused(true);
    c.tracker.activity(); c.advance(10);
    c.tracker.setPaused(false); c.jump(60_000); c.advance(2);
    expect(c.tracker.seconds).toBe(5);
    void c.tracker.stop(); c.advance(10);
    expect(c.tracker.seconds).toBe(5);
  });
  it("counts media playback only with fresh progress and excludes paused/buffering time", () => {
    const c = clock("video");
    c.tracker.playback(true); c.advance(2);
    c.tracker.updateProgress({ position: 2, total: 50, anchor: "", label: "0:02 / 0:50" });
    c.advance(10);
    expect(c.tracker.seconds).toBe(6);
    c.tracker.playback(false); c.advance(5);
    expect(c.tracker.seconds).toBe(6);
  });
  it("never estimates external website time but saves manual checkpoints", async () => {
    const c = clock("website");
    c.tracker.activity(); c.advance(10);
    c.tracker.updateProgress({ position: 0, total: 0, anchor: "", label: "Chapter 4" });
    await c.tracker.flush();
    expect(c.tracker.seconds).toBe(0);
    expect(c.save).toHaveBeenCalledWith(0, "2026-10-01", expect.objectContaining({ label: "Chapter 4" }), expect.any(String));
  });
  it("does not treat repeated playing-state reports as advancing media", () => {
    const c = clock("youtube");
    c.tracker.playback(true);
    for (let i = 0; i < 20; i++) {
      c.advance(1);
      c.tracker.playback(true);
    }
    expect(c.tracker.seconds).toBe(4);
    expect(c.tracker.state).toBe("idle");
  });
  it("retains failed writes for retry and serializes concurrent flushes", async () => {
    const c = clock();
    c.tracker.activity(); c.advance(5);
    c.save.mockRejectedValueOnce(new Error("disk full"));
    await expect(c.tracker.flush()).rejects.toThrow("disk full");
    await Promise.all([c.tracker.flush(), c.tracker.flush()]);
    expect(c.save).toHaveBeenCalledTimes(2);
    expect(c.save).toHaveBeenLastCalledWith(5, "2026-10-01", null, c.save.mock.calls[0][3]);
  });
});
