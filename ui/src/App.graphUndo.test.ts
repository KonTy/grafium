import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const app = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");
const api = readFileSync(join(process.cwd(), "src/lib/api.ts"), "utf8");

describe("graph-bound undo integration", () => {
  it("resets app history through the same guarded path for opening and creating graphs", () => {
    expect(api).toContain('runGraphChange(() => invoke<GraphInfo>("open_graph", { path }))');
    expect(api).toContain('runGraphChange(() => invoke<GraphInfo>("create_graph", { path, name }))');
  });

  it("remounts editors on graph change so copied IDs cannot reuse CodeMirror history", () => {
    expect(app).toContain("function handleGraphChanged() {\n    graphGeneration += 1;");
    const main = app.slice(app.indexOf("<main "), app.indexOf("</main>"));
    expect(main.indexOf("{#key graphGeneration}")).toBeGreaterThan(-1);
    expect(main.indexOf("{#key graphGeneration}")).toBeLessThan(main.indexOf("<JournalView"));
    expect(main.indexOf("{#key graphGeneration}")).toBeLessThan(main.indexOf("<PageContent"));
  });

  it("keeps global private playback outside graph-bound editor remounts", () => {
    const toolbar = app.indexOf("<PrivateReaderToolbar ");
    expect(toolbar).toBeGreaterThan(-1);
    expect(toolbar).toBeLessThan(app.indexOf("<main "));
    const handler = app.slice(app.indexOf("function handleGraphChanged()"), app.indexOf("function handleGraphChanged()") + 220);
    expect(handler).toContain("privateBookId = null;");
  });
});
