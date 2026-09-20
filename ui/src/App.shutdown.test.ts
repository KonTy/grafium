import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");

describe("application shutdown notice", () => {
  it("shows a blocking status dialog when native shutdown starts", () => {
    expect(source).toContain('listen("app-shutdown-started"');
    expect(source).toContain("shuttingDown = true;");
    expect(source).toContain('{#if shuttingDown}');
    expect(source).toContain('aria-labelledby="shutdown-title"');
    expect(source).toContain("Saving your place and releasing local AI models…");
  });
});
