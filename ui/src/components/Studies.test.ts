import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import type { StudyItem } from "../lib/studies";
import { localStudyDay } from "../lib/studySources";

const api = vi.hoisted(() => ({
  listStudies: vi.fn(), saveStudy: vi.fn(), removeStudy: vi.fn(),
  listPageSummaries: vi.fn(), getPage: vi.fn(), listFlashcardTopics: vi.fn(), listAssets: vi.fn(),
}));
vi.mock("../lib/studies", () => ({ listStudies: api.listStudies, saveStudy: api.saveStudy, removeStudy: api.removeStudy }));
vi.mock("../lib/api", () => ({ listPageSummaries: api.listPageSummaries, getPage: api.getPage, listFlashcardTopics: api.listFlashcardTopics, listAssets: api.listAssets }));
vi.mock("../lib/books", () => ({ isOriginalBookPage: (page: { properties: Record<string, unknown> }) => page.properties["book-id"] === "original" }));
import Studies from "./Studies.svelte";

const fixture = (extra: Partial<StudyItem> = {}): StudyItem => ({
  id: "study", title: "French lesson", topic: "Languages", kind: "audio", source: "assets/french.mp3",
  progress: { position: 20, total: 100, anchor: "", label: "20%" }, createdAt: "", updatedAt: "", ...extra,
});
let component: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); });
beforeEach(() => {
  vi.resetAllMocks();
  api.listStudies.mockResolvedValue({ items: [], days: [] });
  api.listPageSummaries.mockResolvedValue([{ id: "page", title: "Physics", is_journal: false }]);
  api.getPage.mockResolvedValue({ id: "page", title: "Physics", properties: {} });
  api.listFlashcardTopics.mockResolvedValue([{ topic: "physics", total: 10, due: 2 }]);
  api.listAssets.mockResolvedValue(["assets/lesson.mp3", "assets/movie.mp4"]);
  api.saveStudy.mockImplementation(async (_graph, item) => item);
});
function button(text: string) {
  return [...document.querySelectorAll<HTMLButtonElement>("button")].find(element => element.textContent?.trim() === text)!;
}
function input(label: string, value: string) {
  const field = [...document.querySelectorAll("label")].find(element => element.textContent?.startsWith(label) && element.querySelector("input"))!.querySelector("input")!;
  field.value = value; field.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
}

describe("Studies library", () => {
  it("filters statistics, opens a study, edits its topic, and confirms source-preserving removal", async () => {
    const item = fixture();
    api.listStudies.mockResolvedValue({ items: [item, fixture({ id: "other", title: "Other", topic: "Science" })],
      days: [{ itemId: item.id, topic: item.topic, day: localStudyDay(), seconds: 90 }, { itemId: "other", topic: "Science", day: localStudyDay(), seconds: 600 }] });
    const onOpen = vi.fn();
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen } });
    await vi.waitFor(() => expect(button("French lesson")).toBeTruthy());
    const filter = document.querySelector<HTMLSelectElement>(".library-heading select")!;
    filter.value = "topic:Languages"; filter.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(document.querySelectorAll(".study-row")).toHaveLength(1);
    expect(document.querySelector(".stats")?.textContent).toContain("1m 30s");
    button("Continue").click(); expect(onOpen).toHaveBeenCalledWith(item);
    document.querySelector<HTMLButtonElement>(".topic")!.click(); flushSync();
    input("Topic", "French"); button("Save topic").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({ id: "study", topic: "French" })));
    await vi.waitFor(() => expect(document.querySelector(".topic-edit")).toBeNull());
    filter.value = ""; filter.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    document.querySelector<HTMLButtonElement>('[aria-label="Remove French lesson from Studies"]')!.click(); flushSync();
    expect(document.body.textContent).toContain("will not be deleted");
    expect(api.removeStudy).not.toHaveBeenCalled();
    button("Remove study entry").click();
    await vi.waitFor(() => expect(api.removeStudy).toHaveBeenCalledWith("/graph", "study"));
  });
  it("prefills an original book and displays duplicate and save errors", async () => {
    const page = { id: "book-page", title: "Original book", properties: { "book-id": "original" } };
    api.listStudies.mockResolvedValue({ items: [fixture({ kind: "book", source: page.id })], days: [] });
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn(), addPage: page as never } });
    await vi.waitFor(() => expect(button("Add study")?.disabled).toBe(false));
    expect(document.querySelector<HTMLSelectElement>(".form-grid select")?.value).toBe("book");
    button("Add study").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("already in Studies"));
    expect(api.saveStudy).not.toHaveBeenCalled();
  });
  it("chooses a page using its persisted ID and does not silently ignore a save failure", async () => {
    api.saveStudy.mockRejectedValue(new Error("Disk full"));
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    await vi.waitFor(() => expect(button("Physics")).toBeTruthy());
    button("Physics").click();
    await vi.waitFor(() => expect(document.querySelector<HTMLInputElement>('.form-grid input')?.value).toBe("Physics"));
    input("Topic", "Natural sciences");
    button("Add study").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Disk full"));
    expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({ source: "page", kind: "page", topic: "Natural sciences" }));
  });
  it("ignores an old graph's late list response", async () => {
    let finish: (value: unknown) => void = () => {};
    api.listStudies.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    api.listStudies.mockResolvedValueOnce({ items: [fixture({ title: "New graph lesson" })], days: [] });
    const state = new SvelteMap([["graph", "/old"]]);
    component = mount(Studies, { target: document.body, props: {
      get graphPath() { return state.get("graph")!; }, onOpen: vi.fn(),
    } });
    flushSync(); state.set("graph", "/new"); flushSync();
    await vi.waitFor(() => expect(button("New graph lesson")).toBeTruthy());
    finish({ items: [fixture({ title: "Old graph lesson" })], days: [] });
    await Promise.resolve(); flushSync();
    expect(button("New graph lesson")).toBeTruthy();
    expect(button("Old graph lesson")).toBeUndefined();
  });
  it("exposes a Ctrl+F text filter and uses the progress returned by metadata saves", async () => {
    api.listStudies.mockResolvedValue({ items: [fixture(), fixture({ id: "other", title: "Physics" })], days: [] });
    api.saveStudy.mockImplementation(async (_graph, item) => ({
      ...item, progress: { position: 80, total: 100, anchor: "", label: "Latest progress: 80%" },
    }));
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(button("French lesson")).toBeTruthy());
    const search = document.querySelector<HTMLInputElement>("[data-local-search]")!;
    expect(typeof search.select).toBe("function");
    search.value = "French"; search.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
    expect(document.querySelectorAll(".study-row")).toHaveLength(1);
    document.querySelector<HTMLButtonElement>(".topic")!.click(); flushSync();
    input("Topic", "French"); button("Save topic").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Latest progress: 80%"));
    expect(document.querySelector("progress")?.value).toBe(80);
  });
  it("renders an independent Topic column with General for unassigned entries", async () => {
    api.listStudies.mockResolvedValue({ items: [fixture({ topic: "" }), fixture({ id: "other", topic: "General" })], days: [] });
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(document.querySelectorAll(".topic-cell")).toHaveLength(2));
    expect(document.querySelector(".column-headings")?.textContent).toContain("Topic");
    expect(document.querySelector(".details .topic")).toBeNull();
    expect([...document.querySelectorAll(".topic")].every(element => element.textContent?.includes("General"))).toBe(true);
    const filter = document.querySelector<HTMLSelectElement>(".library-heading select")!;
    expect([...filter.options].filter(option => option.textContent === "General")).toHaveLength(1);
    filter.value = "topic:General"; filter.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(document.querySelectorAll(".study-row")).toHaveLength(2);
  });
  it("suggests existing graph assets while allowing a directly entered media source", async () => {
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    const kind = document.querySelector<HTMLSelectElement>(".form-grid select")!;
    kind.value = "audio"; kind.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    await vi.waitFor(() => expect(document.querySelectorAll("datalist option")).toHaveLength(2));
    const source = [...document.querySelectorAll("label")].find(element => element.textContent?.startsWith("Media URL"))!.querySelector("input")!;
    expect(source.list).toBe(document.querySelector("datalist"));
    input("Title", "Listening lesson"); input("Media URL", "assets/lesson.mp3");
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      source: "assets/lesson.mp3", kind: "audio", topic: "General",
    })));
  });
});
