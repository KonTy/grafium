import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_READER_THEME, isReaderTheme, observeReaderTheme, readReaderTheme } from "./bookReaderTheme";
import { readerFrameURL, readReaderMessage } from "./bookReaderSecurity";
import { applyBookTheme } from "./reader/theme";

const matrix = {
  background: "#000000", text: "#00ff41", link: "#80ffff",
  selectionBackground: "#00ff41", selectionText: "#000000",
};
afterEach(() => { document.documentElement.removeAttribute("style"); document.documentElement.className = ""; document.body.replaceChildren(); });

describe("isolated reader themes", () => {
  it("reads graph-independent root colors and observes changes with cleanup", async () => {
    expect(readReaderTheme()).toEqual(DEFAULT_READER_THEME);
    const themes: unknown[] = [];
    const cleanup = observeReaderTheme(theme => themes.push(theme));
    document.documentElement.style.cssText = "--bg-primary:#000000;--text-primary:#00ff41;--text-link:#80ffff;--accent:#00ff41";
    await Promise.resolve();
    expect(themes).toEqual([DEFAULT_READER_THEME, matrix]);
    document.documentElement.className = "new-theme";
    await Promise.resolve();
    expect(themes).toHaveLength(2);
    cleanup();
    document.documentElement.style.setProperty("--text-primary", "#ffffff");
    await Promise.resolve();
    expect(themes).toHaveLength(2);
  });
  it("allows literal colors only, including initial-frame CSS and script serialization", () => {
    const html = decodeURIComponent(readerFrameURL("safe-token", matrix));
    expect(html).toContain("background:#000000;color:#00ff41");
    expect(html).toContain("GRAFIUM_BOOK_THEME=");
    for (const background of ['red;}body{display:none', '</style><script>alert(1)</script>',
      'url(https://evil.test)', "var(--bg)", "currentColor", "inherit", "rgb(,)", "rgb(1 2)", null]) {
      const theme = { ...matrix, background };
      expect(isReaderTheme(theme)).toBe(false);
      expect(() => readerFrameURL("token", theme as typeof matrix)).toThrow("Invalid reader theme");
    }
    expect(isReaderTheme({ ...matrix, text: "rgb(0, 255, 65)", link: "hsl(120 100% 50%)" })).toBe(true);
  });
  it("observes stylesheet palettes selected by root class changes", async () => {
    const style = document.createElement("style");
    style.textContent = ":root.reader-test-palette{--bg-primary:#000000;--text-primary:#00ff41}";
    document.head.append(style);
    const themes: ReturnType<typeof readReaderTheme>[] = [];
    const cleanup = observeReaderTheme(theme => themes.push(theme));
    try {
      document.documentElement.className = "reader-test-palette";
      await Promise.resolve();
      expect(themes.at(-1)?.text).toBe("#00ff41");
      expect(themes.at(-1)?.background).toBe("#000000");
    } finally { cleanup(); style.remove(); }
  });
  it("recolors publisher-important prose and links without replacing text nodes or artwork", () => {
    document.body.innerHTML = `<div style="background:white;color:black">
      <p style="color:#000 !important;background:#fff !important">Words <a href="#note"><span>link</span></a></p>
      <div id="image-only" style="background:white"><img style="background:red" src="data:image/png;base64,AA=="></div>
      <svg style="background:white"><path fill="currentColor" d="M0 0h1v1z"/></svg>
      <div id="backdrop" style="background-image:url(blob:local);background-color:white">Caption</div>
    </div>`;
    const text = document.querySelector("p")!.firstChild;
    const svg = document.querySelector("svg")!;
    const fill = svg.querySelector("path")!.getAttribute("fill");
    applyBookTheme(document, matrix);
    expect(document.body.style.backgroundColor).toBe("rgb(0, 0, 0)");
    expect(document.querySelector("p")!.style.color).toBe("rgb(0, 255, 65)");
    expect(document.querySelector("p")!.style.backgroundColor).toBe("transparent");
    expect((document.querySelector("a span") as HTMLElement).style.color).toBe("rgb(128, 255, 255)");
    expect(document.querySelector("img")!.style.backgroundColor).toBe("red");
    expect((document.querySelector("#image-only") as HTMLElement).style.backgroundColor).toBe("white");
    expect((document.querySelector("#backdrop") as HTMLElement).style.backgroundImage).toContain("blob:local");
    expect(svg.style.color).toBe("rgb(0, 0, 0)");
    expect(svg.style.backgroundColor).toBe("white");
    expect(svg.querySelector("path")!.getAttribute("fill")).toBe(fill);
    applyBookTheme(document, { ...matrix, text: "#eeeeee" }, 150);
    expect(document.querySelector("p")!.firstChild).toBe(text);
    expect(document.querySelector("p")!.style.color).toBe("rgb(238, 238, 238)");
    expect(svg.style.color).toBe("rgb(0, 0, 0)");
  });
  it("accepts only the three bounded control signals from the authenticated opaque frame", () => {
    for (const type of ["toggle-controls", "exit-fullscreen", "toggle-fullscreen"]) {
      const data = { channel: "grafium-book", token: "secret", type };
      const event = { source: window, origin: "null", data } as unknown as MessageEvent;
      expect(readReaderMessage(event, window, "secret")).toEqual({ type });
      expect(readReaderMessage({ ...event, data: { ...data, command: "anything" } } as MessageEvent, window, "secret")).toBeNull();
      expect(readReaderMessage(event, window, "wrong")).toBeNull();
      expect(readReaderMessage({ ...event, origin: "https://app.test" } as MessageEvent, window, "secret")).toBeNull();
      expect(readReaderMessage({ ...event, source: {} } as MessageEvent, window, "secret")).toBeNull();
    }
  });
  it("scales captured publisher typography once and restores inline declarations at 100%", () => {
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const chapter = frame.contentDocument!;
    chapter.body.innerHTML = `<h1 style="font-size:24px">Heading</h1>
      <p style="font-size:10px !important;line-height:14px !important">Small publisher text
        <span style="font-size:8px">Smaller text</span></p>
      <svg style="font-size:12px"><text>Diagram</text></svg>`;
    const paragraph = chapter.querySelector("p")!;
    for (const size of [200, 200, 150, 200]) {
      applyBookTheme(chapter, matrix, size);
      expect(parseFloat(paragraph.style.fontSize)).toBe(10 * size / 100);
      expect(parseFloat(paragraph.style.lineHeight)).toBe(14 * size / 100);
      expect(parseFloat(chapter.querySelector("h1")!.style.fontSize)).toBe(24 * size / 100);
      expect(parseFloat(chapter.querySelector("span")!.style.fontSize)).toBe(8 * size / 100);
      expect(chapter.querySelector("svg")!.style.fontSize).toBe("12px");
    }
    applyBookTheme(chapter, matrix, 100);
    expect(paragraph.style.fontSize).toBe("10px");
    expect(paragraph.style.getPropertyPriority("font-size")).toBe("important");
    expect(paragraph.style.lineHeight).toBe("14px");
    expect(chapter.querySelector("h1")!.style.getPropertyPriority("font-size")).toBe("");
  });
});
