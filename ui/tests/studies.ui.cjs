const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      blockCount: 100,
      beforeNavigate: page => page.addInitScript(() => {
        if (window !== window.top) return;
        function install(internals) {
          const progress = { position: 0, total: 0, anchor: "", label: "" };
          const entry = (id, title, kind, source, topic) => ({
            id, title, kind, source, topic, progress: { ...progress }, createdAt: "", updatedAt: "",
          });
          const fixture = window.__studiesFixture = {
            items: [
              entry("reading", "Reading practice", "page", "selection-page", "Health"),
              entry("cards", "Chinese cards", "flashcards", "chinese", "Chinese"),
            ], days: [], topics: ["Health", "Chinese", "Past topic"], writes: [], receipts: new Set(), library: [],
          };
          const original = internals.invoke;
          internals.invoke = async (command, args = {}) => {
            if (["list_studies", "save_study", "remove_study", "record_study_activity"].includes(command)) {
              assertGraph(args.graphPath);
            }
            if (command === "list_studies") return structuredClone({ items: fixture.items, days: fixture.days, topics: fixture.topics });
            if (command === "study_link_title") return args.url.includes("youtube.com") ? "Video lesson" : "Reading website";
            if (command === "save_study") {
              const item = structuredClone(args.item);
              fixture.items = fixture.items.filter(old => old.id !== item.id).concat(item);
              fixture.topics = [...new Set([...fixture.topics, item.topic])];
              return item;
            }
            if (command === "remove_study") {
              fixture.items = fixture.items.filter(item => item.id !== args.id);
              return;
            }
            if (command === "record_study_activity") {
              if (fixture.receipts.has(args.requestId)) return;
              fixture.receipts.add(args.requestId);
              fixture.writes.push(structuredClone(args));
              const item = fixture.items.find(item => item.id === args.id);
              if (!item) throw new Error("Study was removed");
              if (args.progress) item.progress = structuredClone(args.progress);
              if (args.seconds) fixture.days.push({ itemId: item.id, topic: item.topic, day: args.day, seconds: args.seconds });
              return;
            }
            // Media and YouTube studies reference stable Library items (app-private).
            if (command === "reader_snapshot" || command === "reader_rescan") {
              return structuredClone({ libraryPath: "/synthetic/library", books: fixture.library });
            }
            if (command === "reader_add_link") {
              if ("graphPath" in args) throw new Error("Library links must not use the active graph");
              fixture.library.push({
                id: crypto.randomUUID(), title: args.title, kind: args.kind, sourceUrl: args.url,
                available: true, tracks: [], position: null, bookmarks: [], favorite: false, lastUsedAt: 0,
              });
              return structuredClone({ libraryPath: "/synthetic/library", books: fixture.library });
            }
            if (command === "list_page_summaries") return window.__selectionState.pages;
            if (command === "list_flashcard_topics") return [{ topic: "chinese", total: 1, due: 1 }, { topic: "physics", total: 2, due: 1 }];
            if (command === "list_assets") return ["assets/Chinese-listening.mp3", "assets/Lecture.mp4", "assets/image.png"];
            if (command === "list_flashcards_due") return [{
              id: "card", front: "Synthetic question", back: "Synthetic answer", page_file_path: "pages/Cards.md",
            }];
            if (command === "grade_flashcard") return;
            if (command === "help_get_page" && args.context === "studies") return "# Studies\n\n90 seconds of inactivity.";
            return original(command, args);
          };
          function assertGraph(path) {
            if (path !== "/synthetic/keyboard-selection") throw new Error(`Wrong graph: ${path}`);
          }
          return internals;
        }
        let internals = window.__TAURI_INTERNALS__;
        if (internals) internals = install(internals);
        Object.defineProperty(window, "__TAURI_INTERNALS__", {
          configurable: true, get: () => internals, set: value => { internals = install(value); },
        });
      }),
    });
    page.setDefaultTimeout(15000);
    const studiesNav = page.locator(".sidebar").getByRole("button", { name: "Studies", exact: true });
    await studiesNav.click();
    await page.getByRole("heading", { name: "Studies", exact: true }).waitFor();
    await page.getByRole("button", { name: "Reading practice", exact: true }).click();
    await page.getByRole("button", { name: "Back to Studies", exact: true }).waitFor();
    await page.locator("#block-large-0").waitFor();
    await page.waitForTimeout(2700);
    await page.locator("main.main-content").evaluate(main => { main.scrollTop = 900; });
    await page.waitForTimeout(1200);
    const readingTop = await page.locator("main.main-content").evaluate(main => main.scrollTop);
    assert.ok(readingTop > 500, "source scrolls to an actual reading position");
    await page.getByRole("button", { name: "Back to Studies", exact: true }).click();
    await page.getByRole("heading", { name: "Studies", exact: true }).waitFor();
    const saved = await page.evaluate(() => window.__studiesFixture.items.find(item => item.id === "reading").progress);
    assert.ok(saved.position > 500);
    assert.ok(saved.anchor.includes("large-"));
    assert.ok(await page.evaluate(() => window.__studiesFixture.days.some(day => day.itemId === "reading" && day.seconds > 0)),
      "focused reading records actual time, not just progress");
    await page.getByRole("button", { name: "Reading practice", exact: true }).click();
    await page.waitForFunction(expected => Math.abs(document.querySelector("main.main-content").scrollTop - expected) < 40, readingTop);
    await page.getByRole("button", { name: "Pause clock", exact: true }).click();
    assert.match(await page.locator(".study-session-bar").innerText(), /paused/);
    await page.getByRole("button", { name: "Back to Studies", exact: true }).click();
    assert.ok(await page.evaluate(() => window.__studiesFixture.items.find(item => item.id === "reading").progress.position > 500),
      "using session controls does not scroll back to the start and overwrite the checkpoint");
    await page.getByRole("button", { name: "Chinese cards", exact: true }).click();
    await page.getByRole("button", { name: /Show answer/ }).click();
    await page.getByRole("button", { name: /Good recalled/ }).click();
    await page.getByText("Session complete").waitFor();
    await page.getByRole("button", { name: "Back to Studies", exact: true }).click();
    await page.getByRole("button", { name: "+ Add study", exact: true }).click();
    const form = page.locator("form.add-form");
    const search = form.getByRole("combobox", { name: "Search or paste a link" });
    await search.fill("https://example.com/lesson");
    await form.locator(".selected-source").getByText("Reading website", { exact: true }).waitFor();
    assert.equal(await form.getByLabel("Source type").inputValue(), "website");
    await form.getByLabel("Topic", { exact: true }).selectOption("topic:Health");
    await form.getByRole("button", { name: "Add study", exact: true }).click();
    await page.getByRole("button", { name: "Reading website", exact: true }).click();
    await page.getByLabel("Where did you leave off?").fill("Section 4");
    await page.getByLabel("Progress (%)").fill("35");
    await page.getByRole("button", { name: "Save checkpoint", exact: true }).click();
    await page.waitForFunction(() => window.__studiesFixture.items.some(item => item.progress.label === "Section 4"));
    await page.getByRole("button", { name: "Back to Studies", exact: true }).click();
    await page.locator(".library-heading select").selectOption("topic:Health");
    assert.equal(await page.getByRole("button", { name: "Chinese cards", exact: true }).count(), 0);
    await page.getByText("Section 4", { exact: true }).waitFor();
    if (process.env.STUDIES_SCREENSHOT) await page.screenshot({ path: process.env.STUDIES_SCREENSHOT });
    await page.getByRole("button", { name: "+ Add study", exact: true }).click();
    await search.fill("https://youtu.be/dQw4w9WgXcQ");
    await form.locator(".selected-source").getByText("Video lesson", { exact: true }).waitFor();
    assert.equal(await form.getByLabel("Source type").inputValue(), "youtube");
    await form.getByLabel("Topic", { exact: true }).selectOption("topic:Past topic");
    await form.getByLabel("Topic", { exact: true }).selectOption("new");
    await form.getByLabel("New topic", { exact: true }).fill("Learning");
    if (process.env.STUDIES_FORM_SCREENSHOT) await page.screenshot({ path: process.env.STUDIES_FORM_SCREENSHOT });
    await form.getByRole("button", { name: "Add study", exact: true }).click();
    await page.locator(".library-heading select").selectOption("topic:Learning");
    await page.getByRole("button", { name: "Video lesson", exact: true }).waitFor();
    assert.ok(await page.evaluate(() => window.__studiesFixture.items.some(item => item.kind === "library"
      && item.topic === "Learning" && window.__studiesFixture.library.some(book => book.id === item.source && book.kind === "youtube"))),
      "a YouTube study references its stable Library item");
    await page.getByRole("button", { name: "+ Add study", exact: true }).click();
    await form.getByRole("option", { name: "#physics — Flashcards", exact: true }).waitFor();
    await form.getByRole("option", { name: "Chinese listening — Audio", exact: true }).waitFor();
    await form.getByRole("option", { name: "Lecture — Video", exact: true }).waitFor();
    assert.equal(await form.getByLabel("Title", { exact: true }).count(), 0);
    assert.equal(await form.getByLabel("Source type", { exact: true }).count(), 0);
    if (process.env.STUDIES_PICKER_SCREENSHOT) await page.screenshot({ path: process.env.STUDIES_PICKER_SCREENSHOT });
    await search.fill("physics");
    await search.press("ArrowDown");
    await search.press("Enter");
    await form.locator(".selected-source").getByText("#physics", { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.__studiesFixture.items.some(item => item.source === "physics")), false);
    assert.equal(await form.locator("details").getAttribute("open"), null);
    await form.getByRole("button", { name: "Add study", exact: true }).click();
    await page.getByRole("button", { name: "#physics", exact: true }).waitFor();
    assert.ok(await page.evaluate(() => window.__studiesFixture.items.some(item => item.kind === "flashcards" && item.source === "physics" && item.topic === "Learning")));
    await page.keyboard.press("F1");
    await page.getByText("90 seconds of inactivity.").waitFor();
    assert.deepEqual(errors, []);
    assert.equal(await page.evaluate(() => window.__selectionState.calls.some(call => call.cmd === "create_page")), false);
    console.log("PASS Studies: unified source search, keyboard selection and Add confirmation, URL prefill, topic reuse, resume, timing, and F1");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
