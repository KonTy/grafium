import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/components/Statistics.svelte"), "utf8");

describe("Statistics task undo wiring", () => {
  it("records undo entries for task text changes", () => {
    expect(source).toContain("getBlock");
    expect(source).toContain("function pushTaskContentUndo(");
    expect(source).toContain('type: "update_block"');
    expect(source).toContain("const afterContent = await updateTaskState(blockId, \"DONE\");");
    expect(source).toContain("const afterContent = await cycleTaskState(blockId);");
  });
});
