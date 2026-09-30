import { describe, expect, it } from "vitest";
import { helpPageTitle, isHelpContext, loadHelpPage, type HelpContext } from "./help";
import { vi } from "vitest";
import chatHelp from "../../src-tauri/resources/welcome/pages/Help - Chat.md?raw";
import aiHelp from "../../src-tauri/resources/welcome/pages/AI Setup And Privacy.md?raw";
import appSource from "../App.svelte?raw";
import recoverySource from "../components/RuntimeRecovery.svelte?raw";

const api = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue("AI help") }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));

describe("contextual help", () => {
  it("routes F1 in recovery controls to current bundled AI guidance", async () => {
    expect(helpPageTitle("ai")).toBe("AI Setup And Privacy");
    expect(isHelpContext("ai")).toBe(true);
    expect(isHelpContext("constructor")).toBe(false);
    expect(recoverySource).toContain('data-help-context="ai"');
    expect(appSource).toContain('closest("[data-help-context]")');
    await loadHelpPage("ai");
    expect(api.invoke).toHaveBeenCalledWith("help_get_page", { context: "ai" });
  });
  it("links Chat's runtime warnings to GPU fallback and shutdown guidance", () => {
    expect(helpPageTitle("chat")).toBe("Help - Chat");
    expect(chatHelp).toContain("[[AI Setup And Privacy]]");
    expect(chatHelp).toContain("GPU");
    expect(aiHelp).toContain("queued and active");
    expect(aiHelp).toContain("not GPU driver or");
    expect(aiHelp).toContain("uses CPU");
  });
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
      "ai",
      "sync",
      "search",
    ];

    for (const context of contexts) {
      expect(helpPageTitle(context)).toMatch(/^Grafium Help$|^Help - |^AI Setup And Privacy$/);
    }
  });
});
