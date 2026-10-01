import { describe, expect, it } from "vitest";
import { finiteStudyProgress, localStudyDay, normalizeStudySource, studyPercent, studyTime, youtubeVideoId } from "./studySources";

describe("study sources", () => {
  it.each([
    "https://youtu.be/dQw4w9WgXcQ?t=12",
    "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=playlist",
    "https://m.youtube.com/shorts/dQw4w9WgXcQ",
    "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
  ])("normalizes a single YouTube video: %s", url => {
    expect(youtubeVideoId(url)).toBe("dQw4w9WgXcQ");
    expect(normalizeStudySource("youtube", url)).toBe("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  });
  it.each([
    "javascript:alert(1)", "file:///etc/passwd", "https://youtube.com.evil.test/watch?v=dQw4w9WgXcQ",
    "https://youtu.be/short", "https://youtube.com/watch?v=dQw4w9WgXcQ&v=abcdefghijk",
    "https://youtube.com/playlist?list=dQw4w9WgXcQ", "https://youtu.be/dQw4w9WgXcQ/extra",
    "https://youtube.com:444/watch?v=dQw4w9WgXcQ", "https://user@youtube.com/watch?v=dQw4w9WgXcQ",
  ])("rejects malformed or unsafe YouTube URL: %s", url => {
    expect(() => youtubeVideoId(url)).toThrow();
  });
  it.each(["javascript:alert(1)", "data:text/html,x", "file:///graph/assets/audio.mp3", "//evil.test/video",
    "../assets/audio.mp3", "assets/../audio.mp3", "assets/%2e%2e/audio.mp3", "/etc/passwd",
    "assets\\audio.mp3", "assets/%252e%252e/audio.mp3", "https://user:pass@example.com/a",
    "https://example.com\\@evil.test/a", "assets/a\n.mp3",
  ])("rejects unsafe media source: %s", value => {
    expect(() => normalizeStudySource("audio", value)).toThrow();
  });
  it("accepts graph paths and direct http(s) media without making requests", () => {
    expect(normalizeStudySource("audio", " assets/Lesson%201.mp3 ")).toBe("assets/Lesson 1.mp3");
    expect(normalizeStudySource("video", "https://example.com/lesson.mp4")).toBe("https://example.com/lesson.mp4");
    expect(normalizeStudySource("website", "https://example.com")).toBe("https://example.com/");
    expect(normalizeStudySource("flashcards", "")).toBe("");
  });
  it("bounds progress and formats finite time", () => {
    expect(finiteStudyProgress(Infinity, NaN)).toEqual({ position: 0, total: 0, label: "", anchor: "" });
    expect(finiteStudyProgress(200, 100).position).toBe(100);
    expect(studyPercent(finiteStudyProgress(25, 100))).toBe(25);
    expect(studyTime(3661)).toBe("1h 1m");
    expect(studyTime(NaN)).toBe("0s");
    expect(localStudyDay(new Date(2026, 8, 30, 23))).toBe("2026-09-30");
  });
});
