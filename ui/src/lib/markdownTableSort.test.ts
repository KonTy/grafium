import { describe, expect, it } from "vitest";
import { sortMarkdownTableColumn } from "./markdownTableSort";

const TABLE = [
  "| Name | Amount |",
  "| ---- | ------ |",
  "| Zinc | 12 |",
  "| Iron | 3 |",
  "| B12 | 8 |",
].join("\n");

describe("sortMarkdownTableColumn", () => {
  it("sorts body rows by a text column without rewriting the header", () => {
    const next = sortMarkdownTableColumn(TABLE, 0, 0, "asc");
    expect(next).toBe(
      [
        "| Name | Amount |",
        "| ---- | ------ |",
        "| B12 | 8 |",
        "| Iron | 3 |",
        "| Zinc | 12 |",
      ].join("\n")
    );
  });

  it("sorts numeric cells numerically, not lexicographically", () => {
    const next = sortMarkdownTableColumn(TABLE, 0, 1, "asc");
    expect(next).toBe(
      [
        "| Name | Amount |",
        "| ---- | ------ |",
        "| Iron | 3 |",
        "| B12 | 8 |",
        "| Zinc | 12 |",
      ].join("\n")
    );
  });

  it("sorts loose pipe tables that have no delimiter row", () => {
    const next = sortMarkdownTableColumn(
      [
        "| Food | Mg |",
        "| Heart | 500 |",
        "| Fish | 200 |",
      ].join("\n"),
      0,
      1,
      "asc",
    );
    expect(next).toBe(
      [
        "| Food | Mg |",
        "| Fish | 200 |",
        "| Heart | 500 |",
      ].join("\n")
    );
  });

  it("toggles to descending and leaves fenced fake tables alone", () => {
    const content = [
      TABLE,
      "",
      "```",
      "| Nope | 9 |",
      "| ---- | --- |",
      "| Z | 1 |",
      "```",
      "",
      "| City | Pop |",
      "| ---- | --- |",
      "| Oslo | 1 |",
      "| Rome | 2 |",
    ].join("\n");
    const next = sortMarkdownTableColumn(content, 1, 1, "desc");
    expect(next).toContain("| Rome | 2 |");
    expect(next?.indexOf("| Rome | 2 |")).toBeLessThan(next?.indexOf("| Oslo | 1 |") ?? 0);
    expect(next).toContain("```\n| Nope | 9 |");
  });
});
