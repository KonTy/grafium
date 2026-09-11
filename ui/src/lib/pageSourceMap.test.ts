import { describe, expect, it } from "vitest";
import {
  blockIntersectsSourceRange,
  findSourceBlockAtPosition,
  parsePageSourceMap,
  sourceBlockContentReplacement,
} from "./pageSourceMap";

describe("page source block map", () => {
  it("maps outline source blocks to explicit ids and ranges", () => {
    const source = [
      "title:: Demo",
      "",
      "- Parent **raw**",
      "  id:: parent-id",
      "  - Child",
      "    id:: child-id",
      "- Sibling",
      "  id:: sibling-id",
      "",
    ].join("\n");

    const map = parsePageSourceMap(source);

    expect(map.blocks.map((block) => [block.id, block.depth, block.content])).toEqual([
      ["parent-id", 0, "Parent **raw**"],
      ["child-id", 1, "Child"],
      ["sibling-id", 0, "Sibling"],
    ]);
    expect(map.idLines.map((line) => line.value)).toEqual(["parent-id", "child-id", "sibling-id"]);
    expect(map.blocks[0].ownTo).toBeLessThan(map.blocks[1].ownFrom);
    expect(map.blocks[0].subtreeTo).toBe(map.blocks[1].ownTo);
  });

  it("keeps multiline content separate from id metadata", () => {
    const source = [
      "- ```rust",
      "  let id = \"not metadata\";",
      "  ```",
      "  id:: code-id",
      "- Next",
      "  id:: next-id",
    ].join("\n");

    const map = parsePageSourceMap(source);

    expect(map.blocks[0].id).toBe("code-id");
    expect(map.blocks[0].content).toBe("```rust\nlet id = \"not metadata\";\n```");
    expect(map.blocks[0].contentSegments).toHaveLength(3);
    expect(map.blocks[0].idLine?.line.text).toBe("  id:: code-id");
  });

  it("finds the active block at cursor positions", () => {
    const source = [
      "- Alpha",
      "  id:: alpha-id",
      "- Beta",
      "  second line",
      "  id:: beta-id",
    ].join("\n");
    const map = parsePageSourceMap(source);

    const betaContinuationPosition = source.indexOf("second line") + "second".length;
    const beta = findSourceBlockAtPosition(map, betaContinuationPosition);

    expect(beta?.id).toBe("beta-id");
    expect(blockIntersectsSourceRange(map.blocks[0], betaContinuationPosition, betaContinuationPosition)).toBe(false);
    expect(blockIntersectsSourceRange(map.blocks[1], betaContinuationPosition, betaContinuationPosition)).toBe(true);
  });

  it("builds content replacements that preserve following id metadata", () => {
    const source = [
      "- ![Figure](assets/one.png)",
      "  caption line",
      "  id:: image-id",
      "- Next",
    ].join("\n");
    const map = parsePageSourceMap(source);
    const replacement = sourceBlockContentReplacement(
      map.blocks[0],
      "![Figure](assets/one.png){:width 320}\ncaption line",
    );
    const nextSource = source.slice(0, replacement.from) + replacement.insert + source.slice(replacement.to);

    expect(nextSource).toBe([
      "- ![Figure](assets/one.png){:width 320}",
      "  caption line",
      "  id:: image-id",
      "- Next",
    ].join("\n"));
  });

  it("preserves custom property lines interleaved before later content", () => {
    const source = [
      "- ![Figure](assets/one.png)",
      "  custom:: keep",
      "  caption line",
      "  id:: image-id",
      "- Next",
    ].join("\n");
    const map = parsePageSourceMap(source);
    const replacement = sourceBlockContentReplacement(
      map.blocks[0],
      "![Figure](assets/one.png){:width 320}\ncaption line",
    );
    const nextSource = source.slice(0, replacement.from) + replacement.insert + source.slice(replacement.to);

    expect(nextSource).toBe([
      "- ![Figure](assets/one.png){:width 320}",
      "  custom:: keep",
      "  caption line",
      "  id:: image-id",
      "- Next",
    ].join("\n"));
  });

  it("uses the existing continuation indent when replacing multiline content", () => {
    const source = [
      "\t- first line",
      "\t\tsecond line",
      "\t\tid:: tab-id",
    ].join("\n");
    const map = parsePageSourceMap(source);
    const replacement = sourceBlockContentReplacement(map.blocks[0], "alpha\nbeta");

    expect(replacement.insert).toBe("\t- alpha\n\t\tbeta");
  });
});
