import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, unmount, flushSync } from "svelte";
const api = vi.hoisted(() => ({
  due: vi.fn(), topics: vi.fn(), grade: vi.fn(),
}));
vi.mock("../lib/api", () => ({
  listFlashcardsDue: api.due, listFlashcardTopics: api.topics, gradeFlashcard: api.grade,
  importAnkiApkg: vi.fn(),
}));
vi.mock("../lib/markdown", () => ({
  renderBlock: (text: string) => text, hydrateAssetMedia: vi.fn(), assetBaseDirFor: () => undefined,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
import FlashcardReview from "./FlashcardReview.svelte";
let component: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.clearAllMocks();
});
describe("Studies flashcard entry", () => {
  it("opens the exact due topic and reports successful grades without changing the schedule", async () => {
    api.due.mockResolvedValue([{ id: "card", front: "Question", back: "Answer" }]);
    api.grade.mockResolvedValue(undefined);
    const activity = vi.fn(), progress = vi.fn(), complete = vi.fn(), exit = vi.fn();
    component = mount(FlashcardReview, { target: document.body, props: {
      initialTopic: "chinese", onStudyActivity: activity, onStudyProgress: progress,
      onStudyComplete: complete, onExit: exit,
    } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("Question"));
    expect(api.due).toHaveBeenCalledWith(100, "chinese");
    expect(api.topics).not.toHaveBeenCalled();
    document.querySelector<HTMLButtonElement>(".reveal")!.click();
    flushSync();
    document.querySelector<HTMLButtonElement>(".grade.good")!.click();
    await vi.waitFor(() => expect(complete).toHaveBeenCalledOnce());
    expect(api.grade).toHaveBeenCalledWith("card", 4);
    expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 1, total: 1 }));
    expect(activity).toHaveBeenCalledTimes(3);
    document.querySelector<HTMLButtonElement>(".back")!.click();
    expect(exit).toHaveBeenCalledOnce();
  });
  it("preserves the empty-string untagged deck instead of mixing all cards", async () => {
    api.due.mockResolvedValue([]);
    const complete = vi.fn();
    component = mount(FlashcardReview, { target: document.body, props: { initialTopic: "", onStudyComplete: complete } });
    await vi.waitFor(() => expect(complete).toHaveBeenCalledOnce());
    expect(api.due).toHaveBeenCalledWith(100, "");
  });
});
