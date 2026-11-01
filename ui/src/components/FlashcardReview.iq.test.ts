import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, unmount, flushSync } from "svelte";

const api = vi.hoisted(() => ({
  packs: vi.fn(), importPack: vi.fn(), topics: vi.fn(), due: vi.fn(), open: vi.fn(),
}));
vi.mock("../lib/api", () => ({
  listFlashcardsDue: api.due, listFlashcardTopics: api.topics, gradeFlashcard: vi.fn(),
}));
vi.mock("../lib/iq", () => ({
  listIqPacks: api.packs, importFlashcardPack: api.importPack,
  startIqSession: vi.fn(), answerIqItem: vi.fn(), finishIqSession: vi.fn(),
}));
vi.mock("../lib/markdown", () => ({
  renderBlock: (text: string) => text, hydrateAssetMedia: vi.fn(), assetBaseDirFor: () => undefined,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: api.open }));
import FlashcardReview from "./FlashcardReview.svelte";

let component: ReturnType<typeof mount> | undefined;
const iqPack = { id: "pack", title: "Reasoning fixture", itemCount: 2, norms: null };
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.clearAllMocks();
});
function setup(packs = [iqPack]) {
  api.packs.mockResolvedValue(packs);
  api.topics.mockResolvedValue([]);
  api.due.mockResolvedValue([]);
  component = mount(FlashcardReview, { target: document.body });
}
describe("IQ packs in flashcards", () => {
  it("opens IQ-marked packs when no recall cards are due", async () => {
    setup();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Reasoning fixture"));
    document.querySelector<HTMLButtonElement>(".topic")!.click(); flushSync();
    expect(document.querySelector('[data-help-context="iq"]')).not.toBeNull();
    expect(document.body.textContent).toContain("reasoning score, not IQ");
    expect(document.querySelector(".grade")).toBeNull();
  });
  it("imports a marked pack through the new import command", async () => {
    setup([]);
    api.open.mockResolvedValue("/packs/fixture.json");
    api.importPack.mockResolvedValue({ iq: iqPack, anki: null });
    await vi.waitFor(() => expect(document.body.textContent).toContain("No flashcards yet"));
    document.querySelector<HTMLButtonElement>(".import-btn")!.click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("2 assessment questions"));
    expect(api.importPack).toHaveBeenCalledWith("/packs/fixture.json");
    expect(api.open).toHaveBeenCalledWith(expect.objectContaining({
      filters: [{ name: "Flashcard or IQ-marked pack", extensions: ["apkg", "json"] }],
    }));
  });
  it("retains the ordinary Anki import result", async () => {
    setup([]);
    api.open.mockResolvedValue("/packs/history.apkg");
    api.importPack.mockResolvedValue({ iq: null, anki: { page_title: "History", card_count: 7, topic: "history" } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("No flashcards yet"));
    document.querySelector<HTMLButtonElement>(".import-btn")!.click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("7 cards (#history)"));
  });
});
