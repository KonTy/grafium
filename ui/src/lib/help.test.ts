import { describe, expect, it } from "vitest";
import { helpPageTitle, type HelpContext } from "./help";

describe("contextual help", () => {
  it("maps every supported context to a seeded help page", () => {
    const contexts: HelpContext[] = [
      "general",
      "editor",
      "journal",
      "graph",
      "flashcards",
      "tasks",
      "chat",
      "settings",
      "sync",
      "search",
    ];

    for (const context of contexts) {
      expect(helpPageTitle(context)).toMatch(/^Grafium Help$|^Help - /);
    }
  });
});
