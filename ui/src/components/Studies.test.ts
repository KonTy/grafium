import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import type { StudyItem } from "../lib/studies";
import { localStudyDay } from "../lib/studySources";

const api = vi.hoisted(() => ({
  listStudies: vi.fn(), saveStudy: vi.fn(), removeStudy: vi.fn(), fetchStudyLinkTitle: vi.fn(),
  listPageSummaries: vi.fn(), getPage: vi.fn(), listFlashcardTopics: vi.fn(), listAssets: vi.fn(),
  addPrivateLibraryLink: vi.fn(), refreshPrivateLibrary: vi.fn(),
}));
vi.mock("../lib/studies", () => ({ listStudies: api.listStudies, saveStudy: api.saveStudy, removeStudy: api.removeStudy, fetchStudyLinkTitle: api.fetchStudyLinkTitle }));
vi.mock("../lib/api", () => ({ listPageSummaries: api.listPageSummaries, getPage: api.getPage, listFlashcardTopics: api.listFlashcardTopics, listAssets: api.listAssets }));
vi.mock("../lib/books", () => ({ isOriginalBookPage: (page: { properties: Record<string, unknown> }) => page.properties["book-id"] === "original" }));
vi.mock("../lib/privateReader", async () => {
  const { writable } = await import("svelte/store");
  return {
    privateLibrary: writable({ libraryPath: null, books: [] }),
    refreshPrivateLibrary: api.refreshPrivateLibrary, addPrivateLibraryLink: api.addPrivateLibraryLink,
    bookmarkLabel: () => "Saved Library position",
  };
});
import { privateLibrary, type ReaderBook } from "../lib/privateReader";
import Studies from "./Studies.svelte";

const libraryId = "12345678-1234-4234-8234-123456789abc";
const libraryBook = (extra: Partial<ReaderBook> = {}): ReaderBook => ({
  id: libraryId, title: "Private lesson", kind: "audio", available: true, tracks: [], position: null, bookmarks: [], ...extra,
});
const fixture = (extra: Partial<StudyItem> = {}): StudyItem => ({
  id: "study", title: "French lesson", topic: "Languages", kind: "audio", source: "assets/french.mp3",
  progress: { position: 20, total: 100, anchor: "", label: "20%" }, createdAt: "", updatedAt: "", ...extra,
});
let component: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); });
beforeEach(() => {
  vi.resetAllMocks();
  privateLibrary.set({ libraryPath: null, books: [] });
  api.refreshPrivateLibrary.mockResolvedValue(undefined);
  api.addPrivateLibraryLink.mockImplementation(async (title, kind, sourceUrl) => libraryBook({ title, kind, sourceUrl }));
  api.listStudies.mockResolvedValue({ items: [], days: [] });
  api.listPageSummaries.mockResolvedValue([{ id: "page", title: "Physics", is_journal: false }]);
  api.getPage.mockResolvedValue({ id: "page", title: "Physics", properties: {} });
  api.listFlashcardTopics.mockResolvedValue([{ topic: "physics", total: 10, due: 2 }]);
  api.listAssets.mockResolvedValue(["assets/lesson.mp3", "assets/movie.mp4"]);
  api.saveStudy.mockImplementation(async (_graph, item) => item);
  api.fetchStudyLinkTitle.mockResolvedValue("A helpful lesson");
});
function button(text: string) {
  return [...document.querySelectorAll<HTMLButtonElement>("button")].find(element => element.textContent?.trim() === text
    || element.querySelector(".result-text strong")?.textContent === text)!;
}
function input(label: string, value: string) {
  if (label === "Paste a web link") label = "Search or paste a link";
  const field = [...document.querySelectorAll("label")].find(element => element.textContent?.startsWith(label) && element.querySelector("input"))!.querySelector("input")!;
  field.value = value; field.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
}
function selectTopic(value: string) {
  const select = document.querySelector<HTMLSelectElement>(".topic-picker select")!;
  select.value = value; select.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
}
function newTopic(value: string) {
  selectTopic("new"); input("New topic", value);
}
async function openAdd() {
  component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
  await vi.waitFor(() => expect(document.querySelector('[role="status"]')).toBeNull());
  button("+ Add study").click(); flushSync();
}
function titleValue() {
  return document.querySelector<HTMLInputElement>("details input")?.value ?? "";
}

describe("Studies library", () => {
  it("prefills a Library reference without importing media or duplicating its progress", async () => {
    const book = libraryBook({ progress: { position: 60, total: 100, anchor: "", label: "60%" } });
    const onLibraryConsumed = vi.fn();
    privateLibrary.set({ libraryPath: "/private/library", books: [book] });
    component = mount(Studies, { target: document.body, props: {
      graphPath: "/graph", onOpen: vi.fn(), addLibrary: book, onLibraryConsumed,
    } });
    await vi.waitFor(() => expect(button("Add study")?.disabled).toBe(false));
    expect(document.querySelector(".selected-source .badge")?.textContent).toBe("Library");
    expect(api.saveStudy).not.toHaveBeenCalled();
    expect(onLibraryConsumed).toHaveBeenCalledOnce();
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      source: libraryId, kind: "library", title: book.title,
      progress: { position: 0, total: 0, anchor: "", label: "" },
    })));
    expect(api.addPrivateLibraryLink).not.toHaveBeenCalled();
    expect(document.querySelector(".private-library")).toBeNull();
  });

  it("reads canonical Library progress and removes only the plan entry", async () => {
    const book = libraryBook({ progress: { position: 70, total: 100, anchor: "", label: "70% in Library" } });
    privateLibrary.set({ libraryPath: "/private/library", books: [book] });
    const item = fixture({ kind: "library", source: libraryId, title: book.title });
    api.listStudies.mockResolvedValue({ items: [item], days: [] });
    const onOpen = vi.fn();
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen } });
    await vi.waitFor(() => expect(document.querySelector("progress")?.value).toBe(70));
    button("Continue").click();
    expect(onOpen).toHaveBeenCalledWith(item);
    document.querySelector<HTMLButtonElement>('[aria-label="Remove Private lesson from Studies"]')!.click();
    flushSync();
    button("Remove study entry").click();
    await vi.waitFor(() => expect(api.removeStudy).toHaveBeenCalledWith("/graph", item.id));
    let remaining: ReaderBook[] = [];
    const unsubscribe = privateLibrary.subscribe(value => { remaining = value.books; });
    unsubscribe();
    expect(remaining).toEqual([book]);
  });

  it("keeps the plan unsaved when adding the Library link fails", async () => {
    api.addPrivateLibraryLink.mockRejectedValue(new Error("Private storage is full"));
    await openAdd();
    input("Paste a web link", "https://example.com/lesson.mp3");
    button("Add study").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Private storage is full"));
    expect(api.saveStudy).not.toHaveBeenCalled();
  });

  it("does not write a plan into a different graph after delayed Library registration", async () => {
    let finish!: (book: ReaderBook) => void;
    api.addPrivateLibraryLink.mockReturnValueOnce(new Promise<ReaderBook>(resolve => { finish = resolve; }));
    const state = new SvelteMap([["graph", "/old"]]);
    component = mount(Studies, { target: document.body, props: {
      get graphPath() { return state.get("graph")!; }, onOpen: vi.fn(),
    } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    input("Paste a web link", "https://example.com/lesson.mp3");
    button("Add study").click();
    await vi.waitFor(() => expect(api.addPrivateLibraryLink).toHaveBeenCalledOnce());
    state.set("graph", "/new"); flushSync();
    finish(libraryBook());
    await Promise.resolve(); flushSync();
    expect(api.saveStudy).not.toHaveBeenCalled();
  });

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
    newTopic("French"); button("Save topic").click();
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
    await vi.waitFor(() => expect(document.querySelector(".selected-source .badge")?.textContent).toBe("Book"));
    await vi.waitFor(() => expect(document.body.textContent).toContain("already in Studies"));
    expect(button("Add study").disabled).toBe(true);
    expect(api.saveStudy).not.toHaveBeenCalled();
  });
  it("chooses a page using its persisted ID and does not silently ignore a save failure", async () => {
    api.saveStudy.mockRejectedValue(new Error("Disk full"));
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    await vi.waitFor(() => expect(button("Physics")).toBeTruthy());
    button("Physics").click();
    await vi.waitFor(() => expect(titleValue()).toBe("Physics"));
    newTopic("Natural sciences");
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
    newTopic("French"); button("Save topic").click();
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
  it("selects existing graph assets without choosing their type or typing a title", async () => {
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    await vi.waitFor(() => expect(button("lesson")).toBeTruthy());
    input("Search or paste a link", "lesson");
    button("lesson").click(); flushSync();
    expect(titleValue()).toBe("lesson");
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      source: "assets/lesson.mp3", kind: "audio", topic: "General",
    })));
  });
  it("pastes YouTube links without first choosing a source type and saves the fetched title", async () => {
    await openAdd();
    input("Paste a web link", "https://youtu.be/dQw4w9WgXcQ?t=10");
    expect(document.querySelector(".selected-source .badge")?.textContent).toBe("YouTube");
    await vi.waitFor(() => expect(titleValue()).toBe("A helpful lesson"));
    expect(api.fetchStudyLinkTitle).toHaveBeenCalledWith("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      kind: "library", source: libraryId, title: "A helpful lesson",
    })));
    expect(api.addPrivateLibraryLink).toHaveBeenCalledWith("A helpful lesson", "youtube", "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  });
  it("uses the latest URL and ignores an out-of-order title response", async () => {
    let finishOld!: (value: string) => void;
    api.fetchStudyLinkTitle.mockReturnValueOnce(new Promise<string>(resolve => { finishOld = resolve; }));
    await openAdd();
    input("Paste a web link", "https://old.example/lesson");
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(1));
    input("Paste a web link", "https://new.example/lesson");
    await vi.waitFor(() => expect(titleValue()).toBe("A helpful lesson"));
    finishOld("Old lesson"); await Promise.resolve(); flushSync();
    expect(titleValue()).toBe("A helpful lesson");
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      kind: "website", source: "https://new.example/lesson", title: "A helpful lesson",
    })));
  });
  it("never replaces a user-edited title, including edits made during a lookup", async () => {
    let finish!: (value: string) => void;
    api.fetchStudyLinkTitle.mockReturnValueOnce(new Promise<string>(resolve => { finish = resolve; }));
    await openAdd();
    input("Paste a web link", "https://example.com/lesson");
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(1));
    input("Title", "My personal title");
    finish("Fetched title"); await Promise.resolve(); flushSync();
    expect(titleValue()).toBe("My personal title");
    input("Paste a web link", "https://example.com/another");
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(2));
    expect(titleValue()).toBe("My personal title");
  });
  it("shows lookup failures, supports retry, and still permits manual titles", async () => {
    api.fetchStudyLinkTitle.mockRejectedValue(new Error("Site unavailable"));
    await openAdd();
    input("Paste a web link", "https://example.com/lesson");
    await vi.waitFor(() => expect(document.body.textContent).toContain("Site unavailable"));
    button("Retry title lookup").click();
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(2));
    input("Title", "Manual title"); button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({ title: "Manual title" })));
  });
  it("cancels a debounced lookup when the form closes and ignores late results after reopening", async () => {
    await openAdd();
    input("Paste a web link", "https://example.com/not-requested");
    button("Cancel").click(); flushSync();
    await new Promise(resolve => setTimeout(resolve, 500));
    expect(api.fetchStudyLinkTitle).not.toHaveBeenCalled();
    button("+ Add study").click(); flushSync();
    let finish!: (value: string) => void;
    api.fetchStudyLinkTitle.mockReturnValueOnce(new Promise<string>(resolve => { finish = resolve; }));
    input("Paste a web link", "https://example.com/lesson");
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(1));
    button("Cancel").click(); flushSync(); button("+ Add study").click(); flushSync();
    finish("Old form title"); await Promise.resolve(); flushSync();
    expect(titleValue()).toBe("");
  });
  it("ignores metadata when changing graphs and clears prior topic choices", async () => {
    let finish!: (value: string) => void;
    api.fetchStudyLinkTitle.mockReturnValueOnce(new Promise<string>(resolve => { finish = resolve; }));
    api.listStudies.mockResolvedValueOnce({ items: [], days: [], topics: ["Old graph topic"] });
    const state = new SvelteMap([["graph", "/old"]]);
    component = mount(Studies, { target: document.body, props: {
      get graphPath() { return state.get("graph")!; }, onOpen: vi.fn(),
    } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    input("Paste a web link", "https://example.com/lesson");
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(1));
    state.set("graph", "/new"); flushSync();
    await vi.waitFor(() => expect(document.body.textContent).toContain("A little learning"));
    button("+ Add study").click(); flushSync();
    finish("Other graph title"); await Promise.resolve(); flushSync();
    expect(titleValue()).toBe("");
    expect(document.querySelector(".topic-picker")!.textContent).not.toContain("Old graph topic");
  });
  it("infers direct media titles from filenames without fetching their contents", async () => {
    await openAdd();
    input("Paste a web link", "https://example.com/Chinese-lesson.mp3");
    expect(titleValue()).toBe("Chinese lesson");
    expect(document.querySelector(".selected-source .badge")?.textContent).toBe("Audio");
    await vi.waitFor(() => expect(button("Add study").disabled).toBe(false));
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      kind: "library", source: libraryId, title: "Chinese lesson",
    })));
    expect(api.addPrivateLibraryLink).toHaveBeenCalledWith("Chinese lesson", "audio", "https://example.com/Chinese-lesson.mp3");
    expect(api.fetchStudyLinkTitle).not.toHaveBeenCalled();
  });
  it("keeps manually selected media types for extensionless URLs and cancels old title lookups", async () => {
    let finish!: (value: string) => void;
    api.fetchStudyLinkTitle.mockReturnValueOnce(new Promise<string>(resolve => { finish = resolve; }));
    await openAdd();
    input("Paste a web link", "https://example.com/old");
    await vi.waitFor(() => expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(1));
    input("Paste a web link", "https://example.com/stream?id=42");
    const kind = document.querySelector<HTMLSelectElement>("details select")!;
    kind.value = "audio"; kind.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    input("Title", "Streaming lesson");
    finish("Wrong title"); await Promise.resolve(); flushSync();
    expect(titleValue()).toBe("Streaming lesson");
    expect(kind.value).toBe("audio");
    await vi.waitFor(() => expect(button("Add study").disabled).toBe(false));
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      kind: "library", source: libraryId, title: "Streaming lesson",
    })));
    expect(api.addPrivateLibraryLink).toHaveBeenCalledWith("Streaming lesson", "audio", "https://example.com/stream?id=42");
  });
  it("debounces rapid URL changes and allows an inline topic to reuse a previous choice", async () => {
    api.listStudies.mockResolvedValue({ items: [fixture()], days: [], topics: ["Health"] });
    await openAdd();
    input("Paste a web link", "https://example.com/first");
    input("Paste a web link", "https://example.com/second");
    await vi.waitFor(() => expect(titleValue()).toBe("A helpful lesson"));
    expect(api.fetchStudyLinkTitle).toHaveBeenCalledTimes(1);
    expect(api.fetchStudyLinkTitle).toHaveBeenCalledWith("https://example.com/second");
    button("Cancel").click(); flushSync();
    document.querySelector<HTMLButtonElement>(".topic")!.click(); flushSync();
    selectTopic("topic:Health");
    button("Save topic").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({
      id: "study", topic: "Health",
    })));
  });
  it("reuses past topics and offers arbitrary new topics without sentinel collisions", async () => {
    api.listStudies.mockResolvedValue({ items: [], days: [], topics: ["Health", "new", "topic:Language"] });
    await openAdd();
    const select = document.querySelector<HTMLSelectElement>(".topic-picker select")!;
    expect([...select.options].map(option => option.textContent)).toEqual(expect.arrayContaining(["General", "Health", "new", "topic:Language", "+ Add a new topic..."]));
    selectTopic("topic:Health");
    expect(document.querySelector(".topic-picker input")).toBeNull();
    newTopic("New arbitrary topic");
    expect(document.activeElement?.getAttribute("placeholder")).toBe("Name your topic");
    selectTopic("topic:new");
    input("Paste a web link", "https://example.com/lesson");
    await vi.waitFor(() => expect(titleValue()).toBe("A helpful lesson"));
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({ topic: "new" })));
  });
  it("rejects a whitespace-only new topic instead of silently saving it as General", async () => {
    await openAdd();
    await vi.waitFor(() => expect(button("Physics")).toBeTruthy());
    button("Physics").click();
    await vi.waitFor(() => expect(titleValue()).toBe("Physics"));
    newTopic("   "); button("Add study").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Give the topic a name"));
    expect(api.saveStudy).not.toHaveBeenCalled();
  });
  it.each([
    ["#physics", "flashcards", "physics", "#physics"],
    ["Untagged cards", "flashcards", "", "Untagged cards"],
    ["lesson", "audio", "assets/lesson.mp3", "lesson"],
    ["movie", "video", "assets/movie.mp4", "movie"],
    ["Original book", "book", "book", "Original book"],
  ])("adds %s with automatically selected type/title and one confirmation", async (query, kind, source, title) => {
    api.listPageSummaries.mockResolvedValue([{ id: "book", title: "Original book", is_journal: false, is_book: true }]);
    api.getPage.mockResolvedValue({ id: "book", title: "Original book", properties: { "book-id": "original" } });
    api.listFlashcardTopics.mockResolvedValue([{ topic: "physics", total: 10, due: 2 }, { topic: "", total: 3, due: 1 }]);
    await openAdd();
    await vi.waitFor(() => expect(button("Original book")).toBeTruthy());
    expect(document.querySelectorAll('[role="option"]')).toHaveLength(5);
    expect(document.querySelector("details")).toBeNull();
    input("Search or paste a link", query);
    const field = document.querySelector<HTMLInputElement>('[role="combobox"]')!;
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })); flushSync();
    expect(field.getAttribute("aria-activedescendant")).toBeTruthy();
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(titleValue()).toBe(title));
    expect(api.saveStudy).not.toHaveBeenCalled();
    expect(document.querySelector("details")?.open).toBe(false);
    button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({ kind, source, title, topic: "General" })));
  });
  it("clears a previous selection when searching again and ignores its late page response", async () => {
    let finish!: (value: unknown) => void;
    api.getPage.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    await openAdd();
    await vi.waitFor(() => expect(button("Physics")).toBeTruthy());
    button("Physics").click(); flushSync();
    input("Search or paste a link", "lesson");
    button("lesson").click(); flushSync();
    finish({ id: "old", title: "Old page", properties: {} });
    await Promise.resolve(); flushSync();
    expect(titleValue()).toBe("lesson");
    input("Search or paste a link", "unmatched source");
    expect(button("Add study").disabled).toBe(true);
    expect(document.body.textContent).toContain("No matching sources");
    expect(api.fetchStudyLinkTitle).not.toHaveBeenCalled();
  });
  it("reports partial catalog failures while keeping available sources and links usable", async () => {
    api.listAssets.mockRejectedValueOnce(new Error("Asset directory unavailable"));
    await openAdd();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Could not load media assets"));
    expect(button("Physics")).toBeTruthy();
    expect(button("#physics")).toBeTruthy();
    button("Retry sources").click();
    await vi.waitFor(() => expect(button("lesson")).toBeTruthy());
    expect(document.body.textContent).not.toContain("Asset directory unavailable");
  });
  it("shows pages before slow asset discovery finishes and discards late results after closing", async () => {
    let finish!: (value: string[]) => void;
    api.listAssets.mockReturnValueOnce(new Promise<string[]>(resolve => { finish = resolve; }));
    await openAdd();
    await vi.waitFor(() => expect(button("Physics")).toBeTruthy());
    expect(document.body.textContent).toContain("Loading sources...");
    button("Cancel").click(); flushSync();
    button("+ Add study").click(); flushSync();
    await vi.waitFor(() => expect(button("lesson")).toBeTruthy());
    finish(["assets/Old-graph-recording.mp3"]); await Promise.resolve(); flushSync();
    expect(button("Old graph recording")).toBeUndefined();
    expect(button("Physics")).toBeTruthy();
  });
  it("marks existing sources without adding duplicates and defaults to the active topic filter", async () => {
    api.listStudies.mockResolvedValue({ items: [fixture({ source: "assets/lesson.mp3", topic: "Health" })], days: [] });
    component = mount(Studies, { target: document.body, props: { graphPath: "/graph", onOpen: vi.fn() } });
    await vi.waitFor(() => expect(button("French lesson")).toBeTruthy());
    const filter = document.querySelector<HTMLSelectElement>(".library-heading select")!;
    filter.value = "topic:Health"; filter.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    button("+ Add study").click(); flushSync();
    await vi.waitFor(() => expect(button("lesson")).toBeTruthy());
    expect(button("lesson").disabled).toBe(true);
    expect(button("lesson").textContent).toContain("Already added");
    expect(document.querySelector<HTMLSelectElement>(".topic-picker select")!.value).toBe("topic:Health");
    button("movie").click(); flushSync(); button("Add study").click();
    await vi.waitFor(() => expect(api.saveStudy).toHaveBeenCalledWith("/graph", expect.objectContaining({ kind: "video", topic: "Health" })));
  });
});
