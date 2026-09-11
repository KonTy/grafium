import { describe, expect, it } from "vitest";
import { renderAssistantMarkdown, renderBlock } from "./markdown";

describe("markdown video embeds", () => {
  it("renders YouTube watch URLs from {{video ...}} blocks", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("{{video https://www.youtube.com/watch?v=DnoTA17O2sg}}");

    const iframe = host.querySelector("iframe");
    expect(iframe).not.toBeNull();
    expect(iframe?.getAttribute("src")).toBe("https://www.youtube-nocookie.com/embed/DnoTA17O2sg");
    expect(iframe?.getAttribute("loading")).toBe("lazy");
    expect(host.textContent).not.toContain("{{video");
  });

  it("renders short YouTube URLs too", () => {
    const html = renderBlock("{{video https://youtu.be/DnoTA17O2sg}}");

    expect(html).toContain("youtube-nocookie.com/embed/DnoTA17O2sg");
  });

  it("renders direct video files with controls", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("{{video assets/demo.mp4}}", "pages/Demo");

    const video = host.querySelector("video.grafium-video-file");
    expect(video).not.toBeNull();
    expect(video?.getAttribute("controls")).not.toBeNull();
    expect(video?.getAttribute("data-asset")).toBe("pages/Demo/assets/demo.mp4");
  });

  it("strips generated iframes from untrusted assistant markdown", () => {
    const host = document.createElement("div");
    host.innerHTML = renderAssistantMarkdown("{{video https://www.youtube.com/watch?v=DnoTA17O2sg}}");

    expect(host.querySelector("iframe")).toBeNull();
  });
});
