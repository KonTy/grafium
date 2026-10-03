const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdirSync, readFileSync, rmSync } = require("node:fs");
const { join } = require("node:path");
const { openEditor } = require("./keyboardSelection.ui.cjs");

function silentAudio() {
  const samples = 8000 * 90;
  const bytes = Buffer.alloc(44 + samples * 2);
  bytes.write("RIFF", 0); bytes.writeUInt32LE(bytes.length - 8, 4);
  bytes.write("WAVEfmt ", 8); bytes.writeUInt32LE(16, 16);
  bytes.writeUInt16LE(1, 20); bytes.writeUInt16LE(1, 22);
  bytes.writeUInt32LE(8000, 24); bytes.writeUInt32LE(16000, 28);
  bytes.writeUInt16LE(2, 32); bytes.writeUInt16LE(16, 34);
  bytes.write("data", 36); bytes.writeUInt32LE(samples * 2, 40);
  return bytes;
}

function syntheticVideo() {
  const directory = join(__dirname, ".playwright-artifacts");
  mkdirSync(directory, { recursive: true });
  const path = join(directory, `fixture-${process.pid}.webm`);
  try {
    execFileSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
      "color=c=blue:s=160x90:r=5:d=20", "-threads", "1", "-c:v", "libvpx", path]);
    return readFileSync(path);
  } finally {
    try { rmSync(path, { force: true }); } catch (error) { if (error.code !== "ENOENT") throw error; }
  }
}

(async () => {
  const videoBytes = syntheticVideo();
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      beforeNavigate: async page => {
        await page.route("http://127.0.0.1:5199/reader-fixture.wav", route => {
          const bytes = silentAudio();
          const range = route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
          const start = range ? Number(range[1]) : 0;
          const end = range?.[2] ? Math.min(Number(range[2]), bytes.length - 1) : bytes.length - 1;
          return route.fulfill({
            status: range ? 206 : 200, contentType: "audio/wav", body: bytes.subarray(start, end + 1),
            headers: {
              "Access-Control-Allow-Origin": "*", "Accept-Ranges": "bytes",
              ...(range ? { "Content-Range": `bytes ${start}-${end}/${bytes.length}` } : {}),
            },
          });
        });
        await page.route("http://127.0.0.1:5199/reader-fixture.webm", route => {
          const range = route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
          const start = range ? Number(range[1]) : 0;
          const end = range?.[2] ? Math.min(Number(range[2]), videoBytes.length - 1) : videoBytes.length - 1;
          return route.fulfill({
            status: range ? 206 : 200, contentType: "video/webm", body: videoBytes.subarray(start, end + 1),
            headers: { "Access-Control-Allow-Origin": "*", "Accept-Ranges": "bytes",
              ...(range ? { "Content-Range": `bytes ${start}-${end}/${videoBytes.length}` } : {}) },
          });
        });
        await page.addInitScript(() => {
          if (window !== window.top) return;
          const OriginalAudio = window.Audio;
          window.Audio = function (...args) {
            const audio = new OriginalAudio(...args);
            window.__privateAudio = audio;
            return audio;
          };
          function install(internals) {
            const fixture = window.__privateReaderFixture = {
              libraryPath: "/synthetic/private-library",
              books: [{
                id: "private-book", title: "Private listening fixture", kind: "audio", available: true,
                tracks: [{ id: "track-one", title: "Chapter 1", relativePath: "Fixture/1.mp3", available: true }],
                position: { trackId: "track-one", offsetMs: 12000 }, bookmarks: [],
              }, {
                id: "direct-audio", title: "Direct audio fixture", kind: "audio", available: true,
                sourceUrl: "http://127.0.0.1:5199/reader-fixture.wav", tracks: [], position: null, bookmarks: [],
              }, {
                id: "direct-video", title: "Direct video fixture", kind: "video", available: true,
                sourceUrl: "http://127.0.0.1:5199/reader-fixture.webm", tracks: [], position: null, bookmarks: [],
              }, {
                id: "local-video", title: "Local video fixture", kind: "video", available: true,
                tracks: [{ id: "video-track", title: "Video", relativePath: "Fixture/video.webm", available: true }],
                position: null, bookmarks: [],
              }],
              writes: [], graph: "/synthetic/keyboard-selection", failBookmark: false,
            };
            const original = internals.invoke;
            internals.invoke = async (command, args = {}) => {
              if (command === "library_index_status") return { enabled: true, transcribeMedia: true, running: false, jobId: null, items: { total: 0, indexed: 0, pending: 0, failed: 0, titleOnly: 0 }, chunks: 0, semantic: "ready", semanticReason: null, transcription: "ready", transcriptionReason: null, lastIndexedAt: null, errors: [] };
              if (command === "library_search") return [];
              if (command.startsWith("reader_")) {
                fixture.writes.push({ command, args: structuredClone(args) });
                if ("graphPath" in args) throw new Error("Private reading must not use the active graph.");
                switch (command) {
                  case "reader_snapshot": case "reader_rescan":
                    return structuredClone({ libraryPath: fixture.libraryPath, books: fixture.books });
                  case "reader_media_url": return `http://127.0.0.1:5199/reader-fixture.${args.bookId === "local-video" ? "webm" : "wav"}`;
                  case "reader_save_position":
                    fixture.books.find(book => book.id === args.bookId).position = structuredClone(args.position);
                    return;
                  case "reader_record_activity": {
                    const book = fixture.books.find(book => book.id === args.bookId);
                    book.lastUsedAt = Date.now();
                    book.progress = structuredClone(args.progress);
                    return structuredClone({ libraryPath: fixture.libraryPath, books: fixture.books });
                  }
                  case "reader_add_bookmark": {
                    if (fixture.failBookmark) throw new Error("Synthetic durable bookmark failure");
                    const bookmark = {
                      id: `bookmark-${fixture.books[0].bookmarks.length}`, bookId: args.bookId,
                      position: structuredClone(args.position), createdAt: Date.now(), note: args.note,
                    };
                    fixture.books[0].bookmarks.push(bookmark);
                    return structuredClone(bookmark);
                  }
                  default: throw new Error(`Unhandled private reader command: ${command}`);
                }
              }
              if (command === "list_studies") return { items: [], days: [], topics: [] };
              if (command === "get_graph_info") return { name: fixture.graph.endsWith("other") ? "Other fixture" : "Keyboard selection fixture", path: fixture.graph };
              if (command === "list_graphs") return [
                { name: "Keyboard selection fixture", path: "/synthetic/keyboard-selection" },
                { name: "Other fixture", path: "/synthetic/other" },
              ];
              if (command === "validate_graph") return { is_valid: true };
              if (command === "open_graph") { fixture.graph = args.path; return; }
              if (command === "help_get_page" && args.context === "reader") return "# Private reader\n\nPrivate reading help fixture.";
              return original(command, args);
            };
            return internals;
          }
          let internals = window.__TAURI_INTERNALS__;
          if (internals) internals = install(internals);
          Object.defineProperty(window, "__TAURI_INTERNALS__", {
            configurable: true, get: () => internals, set: value => { internals = install(value); },
          });
        });
      },
    });
    page.setDefaultTimeout(15000);
    const sidebar = page.locator(".sidebar");
    await sidebar.getByRole("button", { name: "Library", exact: true }).click();
    await page.getByRole("button", { name: "Private listening fixture", exact: true }).click();
    assert.equal(await page.evaluate(() => !!window.__privateAudio && !window.__privateAudio.paused), false, "opening Library must not autoplay");
    await page.getByRole("button", { name: "Resume audio", exact: true }).click();
    const toolbar = page.getByRole("region", { name: "Library playback" });
    await toolbar.getByRole("button", { name: "Pause", exact: true }).waitFor();
    await page.waitForFunction(() => window.__privateAudio && !window.__privateAudio.paused && window.__privateAudio.currentTime >= 12);
    const inPage = page.getByRole("region", { name: "Audiobook playback", exact: true });
    await inPage.getByRole("button", { name: "Stop", exact: true }).waitFor({ state: "visible" });
    await toolbar.getByRole("slider", { name: "Seek audio" }).waitFor({ state: "visible" });
    await page.waitForFunction(() => !document.querySelector('[aria-label="Library playback"] input[type="range"]').disabled);
    assert.equal(await toolbar.getByRole("slider").getAttribute("max"), "90000");
    await sidebar.getByRole("button", { name: "All Pages", exact: true }).click();
    await toolbar.getByRole("button", { name: "Pause", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), false);
    await toolbar.getByRole("button", { name: "Pause", exact: true }).click();
    await toolbar.getByRole("button", { name: "Resume", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), true);
    await toolbar.getByRole("slider").fill("31000");
    await page.waitForFunction(() => Math.abs(window.__privateAudio.currentTime - 31) < .1);
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), true, "seek preserves pause");
    await toolbar.getByRole("button", { name: "Bookmark", exact: true }).click();
    await toolbar.getByText("Bookmark saved on this device.").waitFor();
    const captured = await page.evaluate(() => window.__privateReaderFixture.books[0].bookmarks[0].position);
    assert.equal(captured.trackId, "track-one");
    assert.ok(Math.abs(captured.offsetMs - 31000) < 100, `bookmark samples the actual player, not the last checkpoint: ${JSON.stringify(captured)}`);
    await page.evaluate(() => { window.__privateReaderFixture.failBookmark = true; });
    await toolbar.getByRole("button", { name: "Bookmark", exact: true }).click();
    await toolbar.getByRole("alert").getByText(/Synthetic durable bookmark failure/).waitFor();
    assert.equal(await toolbar.getByText("Bookmark saved on this device.").count(), 0);
    await page.evaluate(() => { window.__privateReaderFixture.failBookmark = false; });
    await toolbar.getByRole("button", { name: "Resume", exact: true }).click();
    await page.locator(".graph-menu-container > button").click();
    await page.getByRole("menuitem", { name: /Other fixture/ }).click();
    await page.waitForFunction(() => window.__privateReaderFixture.graph === "/synthetic/other");
    await toolbar.getByRole("button", { name: "Pause", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), false);
    await page.setViewportSize({ width: 390, height: 844 });
    const stopBounds = await toolbar.getByRole("button", { name: "Stop", exact: true }).boundingBox();
    assert.ok(stopBounds && stopBounds.x >= 0 && stopBounds.x + stopBounds.width <= 390 && stopBounds.y + stopBounds.height <= 844, "Stop must remain onscreen on a phone-width viewport");
    await toolbar.getByRole("slider").focus();
    await page.keyboard.press("ArrowRight");
    await page.setViewportSize({ width: 1280, height: 900 });
    await toolbar.getByRole("button", { name: "Bookmark", exact: true }).focus();
    await page.keyboard.press("F1");
    await page.getByText("Private reading help fixture.").waitFor();
    await page.keyboard.press("Escape");
    await toolbar.getByRole("button", { name: "Stop", exact: true }).click();
    await toolbar.waitFor({ state: "hidden" });
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), true);
    await sidebar.getByRole("button", { name: "Library", exact: true }).click();
    await page.getByRole("button", { name: "Direct audio fixture", exact: true }).click();
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), true);
    await page.getByRole("button", { name: "Play audio", exact: true }).click();
    await toolbar.getByRole("button", { name: "Pause", exact: true }).waitFor();
    await page.waitForFunction(() => !window.__privateAudio.paused && window.__privateAudio.currentTime > 0);
    await sidebar.getByRole("button", { name: "All Pages", exact: true }).click();
    await toolbar.getByRole("button", { name: "Stop", exact: true }).click();
    await toolbar.waitFor({ state: "hidden" });
    assert.equal(await page.evaluate(() => window.__privateAudio.paused), true);
    for (const title of ["Direct video fixture", "Local video fixture"]) {
      await sidebar.getByRole("button", { name: "Library", exact: true }).click();
      await page.getByRole("button", { name: title, exact: true }).click();
      assert.equal(await page.locator("video").count(), 0, "opening video details must not autoplay");
      await page.getByRole("button", { name: "Play", exact: true }).click();
      const controls = page.getByRole("region", { name: "Media playback controls" });
      await page.waitForFunction(() => {
        const video = document.querySelector("video");
        return video && !video.paused && video.videoWidth === 160 && video.currentTime > 0;
      });
      await controls.getByRole("button", { name: "Pause", exact: true }).click();
      assert.equal(await page.locator("video").evaluate(video => video.paused), true);
      await controls.getByRole("slider", { name: "Seek video" }).fill("10");
      await page.waitForFunction(() => Math.abs(document.querySelector("video").currentTime - 10) < .1);
      assert.equal(await page.locator("video").evaluate(video => video.paused), true);
      await controls.getByRole("button", { name: "Resume", exact: true }).click();
      await page.waitForFunction(() => !document.querySelector("video").paused);
      await controls.getByRole("button", { name: "Stop", exact: true }).click();
      await controls.getByRole("status").getByText(/Stopped/).waitFor();
      assert.equal(await page.locator("video").evaluate(video => video.paused), true);
      await controls.getByRole("button", { name: "Resume", exact: true }).click();
      await page.waitForFunction(() => !document.querySelector("video").paused);
      await page.locator("video").evaluate(video => { window.__foregroundVideo = video; });
      await sidebar.getByRole("button", { name: "All Pages", exact: true }).click();
      assert.equal(await page.evaluate(() => window.__foregroundVideo.paused && !window.__foregroundVideo.getAttribute("src")), true);
    }
    const state = await page.evaluate(() => ({
      progress: window.__privateReaderFixture.books[0].position,
      graphWrites: window.__selectionState.calls.filter(call =>
        call.cmd === "create_page" || ["create_block", "insert_block"].includes(call.cmd) && call.args.content.trim()),
    }));
    assert.ok(state.progress.offsetMs >= 31000);
    assert.deepEqual(state.graphWrites, [], "reading and bookmarks must not create graph pages or note content");
    assert.deepEqual(errors, []);
    console.log("PASS private reader: real local/direct audio and video controls, seek/pause/stop, no autoplay, narrow viewport, navigation, private persistence and F1");
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
