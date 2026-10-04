// A Library location on an unplugged drive: its items stay listed and
// searchable, clicking one says to plug the drive in, and reconnecting opens
// it. Locations are added and removed in Settings. Synthetic IPC only; no
// personal graph, Library or drive is touched.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

const DRIVE = "/run/media/fixture/6TWDBACKUP/books";
const CARD = "/run/media/fixture/SDCARD";

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      beforeNavigate: (page) => page.addInitScript(({ DRIVE, CARD }) => {
        if (window !== window.top) return;
        const fixture = window.__libraryFixture = {
          driveConnected: false, picked: [], calls: [],
          locations: [DRIVE, CARD],
        };
        const book = (id, title, kind, location) => ({
          id, title, kind, location, available: true, tracks: kind === "audio"
            ? [{ id: `${id}-t1`, title: "Chapter 1", relativePath: `${title}/1.mp3`, available: true }] : [],
          position: kind === "audio" ? { trackId: `${id}-t1`, offsetMs: 61000 } : null,
          bookmarks: [], favorite: false, lastUsedAt: 0,
        });
        const books = [
          book("dune", "Dune", "epub", DRIVE),
          book("novel", "Fixture novel", "audio", DRIVE),
          book("talk", "Card talk", "audio", CARD),
        ];
        const snapshot = () => {
          const connected = path => path !== DRIVE || fixture.driveConnected;
          return structuredClone({
            libraryPath: fixture.locations[0] ?? null,
            locations: fixture.locations.map(path => ({
              path, connected: connected(path), items: books.filter(b => b.location === path).length,
              ...(connected(path) ? {} : { reason: "Not connected" }),
            })),
            books: books.filter(b => fixture.locations.includes(b.location)).map(b => ({
              ...b, available: connected(b.location), ...(connected(b.location) ? {} : { disconnected: true }),
            })),
          });
        };
        function install(internals) {
          const original = internals.invoke;
          internals.invoke = async (command, args = {}) => {
            if (command.startsWith("reader_")) fixture.calls.push({ command, args: structuredClone(args) });
            switch (command) {
              case "reader_snapshot": case "reader_rescan": return snapshot();
              case "reader_add_location": fixture.locations.push(args.path); return snapshot();
              case "reader_remove_location":
                fixture.locations = fixture.locations.filter(path => path !== args.path); return snapshot();
              case "reader_read_epub": throw new Error("The fixture never serves book bytes");
              case "library_index_status": return {
                enabled: true, transcribeMedia: true, running: false, jobId: null,
                items: { total: 3, indexed: 2, pending: 0, failed: 0, titleOnly: 0, waiting: fixture.driveConnected ? 0 : 1 },
                chunks: 5, semantic: "ready", semanticReason: null, transcription: "ready", transcriptionReason: null,
                lastIndexedAt: null, errors: [],
              };
              case "library_search": return [{ bookId: "dune", title: "Dune", chunkId: "c1", kind: "epub",
                snippet: "the spice must flow", trackId: null, startMs: null, endMs: null, chapter: "Book One",
                quote: null, score: 1, match: "keyword" }];
              case "plugin:dialog|open": return fixture.picked.shift() ?? null;
              case "list_studies": return { items: [], days: [], topics: [] };
              default: return original(command, args);
            }
          };
          return internals;
        }
        let internals = window.__TAURI_INTERNALS__;
        if (internals) internals = install(internals);
        Object.defineProperty(window, "__TAURI_INTERNALS__", {
          configurable: true, get: () => internals, set: (value) => { internals = install(value); },
        });
      }, { DRIVE, CARD }),
    });
    const library = page.locator("section.private-library");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "__library__" })));
    await library.waitFor();

    const notice = library.locator(".offline-notice");
    await notice.filter({ hasText: "books on 6TWDBACKUP isn't connected. Its 2 items are kept here until you reconnect it." }).waitFor();
    await library.getByText("1 waiting for a disconnected location").waitFor();
    const row = (title) => library.locator("li").filter({ has: page.getByRole("button", { name: title, exact: true }) });
    await row("Dune").getByText("Disconnected · 6TWDBACKUP").waitFor();
    assert.equal(await row("Card talk").getByText("Disconnected").count(), 0, "the connected card is unaffected");

    // Inside search still finds the disconnected book's indexed text.
    await library.getByRole("searchbox").fill("spice");
    const hit = library.locator(".hit").filter({ hasText: "the spice must flow" });
    await hit.filter({ hasText: "Disconnected" }).waitFor();
    await library.getByRole("searchbox").fill("");

    // Clicking a disconnected item checks again, then asks for the drive.
    const rescans = () => page.evaluate(() => window.__libraryFixture.calls.filter(c => c.command === "reader_rescan").length);
    const before = await rescans();
    await row("Dune").getByRole("button", { name: "Read", exact: true }).click();
    await page.locator(".toast").filter({ hasText: "“Dune” is on 6TWDBACKUP, which isn't connected. Plug in the drive or SD card" }).waitFor();
    assert.ok(await rescans() > before, "the click rechecked the location first");
    assert.equal(await page.locator("section.private-detail").count(), 0, "nothing was opened");

    // Its detail page keeps history and offers Check again; plugging in clears it.
    await row("Fixture novel").getByRole("button", { name: "Fixture novel", exact: true }).click();
    const detail = page.locator("section.private-detail");
    await detail.getByText("“Fixture novel” is on 6TWDBACKUP, which isn't connected.").waitFor();
    assert.equal(await detail.getByRole("button", { name: "Resume audio" }).isDisabled(), true);
    await page.evaluate(() => { window.__libraryFixture.driveConnected = true; });
    await detail.getByRole("button", { name: "Check again" }).click();
    await detail.getByText("isn't connected").waitFor({ state: "detached" });
    assert.equal(await detail.getByRole("button", { name: "Resume audio" }).isDisabled(), false);
    await detail.getByRole("button", { name: "← Library" }).click();
    await library.waitFor();
    await notice.waitFor({ state: "detached" });

    // Settings lists locations; adding and removing one go through native commands.
    await library.getByRole("button", { name: "Library settings" }).click();
    const settings = page.locator(".private-settings");
    await settings.getByRole("heading", { name: "Library locations" }).waitFor();
    const locations = settings.getByRole("list", { name: "Library locations" }).locator("li");
    await locations.filter({ hasText: "books on 6TWDBACKUP" }).filter({ hasText: "Connected" }).waitFor();
    await page.evaluate(() => { window.__libraryFixture.picked.push("/mnt/nas/audio"); });
    await settings.getByRole("button", { name: "Add location…" }).click();
    await locations.filter({ hasText: "audio on nas" }).waitFor();
    const card = locations.filter({ hasText: "SDCARD" });
    await card.getByRole("button", { name: "Remove…" }).click();
    await card.getByText("Remove “SDCARD” from Library? Its 1 item leaves Library with its reading progress, bookmarks, and search index. The files in the folder are not touched.").waitFor();
    assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Cancel");
    await card.getByRole("button", { name: "Remove location" }).click();
    await card.waitFor({ state: "detached" });
    const calls = await page.evaluate(() => window.__libraryFixture.calls
      .filter(c => ["reader_add_location", "reader_remove_location"].includes(c.command)));
    assert.deepEqual(calls, [
      { command: "reader_add_location", args: { path: "/mnt/nas/audio" } },
      { command: "reader_remove_location", args: { path: CARD } },
    ]);
    assert.deepEqual(errors, []);
    console.log("PASS Library locations: disconnected items kept, plug-in prompt, reconnect, add and remove");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
