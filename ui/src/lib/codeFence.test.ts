import { describe, expect, it } from "vitest";
import { firstContentLine, isFencedCodeBlock, isInsideCodeFenceAt } from "./codeFence";

describe("isFencedCodeBlock", () => {
  it("detects backtick and tilde fences", () => {
    expect(isFencedCodeBlock("```bash\nsudo pacman -Syu\n```")).toBe(true);
    expect(isFencedCodeBlock("~~~\ncode\n~~~")).toBe(true);
  });

  it("ignores a leading list marker so Logseq-style fences still count", () => {
    expect(isFencedCodeBlock("- ```bash\nsudo pacman -Syu\n```")).toBe(true);
    expect(isFencedCodeBlock("1. ```\nls\n```")).toBe(true);
    expect(firstContentLine("- ```bash")).toBe("```bash");
  });

  it("does not treat ordinary text or inline ticks as a fence", () => {
    expect(isFencedCodeBlock("install `pacman`")).toBe(false);
    expect(isFencedCodeBlock("- a normal bullet")).toBe(false);
    expect(isFencedCodeBlock("")).toBe(false);
    expect(isFencedCodeBlock("```inline code```")).toBe(false);
    expect(isFencedCodeBlock("```bad`info\ntext")).toBe(false);
  });

  it("keeps the opening line end and body inside, without swallowing later prose", () => {
    const content = "Heading\n```js\ncode\n```\nOutside";
    for (const position of [content.indexOf("\ncode"), content.indexOf("code"), content.lastIndexOf("```")]) {
      expect(isInsideCodeFenceAt(content, position)).toBe(true);
    }
    expect(isInsideCodeFenceAt(content, 0)).toBe(false);
    expect(isInsideCodeFenceAt(content, content.length)).toBe(false);
  });

  it("requires a matching marker, sufficient length and an empty closing info string", () => {
    for (const opening of ["````md", "~~~~md"]) {
      const content = `${opening}\n\`\`\`\n~~~\n${opening.slice(0, 4)} not a closer\nstill code\n${opening.slice(0, 4)}\nOutside`;
      expect(isInsideCodeFenceAt(content, content.indexOf("still code"))).toBe(true);
      expect(isInsideCodeFenceAt(content, content.length)).toBe(false);
    }
  });
});
