import { describe, expect, it } from "vitest";
import { formatBlocksAsOutlineMarkdown, formatBlocksAsPlainText } from "./blockClipboard";

describe("block clipboard formatting", () => {
  it("formats selected blocks as relative outline markdown", () => {
    expect(formatBlocksAsOutlineMarkdown([
      { content: "[[Tech/Android/Backup]]", depth: 0 },
      { content: "TODO Stocks", depth: 1 },
      { content: "DONE Logseq\nCLOSED: [2026-09-09 Wed 11:10]", depth: 1 },
    ])).toBe([
      "- [[Tech/Android/Backup]]",
      "  - TODO Stocks",
      "  - DONE Logseq",
      "    CLOSED: [2026-09-09 Wed 11:10]",
    ].join("\n"));
  });

  it("normalizes a child-only selection to the top paste depth", () => {
    expect(formatBlocksAsOutlineMarkdown([
      { content: "TODO Stocks", depth: 2 },
      { content: "TODO Pictures", depth: 2 },
    ])).toBe([
      "- TODO Stocks",
      "- TODO Pictures",
    ].join("\n"));
  });

  it("formats selected blocks as plain readable text", () => {
    expect(formatBlocksAsPlainText([
      { content: "[[Tech/Android/Backup]]", depth: 0 },
      { content: "TODO Stocks", depth: 1 },
    ])).toBe("[[Tech/Android/Backup]]\nTODO Stocks");
  });
});
