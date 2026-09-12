import { describe, expect, it } from "vitest";
import { firstContentLine, isFencedCodeBlock } from "./codeFence";

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
  });
});
