// Full-App editor regressions. Every IPC read/write stays in synthetic browser state.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const runtimeDir = fs.mkdtempSync(path.join(__dirname, ".emoji-reader-runtime-"));
process.env.TMPDIR = runtimeDir;
const { chromium } = require("playwright");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function openFixture(browser, continuous = false, width = 1400, libraryBooks = null) {
  const page = await browser.newPage({ viewport: { width, height: 1100 } });
  page.setDefaultTimeout(8_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript((libraryBooks) => {
    if (window !== window.top) return;
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify({
      kind: "page", title: "Emoji reader regression",
    }));
    localStorage.setItem("grafium.startupTheme", JSON.stringify({
      background: "#ffffff", foreground: "#24292f", colorScheme: "light",
    }));
    const note = {
      id: "emoji-reader-page", title: "Emoji reader regression",
      properties: {}, is_journal: false, file_path: "pages/Emoji reader regression.md",
      created_at: 0, updated_at: 0,
    };
    const pages = [
      note,
      { ...note, id: "orbits-page", title: "Space/Orbits", file_path: "pages/Space/Orbits.md" },
      { ...note, id: "light-page", title: "Space/Light", file_path: "pages/Space/Light.md" },
    ];
    const makeBlock = (id, order, content) => ({
      id, page_id: note.id, parent_id: null, order_index: order, content,
      block_type: "text", properties: {}, created_at: 0, updated_at: 0,
    });
    const blocks = [
      makeBlock("draft", 0, "Practice here"),
      makeBlock("prose", 1, "Observatory notes connect distant ideas with careful observations."),
      makeBlock("rich", 2, "A bright :icon-star: and literal `:icon-star:` with `const orbit = 1`."),
      makeBlock("fenced", 3, "```text\n:icon-star:\n/emoji rocket\n```"),
      makeBlock("math", 4, "Orbital speed is $v = \\sqrt{GM/r}$ while motion continues."),
      makeBlock("task", 5, "TODO Compare telescope aperture [#A]"),
      makeBlock("table", 6, "| Target | Count |\n| --- | ---: |\n| Venus | 2 |\n| Moon | 10 |\n| Sun | 1 |"),
      makeBlock("linked", 7, "Continue with [[Space/Orbits]] and [[Space/Light]]."),
    ];
    const serialize = () => window.__emojiReader.blocks
      .filter((block) => block.page_id === note.id)
      .sort((a, b) => a.order_index - b.order_index)
      .map((block) => {
        const [first, ...rest] = block.content.split("\n");
        return [`- ${first}`, ...rest.map((line) => `  ${line}`), `  id:: ${block.id}`].join("\n");
      }).join("\n\n") + "\n";
    window.__emojiReader = { blocks, source: "", writes: [], calls: [] };
    window.__selectionState = { pages, blocks };
    window.__emojiReader.source = serialize();
    let sequence = 0;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        const state = window.__emojiReader;
        state.calls.push({ cmd, args: structuredClone(args) });
        switch (cmd) {
          case "reader_snapshot":
          case "reader_rescan": return { libraryPath: libraryBooks ? "/synthetic/library" : null, books: libraryBooks ?? [] };
          case "get_page": {
            const found = pages.find((item) => item.id === args.id || item.title === args.title);
            if (!found) throw new Error("Page not found");
            return structuredClone(found);
          }
          case "get_graph_info": return { name: "Synthetic Welcome graph", path: "/synthetic/emoji-reader" };
          case "get_app_theme": return "github";
          case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
          case "get_smplos_theme": return null;
          case "get_parent_page": return null;
          case "list_pages":
          case "list_page_summaries": return structuredClone(pages);
          case "search_page_titles":
            // Tests slow the title search to catch keys pressed while results load.
            if (window.__searchDelayMs) await new Promise((resolve) => setTimeout(resolve, window.__searchDelayMs));
            return structuredClone(pages.filter((item) =>
              item.title.toLowerCase().includes(String(args.query).toLowerCase())));
          case "pages_tag_tree": return [
            { key: "astronomy", label: "astronomy", page_id: "tag-astronomy", children: [], descendant_count: 1, updated_at: 5 },
            { key: "deep sky", label: "deep sky", page_id: "tag-deep-sky", children: [], descendant_count: 1, updated_at: 9 },
          ];
          case "list_blocks": return structuredClone(state.blocks.filter((block) => block.page_id === args.pageId));
          // The API returns a string, not { content }, and source writes return void.
          case "get_page_source": return state.source;
          case "update_page_source": {
            if (typeof args.content !== "string") throw new Error("Expected literal Markdown source");
            state.writes.push({ cmd, args: structuredClone(args) });
            state.source = args.content;
            const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
            state.blocks = parsePageSourceMap(args.content).blocks.map((block, index) =>
              makeBlock(block.id ?? `source-${++sequence}`, index, block.content));
            return;
          }
          case "update_block": {
            state.writes.push({ cmd, args: structuredClone(args) });
            const block = state.blocks.find((item) => item.id === args.id);
            if (!block) throw new Error("Block not found");
            block.content = args.content;
            state.source = serialize();
            return;
          }
          case "insert_block": {
            // Mirrors the native insert: number the displayed siblings 0..n.
            state.writes.push({ cmd, args: structuredClone(args) });
            const siblings = state.blocks
              .filter((item) => item.page_id === args.pageId && (item.parent_id ?? null) === (args.parentId ?? null))
              .sort((a, b) => a.order_index - b.order_index);
            const at = Math.min(args.position, siblings.length);
            siblings.forEach((item, index) => { item.order_index = index < at ? index : index + 1; });
            const block = {
              ...makeBlock(`created-${++sequence}`, at, args.content),
              page_id: args.pageId, parent_id: args.parentId ?? null,
            };
            state.blocks.push(block);
            state.source = serialize();
            return structuredClone(block);
          }
          case "create_block": {
            state.writes.push({ cmd, args: structuredClone(args) });
            const block = {
              ...makeBlock(`created-${++sequence}`, args.orderIndex, args.content),
              page_id: args.pageId, parent_id: args.parentId ?? null,
            };
            state.blocks.push(block);
            state.source = serialize();
            return structuredClone(block);
          }
          case "reorder_blocks":
            args.blockIds.forEach((id, order) => {
              const block = state.blocks.find((item) => item.id === id);
              if (block) block.order_index = order;
            });
            state.source = serialize();
            return;
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
  }, libraryBooks);
  await page.goto(continuous ? new URL("tests/fixtures/unified-editor.html", BASE_URL).href : BASE_URL,
    { waitUntil: "networkidle" });
  if (continuous) {
    await page.locator('.unified-rendered-block[data-source-block-id="draft"]').waitFor();
    assert.equal(await page.locator(".prototype-error").count(), 0);
    assert.ok(await page.evaluate(() => window.__emojiReader.calls.some((call) => call.cmd === "get_page_source")));
  } else await page.locator('[data-block-id="draft"]').first().waitFor();
  if (libraryBooks) {
    assert.deepEqual(await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("reader_snapshot")),
      { libraryPath: "/synthetic/library", books: libraryBooks }, "the Library fixture must seed its native snapshot on every app load");
  }
  return { page, continuous, errors };
}

function rendered(fixture, id) {
  return fixture.continuous
    ? fixture.page.locator(`.unified-rendered-block[data-source-block-id="${id}"] .unified-rendered-content`)
    : fixture.page.locator(`[data-block-id="${id}"] .rendered-content`).first();
}

async function editDraft(fixture) {
  const { page, continuous } = fixture;
  if (continuous) {
    await rendered(fixture, "draft").click();
    const editor = page.locator(".unified-page-editor .cm-content");
    // Clicking a preview puts the caret after its "- " prefix; preserve metadata.
    await editor.press("Shift+End");
    await editor.press("Backspace");
    return editor;
  }
  await page.locator('[data-block-id="draft"] .block-content').first().click();
  const editor = page.locator('[data-block-id="draft"] .cm-content');
  await editor.fill("");
  return editor;
}

async function waitForChoices(page, expected) {
  await page.waitForFunction((expected) => {
    const labels = [...document.querySelectorAll(".cm-tooltip-autocomplete .cm-completionLabel")]
      .map((element) => element.textContent);
    return labels.length === expected.length && expected.every((label) => labels.includes(label));
  }, expected);
}

async function acceptWithKeyboard(page, label) {
  // CodeMirror intentionally ignores completion keys for 75ms after opening.
  await page.waitForTimeout(100);
  const options = page.locator(".cm-tooltip-autocomplete [role=option]");
  const count = await options.count();
  assert.ok(count > 0, "completion choices should auto-open without Ctrl+Space");
  for (let index = 0; index <= count; index++) {
    const selected = page.locator('.cm-tooltip-autocomplete [aria-selected="true"] .cm-completionLabel');
    if (await selected.count() && await selected.innerText() === label) {
      await page.keyboard.press("Enter");
      await page.locator(".cm-tooltip-autocomplete").waitFor({ state: "detached" });
      return;
    }
    await page.keyboard.press("ArrowDown");
  }
  const labels = await options.allTextContents();
  const selected = await page.locator('.cm-tooltip-autocomplete [aria-selected="true"]').allTextContents();
  throw new Error(`Keyboard arrows never selected "${label}"; choices=${JSON.stringify(labels)}, selected=${JSON.stringify(selected)}`);
}

async function saveAndPreview(fixture, expected) {
  const { page, continuous } = fixture;
  if (continuous) {
    await page.getByRole("button", { name: "Save source", exact: true }).click();
  } else {
    await page.keyboard.press("Escape");
  }
  await page.waitForFunction((expected) =>
    window.__emojiReader.blocks.find((block) => block.id === "draft")?.content === expected, expected);
  // A real blur returns the edited block to its preview, without source replacement.
  await page.getByRole("button", { name: continuous ? "Bionic Speedreader" : "Search", exact: true }).focus();
  await rendered(fixture, "draft").waitFor();
  const state = await page.evaluate(() => ({
    source: window.__emojiReader.source,
    writes: window.__emojiReader.writes,
  }));
  assert.ok(state.writes.length > 0, "the accepted value must actually persist");
  assert.ok(state.source.includes(`- ${expected}\n`), "saved Markdown must retain the literal inserted value");
  assert.doesNotMatch(state.source, /grafium-icon|bionic-word|<span|<strong/);
}

async function finish(fixture, message) {
  assert.deepEqual(fixture.errors, [], "no uncaught browser errors");
  await fixture.page.close();
  console.log(`PASS ${message}`);
}

async function completionCases(browser, continuous) {
  const mode = continuous ? "isolated continuous component" : "classic";
  {
    const fixture = await openFixture(browser, continuous);
    const { page } = fixture;
    const editor = await editDraft(fixture);
    await editor.pressSequentially("/emoji");
    await page.locator(".cm-tooltip-autocomplete").waitFor();
    assert.ok(await page.locator(".cm-completionLabel").count() > 3);
    await editor.pressSequentially(" rocket");
    await waitForChoices(page, ["🚀 rocket"]);
    for (let index = 0; index < 6; index++) await editor.press("Backspace");
    await editor.pressSequentially("star");
    await waitForChoices(page, ["⭐ star"]);
    for (let index = 0; index < 4; index++) await editor.press("Backspace");
    await editor.pressSequentially("rocket");
    await waitForChoices(page, ["🚀 rocket"]);
    await acceptWithKeyboard(page, "🚀 rocket");
    await saveAndPreview(fixture, "🚀");
    assert.equal(await rendered(fixture, "draft").innerText(), "🚀");
    await finish(fixture, `${mode}: /emoji auto-opens, refilters edited queries, and persists a literal rocket`);
  }
  for (const command of ["/em", "/icon"]) {
    const fixture = await openFixture(browser, continuous);
    const { page } = fixture;
    const editor = await editDraft(fixture);
    await editor.pressSequentially(`${command} star`);
    await waitForChoices(page, command === "/em"
      ? ["⭐ star", "★ star", "☆ star-outline"]
      : ["★ star", "☆ star-outline"]);
    await acceptWithKeyboard(page, "★ star");
    await saveAndPreview(fixture, ":icon-star:");
    await rendered(fixture, "draft").locator('.grafium-icon[aria-label="star"]').waitFor();
    assert.equal(await rendered(fixture, "draft").innerText(), "★");
    await finish(fixture, `${mode}: ${command} filters choices and keyboard acceptance saves an icon shortcode`);
  }
}

async function readerCase(browser, continuous) {
  const fixture = await openFixture(browser, continuous);
  const { page } = fixture;
  const mode = continuous ? "isolated continuous component" : "classic";
  const toggle = () => continuous
    ? page.getByTitle("Bionic Speedreader", { exact: true }).click()
    : page.keyboard.press("ControlOrMeta+Alt+b");
  const root = continuous ? ".unified-page-editor" : ".page-content";
  await rendered(fixture, "rich").locator('.grafium-icon[aria-label="star"]').waitFor();
  assert.equal(await rendered(fixture, "rich").locator(".grafium-icon").count(), 1);
  assert.equal(await rendered(fixture, "rich").locator("code").first().innerText(), ":icon-star:");
  assert.equal(await rendered(fixture, "rich").locator("code .grafium-icon").count(), 0);
  assert.match(await rendered(fixture, "fenced").locator("code").innerText(), /:icon-star:[\s\S]*\/emoji rocket/);
  assert.equal(await rendered(fixture, "fenced").locator(".grafium-icon").count(), 0);
  await rendered(fixture, "math").locator(".katex").first().waitFor();
  await rendered(fixture, "task").locator(".task-checkbox, .task-marker").first().waitFor();
  const before = await page.evaluate(() => ({
    source: window.__emojiReader.source,
    writes: window.__emojiReader.writes.length,
  }));
  assert.equal(await page.locator(`${root} .bionic-word`).count(), 0);
  if (!continuous) assert.equal(await page.locator(".titlebar .bionic-toggle-icon").count(), 0);
  await toggle();
  await rendered(fixture, "prose").locator(".bionic-word").first().waitFor({ timeout: 2_000 });
  assert.equal(await page.locator(`${root} code .bionic-word, ${root} pre .bionic-word, ${root} .katex .bionic-word, ${root} .task-checkbox .bionic-word, ${root} .task-marker .bionic-word, ${root} .priority .bionic-word, ${root} .grafium-icon .bionic-word`).count(), 0);
  assert.equal(await page.evaluate(() => localStorage.getItem("grafium.reader.bionic")), "1");
  await toggle();
  await page.waitForFunction((root) => !document.querySelector(`${root} .bionic-word`), root);
  assert.equal(await page.evaluate(() => localStorage.getItem("grafium.reader.bionic")), "0");
  assert.equal(await rendered(fixture, "prose").innerText(), "Observatory notes connect distant ideas with careful observations.");
  assert.deepEqual(await page.evaluate(() => ({
    source: window.__emojiReader.source,
    writes: window.__emojiReader.writes.length,
  })), before, "reader toggling must not issue any note/source writes");
  await toggle();
  await page.reload({ waitUntil: "networkidle" });
  assert.equal(await page.evaluate(() => localStorage.getItem("grafium.reader.bionic")), "1");
  await page.locator(".rendered-content .bionic-word").first().waitFor();
  await finish(fixture, `${mode}: live Bionic toggles existing previews, skips code/math/task markers, and survives reload without note edits`);
}

async function nativeArrowCase(browser, continuous) {
  const fixture = await openFixture(browser, continuous);
  const { page } = fixture;
  const editor = await editDraft(fixture);
  await editor.pressSequentially("/em star");
  await waitForChoices(page, ["⭐ star", "★ star", "☆ star-outline"]);
  await page.waitForTimeout(100);
  const before = await page.evaluate(() => {
    const view = window.__activeEditorView;
    return { source: view.state.doc.toString(), selection: view.state.selection.toJSON() };
  });
  for (const [direction, label] of [["down", "★ star"], ["up", "⭐ star"], ["down", "★ star"]]) {
    assert.equal(await page.evaluate((direction) =>
      window.__handleNativeVerticalArrow(direction, false), direction), true);
    await page.waitForFunction((label) =>
      document.querySelector('.cm-tooltip-autocomplete [aria-selected="true"] .cm-completionLabel')?.textContent === label, label);
    assert.deepEqual(await page.evaluate(() => {
      const view = window.__activeEditorView;
      return { source: view.state.doc.toString(), selection: view.state.selection.toJSON() };
    }), before, "native-dispatch completion navigation must not move the text caret or edit source");
  }

  await acceptWithKeyboard(page, "★ star");
  await saveAndPreview(fixture, ":icon-star:");
  await finish(fixture, `${continuous ? "continuous" : "classic"}: native vertical-arrow callback moves completion selection, not the caret`);
}

async function globalReaderShortcutCase(browser) {
    const fixture = await openFixture(browser);
    const { page } = fixture;
    const editor = page.locator('[data-block-id="draft"] .cm-content');
    await page.locator('[data-block-id="draft"] .block-content').first().click();
    await editor.waitFor();
    const before = await page.evaluate(() => ({
      source: window.__emojiReader.source, writes: window.__emojiReader.writes.length,
    }));
    await page.keyboard.press("ControlOrMeta+Alt+b");
    await rendered(fixture, "prose").locator(".bionic-word").first().waitFor();
    assert.equal(await editor.evaluate(element => element.contains(document.activeElement)), true);
    assert.deepEqual(await page.evaluate(() => ({
      source: window.__emojiReader.source, writes: window.__emojiReader.writes.length,
    })), before, "Bionic shortcut must not apply bold or write the focused note");

    await page.evaluate(() => {
      const mac = navigator.platform.includes("Mac");
      for (const extra of [{ repeat: true }, { isComposing: true }, { shiftKey: true }]) {
        document.activeElement.dispatchEvent(new KeyboardEvent("keydown", {
          key: "b", code: "KeyB", ctrlKey: !mac, metaKey: mac, altKey: true,
          bubbles: true, cancelable: true, ...extra,
        }));
      }
    });
    assert.equal(await page.evaluate(() => localStorage.getItem("grafium.reader.bionic")), "1");

    await page.keyboard.press("ControlOrMeta+Shift+p");
    await page.getByRole("dialog", { name: "Command palette", exact: true }).waitFor();
    await page.keyboard.press("ControlOrMeta+Alt+b");
    assert.equal(await page.evaluate(() => localStorage.getItem("grafium.reader.bionic")), "1",
      "reader hotkeys must not steal a modal's keys");
    await page.keyboard.press("Escape");
    await page.evaluate(() => {
      window.__bookmarkShortcutCount = 0;
      window.addEventListener("grafium-bookmark", event => {
        event.preventDefault();
        window.__bookmarkShortcutCount++;
      }, { once: true });
    });
    await page.keyboard.press("ControlOrMeta+Alt+m");
    assert.equal(await page.evaluate(() => window.__bookmarkShortcutCount), 1);
    assert.equal(await page.getByText("Open a book in Library or start Library playback", { exact: false }).count(), 0);
    await page.keyboard.press("ControlOrMeta+Alt+m");
    await page.getByText("Open a book in Library or start Library playback", { exact: false }).waitFor();
    await finish(fixture, "global reader shortcuts preserve editor text, reject repeats/composition/modals, and route bookmarks honestly");
}

async function librarySearchCase(browser, phone) {
  const fixture = await openFixture(browser, false, phone ? 420 : 1400,
    ["Distant stars", "Ocean tides"].map((title, index) => ({
      id: `shelf-${index}`, title, kind: "epub", available: true,
      tracks: [], bookmarks: [], position: null,
    })));
  const { page } = fixture;
  await page.getByRole("button", { name: "Library", exact: true }).first().click();
  const search = page.getByRole("searchbox", { name: "Search Library", exact: true });
  await search.waitFor();
  await search.fill("stars");
  assert.deepEqual(await page.locator(".private-library .book-title").allTextContents(), ["Distant stars"]);
  await page.getByRole("button", { name: "Rescan", exact: true }).focus();
  await page.keyboard.press("ControlOrMeta+f");
  assert.equal(await search.evaluate(element => document.activeElement === element), true);
  assert.deepEqual(await search.evaluate(element => [element.selectionStart, element.selectionEnd]), [0, 5]);
  assert.match(await search.getAttribute("title"), /Search Library \((Ctrl|Cmd)-F\)/);
  const bounds = await search.evaluate(element => {
    const box = element.getBoundingClientRect();
    const parent = element.closest(".private-library").getBoundingClientRect();
    return { width: box.width, height: box.height, x: box.x, right: box.right, parentX: parent.x, parentRight: parent.right };
  });
  assert.ok(bounds.height >= 40 && bounds.width >= (phone ? 200 : 300), JSON.stringify(bounds));
  assert.ok(bounds.x >= bounds.parentX && bounds.right <= bounds.parentRight + 1, "search stays inside the shelf");
  await page.keyboard.press("ControlOrMeta+Shift+p");
  const palette = page.getByRole("dialog", { name: "Command palette", exact: true });
  await palette.waitFor();
  await page.keyboard.press("ControlOrMeta+f");
  assert.equal(await palette.evaluate(element => element.contains(document.activeElement)), true,
    "Library search must not steal focus from an open command palette");
  await finish(fixture, `Library Ctrl/Cmd+F selects its query with a wider bounded search field on ${phone ? "phone" : "desktop"}`);
}

async function libraryTypePersistenceCase(browser) {
  const fixture = await openFixture(browser, false, 1400, [
    { id: "reading", title: "Reading", kind: "epub", available: false, tracks: [], bookmarks: [], position: null },
    { id: "listening", title: "Listening", kind: "audio", available: false, favorite: true, tracks: [], bookmarks: [], position: null },
  ]);
  const { page } = fixture;
  const openShelf = () => page.getByRole("button", { name: "Library", exact: true }).first().click();
  const filter = page.locator(".private-library .filters select");
  const search = page.getByRole("searchbox", { name: "Search Library", exact: true });
  await openShelf();
  await filter.selectOption("audio");
  assert.deepEqual(await page.locator(".private-library .book-title").allTextContents(), ["Listening"]);
  await search.fill("Listen");
  await page.getByRole("button", { name: "★ Favorites", exact: true }).click();
  await page.getByRole("button", { name: "Listening", exact: true }).click();
  await page.getByRole("heading", { name: "Listening", exact: true }).waitFor();
  await page.getByRole("button", { name: "← Library", exact: true }).click();
  await filter.waitFor();
  assert.equal(await filter.inputValue(), "audio", "opening and returning from a book retains Audio");
  assert.equal(await search.inputValue(), "", "search keeps its existing nonpersistent behavior");
  assert.equal(await page.getByRole("button", { name: "★ Favorites", exact: true }).getAttribute("aria-pressed"), "false");
  assert.equal(await page.evaluate(() => localStorage.getItem("grafium.library.mediaType")), "audio");
  await page.reload({ waitUntil: "networkidle" });
  await openShelf();
  await filter.waitFor();
  assert.equal(await filter.inputValue(), "audio", "a fresh app instance restores the saved type");
  assert.deepEqual(await page.locator(".private-library .book-title").allTextContents(), ["Listening"]);
  await filter.selectOption("all");
  await page.reload({ waitUntil: "networkidle" });
  await openShelf();
  await filter.waitFor();
  assert.equal(await filter.inputValue(), "all", "resetting to All types is also remembered");
  assert.deepEqual(await page.locator(".private-library .book-title").allTextContents(), ["Listening", "Reading"]);
  await finish(fixture, "Library media type survives book navigation and app reload without persisting search or Favorites");
}

async function headingLayoutCase(browser, phone) {
  const fixture = await openFixture(browser, false, phone ? 420 : 1400);
  const { page } = fixture;
  if (!phone) {
    await page.locator(".page-content").evaluate((element) => {
      element.style.width = "550px";
      element.style.maxWidth = "100%";
      const contentWidth = element.querySelector(".page-heading").getBoundingClientRect().width;
      element.style.width = `${550 + (550 - contentWidth)}px`;
    });
  }
  const bounds = await page.locator(".page-heading").evaluate((heading) => {
    const box = (element) => {
      const { x, y, width, height, right, bottom } = element.getBoundingClientRect();
      return { x, y, width, height, right, bottom };
    };
    const title = heading.querySelector(".page-title");
    const style = getComputedStyle(title);
    return {
      heading: box(heading), title: box(title),
      actions: box(heading.querySelector(".page-heading-actions")),
      buttons: [...heading.querySelectorAll("button")].map(box),
      lineHeight: Number.parseFloat(style.lineHeight) || Number.parseFloat(style.fontSize) * 1.2,
    };
  });
  if (!phone) assert.ok(Math.abs(bounds.heading.width - 550) < 2, "exercise the requested narrow content width");
  assert.ok(bounds.title.width >= Math.min(240, bounds.heading.width - 60),
    `action buttons must not squeeze the title into a tiny column: ${JSON.stringify(bounds)}`);
  assert.ok(bounds.title.height <= bounds.lineHeight * 3 + 2, "ordinary title should occupy at most three readable lines");
  for (const element of [bounds.title, bounds.actions, ...bounds.buttons]) {
    assert.ok(element.x >= bounds.heading.x - 1 && element.right <= bounds.heading.right + 1,
      `heading controls must wrap within the content: ${JSON.stringify(bounds)}`);
  }
  assert.equal(await page.getByRole("button", { name: "Experimental continuous editor", exact: true }).count(), 0,
    "the disabled prototype must not be advertised in the shipped UI");
  await finish(fixture, `heading stays readable and controls stay bounded at ${phone ? "420px phone viewport" : "550px content width"}`);
}

async function tablePreservationCase(browser) {
  const fixture = await openFixture(browser);
  const { page } = fixture;
  const table = rendered(fixture, "table");
  await table.getByRole("columnheader", { name: /^Count/ }).click();
  await page.waitForFunction(() => {
    const text = window.__emojiReader.blocks.find((block) => block.id === "table").content;
    return text.indexOf("| Sun |") < text.indexOf("| Venus |") && text.indexOf("| Venus |") < text.indexOf("| Moon |");
  });
  await table.locator("tbody tr").first().waitFor();
  assert.deepEqual(await table.locator("tbody tr td:first-child").allTextContents(), ["Sun", "Venus", "Moon"]);
  await table.getByRole("columnheader", { name: /^Count/ }).click();
  await page.waitForFunction(() => {
    const text = window.__emojiReader.blocks.find((block) => block.id === "table").content;
    return text.indexOf("| Moon |") < text.indexOf("| Venus |") && text.indexOf("| Venus |") < text.indexOf("| Sun |");
  });
  await table.locator("tbody tr").first().waitFor();
  assert.deepEqual(await table.locator("tbody tr td:first-child").allTextContents(), ["Moon", "Venus", "Sun"]);
  await finish(fixture, "classic: numeric table sorting preserves rendering and persists both directions");
}

async function tagAndCommandCase(browser) {
  const fixture = await openFixture(browser);
  const { page } = fixture;
  const labels = () => page.locator(".cm-tooltip-autocomplete .cm-completionLabel").allTextContents();
  let editor = await editDraft(fixture);
  // # opens the tag picker straight away: tags first, then other pages.
  await editor.pressSequentially("Look up #");
  await page.locator(".cm-tooltip-autocomplete").waitFor();
  await page.waitForFunction(() => document.querySelectorAll(".cm-tooltip-autocomplete .cm-completionLabel").length >= 3);
  const all = await labels();
  assert.deepEqual(all.slice(0, 2), ["#deep sky", "#astronomy"], "existing tags lead, newest first");
  assert.ok(all.includes("#Space/Orbits"), "other pages follow the tags");
  // Typing filters by substring, including the middle of a title; what was
  // typed is offered last, as a new tag.
  await editor.pressSequentially("sky");
  await waitForChoices(page, ["#deep sky", "#sky"]);
  await acceptWithKeyboard(page, "#deep sky");
  await editor.pressSequentially("and #astro");
  await waitForChoices(page, ["#astronomy", "#astro"]);
  await acceptWithKeyboard(page, "#astronomy");
  // The tag ends with a space, so typing on does not reopen the picker.
  await editor.pressSequentially("tonight");
  await saveAndPreview(fixture, "Look up #[[deep sky]] and #astronomy tonight");

  // A namespaced title inside [[...]] never offers / commands.
  editor = await editDraft(fixture);
  await editor.pressSequentially("[[Space/");
  await page.locator(".cm-tooltip-autocomplete .cm-completionLabel").filter({ hasText: "Space/Orbits" }).waitFor();
  assert.ok((await labels()).every((label) => !label.startsWith("/")), `no commands in the link picker: ${await labels()}`);
  await editor.press("Escape");
  await editor.fill("");

  // /time inserts the current time, like Alt+T.
  await editor.pressSequentially("/tim");
  await waitForChoices(page, ["/time"]);
  await acceptWithKeyboard(page, "/time");
  await page.waitForFunction(() => /^\d{2}:\d{2}$/.test(
    window.__emojiReader.blocks.find((block) => block.id === "draft")?.content ?? ""));
  await finish(fixture, "classic: # opens a filtered tag picker, [[ never offers / commands, and /time inserts the time");
}

async function tagPickerBehaviorCase(browser) {
  const fixture = await openFixture(browser);
  const { page } = fixture;
  const pickerCount = () => page.locator(".cm-tooltip-autocomplete").count();
  let editor = await editDraft(fixture);
  // Loose matches never replace a new tag: what was typed comes first.
  await editor.pressSequentially("Saw #asy");
  await waitForChoices(page, ["#asy", "#astronomy"]);
  assert.equal(await page.locator('.cm-tooltip-autocomplete [aria-selected="true"] .cm-completionLabel').innerText(), "#asy");
  assert.match(await page.locator(".cm-tooltip-autocomplete").innerText(), /new tag/);
  await acceptWithKeyboard(page, "#asy");
  await editor.pressSequentially("tonight");
  await saveAndPreview(fixture, "Saw #asy tonight");

  // Moving into a tag that is already written leaves the picker closed.
  editor = await editDraft(fixture);
  await editor.pressSequentially("Tagged #astronomy today");
  // Escape right after typing past a tag leaves editing, not a menu still loading.
  await page.keyboard.press("Escape");
  await page.waitForFunction(() =>
    window.__emojiReader.blocks.find((block) => block.id === "draft")?.content === "Tagged #astronomy today");
  await page.locator('[data-block-id="draft"] .block-content').first().click();
  editor = page.locator('[data-block-id="draft"] .cm-content');
  await editor.waitFor();
  await page.keyboard.press("End");
  for (let index = 0; index < " today".length; index++) await page.keyboard.press("ArrowLeft");
  await page.waitForTimeout(400);
  assert.equal(await pickerCount(), 0, "the cursor resting after #astronomy must not open the picker");

  // Clicking another block while the picker shows moves there for good.
  await page.keyboard.press("End");
  await editor.pressSequentially(" #ast");
  await page.locator(".cm-tooltip-autocomplete").waitFor();
  await page.locator('[data-block-id="prose"] .block-content').first().click();
  await page.waitForTimeout(500);
  assert.equal(await page.evaluate(() => document.activeElement?.closest("[data-block-id]")?.getAttribute("data-block-id")),
    "prose", "the tag picker must not pull focus back to the block it was opened in");
  await page.keyboard.press("Escape");

  // Enter while suggestions are still loading starts the next block at once.
  await page.evaluate(() => { window.__searchDelayMs = 600; });
  editor = await editDraft(fixture);
  await editor.pressSequentially("Pending #ast");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() =>
    window.__emojiReader.blocks.find((block) => block.id === "draft")?.content === "Pending #ast");
  await page.waitForFunction(() => window.__emojiReader.writes.some((write) => write.cmd === "insert_block" || write.cmd === "create_block"));
  await page.waitForTimeout(900);
  assert.equal(await pickerCount(), 0, "results that arrive after Enter must not open a picker");
  assert.notEqual(await page.evaluate(() => document.activeElement?.closest("[data-block-id]")?.getAttribute("data-block-id")),
    "draft", "focus stays in the new block");
  await page.evaluate(() => { window.__searchDelayMs = 0; });
  await finish(fixture, "classic: tag picker keeps typed tags, opens only while typing, never steals focus, and lets Enter through while loading");
}

async function refreshingMenuEnterCase(browser) {
  const fixture = await openFixture(browser);
  const { page } = fixture;
  await page.evaluate(() => { window.__searchDelayMs = 500; });
  const editor = await editDraft(fixture);
  await editor.pressSequentially("[[Orb");
  await page.locator(".cm-tooltip-autocomplete .cm-completionLabel").filter({ hasText: "Space/Orbits" }).waitFor();
  // The menu stays on screen, greyed out, while it refreshes for "i": Enter
  // must not split the block and leave a half-typed link behind.
  await editor.pressSequentially("i");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(150);
  const writes = await page.evaluate(() => window.__emojiReader.writes.map((write) => write.cmd));
  assert.ok(!writes.includes("insert_block") && !writes.includes("create_block"), `no new block: ${writes}`);
  assert.equal(await editor.innerText(), "[[Orbi");
  await page.locator(".cm-tooltip-autocomplete:not(.cm-tooltip-autocomplete-disabled) .cm-completionLabel")
    .filter({ hasText: "Space/Orbits" }).waitFor();
  await acceptWithKeyboard(page, "Space/Orbits");
  await page.evaluate(() => { window.__searchDelayMs = 0; });
  await saveAndPreview(fixture, "[[Space/Orbits]]");
  await finish(fixture, "classic: Enter waits for a refreshing menu instead of splitting the block");
}

async function nativePeriodCase(browser) {
  const fixture = await openFixture(browser);
  const { page } = fixture;
  // Settings moved Ctrl+. from the right sidebar to Italic.
  await page.evaluate(() => localStorage.setItem("grafium.shortcuts.v1", JSON.stringify({
    version: 1, bindings: { "editor-italic": ["mod+i", "mod+."], "toggle-right-sidebar": ["t r", "mod+shift+b"] },
  })));
  await page.reload({ waitUntil: "networkidle" });
  await page.locator('[data-block-id="draft"]').first().waitFor();
  const editor = await editDraft(fixture);
  await editor.pressSequentially("word");
  await page.keyboard.press("Shift+Home");
  // On Linux the window takes Ctrl+. first and hands it back through this hook.
  await page.evaluate(() => window.__toggleReferencePanel());
  await page.waitForFunction(() => document.querySelector('[data-block-id="draft"] .cm-content')?.textContent === "*word*");
  assert.equal(await page.locator(".reference-panel").count(), 0, "the right sidebar no longer owns Ctrl+.");
  await page.evaluate(() => localStorage.removeItem("grafium.shortcuts.v1"));
  await finish(fixture, "classic: native Ctrl+. reaches the editor action Settings bound it to");
}

async function wikiPreservationCase(browser) {
  const fixture = await openFixture(browser);
  const { page } = fixture;
  const editor = await editDraft(fixture);
  await editor.pressSequentially("[[Orbits");
  await page.locator(".cm-tooltip-autocomplete .cm-completionLabel").filter({ hasText: "Space/Orbits" }).waitFor();
  await acceptWithKeyboard(page, "Space/Orbits");
  await saveAndPreview(fixture, "[[Space/Orbits]]");
  await rendered(fixture, "draft").locator(".page-link").waitFor();
  await finish(fixture, "classic: wiki completion still opens, accepts by keyboard and saves the page link");
}

(async () => {
  let browser;
  try {
    browser = await chromium.launch({ args: ["--no-sandbox"] });
    const failures = [];
    for (const continuous of process.env.UI_TEST_CASE === "preservation" ? [] : [false, true]) {
      for (const run of [
        () => completionCases(browser, continuous),
        () => readerCase(browser, continuous),
        () => nativeArrowCase(browser, continuous),
      ]) {
        try { await run(); }
        catch (error) {
          failures.push(error);
          console.error(`FAIL ${continuous ? "continuous" : "classic"}:`, error);
        }
      }
    }
    for (const run of [tablePreservationCase, wikiPreservationCase, tagAndCommandCase, tagPickerBehaviorCase,
      refreshingMenuEnterCase, nativePeriodCase]) {
      try { await run(browser); }
      catch (error) { failures.push(error); console.error(`FAIL ${run.name}:`, error); }
    }
    if (process.env.UI_TEST_CASE !== "preservation") {
      try { await globalReaderShortcutCase(browser); }
      catch (error) { failures.push(error); console.error("FAIL global reader shortcuts:", error); }
      try { await libraryTypePersistenceCase(browser); }
      catch (error) { failures.push(error); console.error("FAIL Library type persistence:", error); }
      for (const phone of [false, true]) {
        try { await librarySearchCase(browser, phone); }
        catch (error) { failures.push(error); console.error(`FAIL Library search ${phone ? "phone" : "desktop"}:`, error); }
        try { await headingLayoutCase(browser, phone); }
        catch (error) { failures.push(error); console.error(`FAIL ${phone ? "phone" : "narrow"} heading:`, error); }
      }
    }
    if (failures.length) throw new Error(`${failures.length} emoji/reader integration group(s) failed`);
  } finally {
    if (browser) await browser.close();
    fs.rmSync(runtimeDir, { recursive: true, force: true });
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
