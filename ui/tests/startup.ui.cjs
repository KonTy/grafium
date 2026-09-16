const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function newPage(browser, cache = JSON.stringify({
  background: "#000000", foreground: "#f2f2f2", colorScheme: "dark",
}), holdTheme = false, sidebar = {}) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.exposeFunction("__loadLayoutPreferences", () => {
    if (sidebar.readError) throw new Error("Preference read failed");
    return { sidebarVisible: sidebar.visible ?? true, wideMode: sidebar.wideMode ?? true };
  });
  await page.exposeFunction("__saveLayoutPreferences", (preferences) => {
    if (sidebar.failSaveOnce) {
      sidebar.failSaveOnce = false;
      throw new Error("Preference write failed");
    }
    if (preferences.sidebarVisible !== undefined) sidebar.visible = preferences.sidebarVisible;
    if (preferences.wideMode !== undefined) sidebar.wideMode = preferences.wideMode;
  });
  await page.addInitScript(({ cache, holdTheme, holdSidebar, holdSidebarSave }) => {
    if (cache !== null) localStorage.setItem("grafium.startupTheme", cache);
    if (!localStorage.getItem("grafium.session.lastLocation")) {
      localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "page", title: "Startup regression" }));
    }
    localStorage.setItem("grafium.pageContent.showBlockGuides", "true");
    const note = {
      id: "startup-page", title: "Startup regression", properties: {}, is_journal: false,
      file_path: "pages/Startup regression.md", created_at: 0, updated_at: 0,
    };
    let sequence = 0;
    window.__startupReveals = [];
    window.__holdTheme = holdTheme;
    window.__holdSidebar = holdSidebar;
    window.__holdSidebarSave = holdSidebarSave;
    window.__sidebarSaveRequests = [];
    window.__sidebarWrites = [];
    window.__layoutWrites = [];
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        switch (cmd) {
          case "get_page":
            if (args.id === note.id || args.title === note.title) return structuredClone(note);
            throw new Error("Page not found");
          case "get_graph_info": return { name: "Startup test", path: "/tmp/startup-test" };
          case "get_layout_preferences": {
            const saved = await window.__loadLayoutPreferences();
            if (window.__holdSidebar) {
              await new Promise((resolve) => { window.__releaseSidebar = resolve; });
            }
            return saved;
          }
          case "set_layout_preferences":
            if (args.preferences.sidebarVisible !== undefined) window.__sidebarSaveRequests.push(args.preferences.sidebarVisible);
            if (window.__holdSidebarSave) {
              await new Promise((resolve) => {
                window.__releaseSidebarSave = () => {
                  window.__holdSidebarSave = false;
                  resolve();
                };
              });
            }
            await window.__saveLayoutPreferences(args.preferences);
            window.__layoutWrites.push(args.preferences);
            if (args.preferences.sidebarVisible !== undefined) window.__sidebarWrites.push(args.preferences.sidebarVisible);
            return;
          case "get_app_theme":
            if (window.__holdTheme) {
              await new Promise((resolve) => { window.__releaseStartupTheme = resolve; });
            }
            return "oled";
          case "get_smplos_theme": return null;
          case "reveal_startup_window":
            window.__startupReveals.push({
              background: args.background,
              appMounted: !!document.querySelector(".app-shell"),
              cssBackground: getComputedStyle(document.documentElement).getPropertyValue("--bg-primary").trim(),
              sidebarVisible: !!document.querySelector(".sidebar-container"),
              wideMode: document.querySelector(".app-shell")?.classList.contains("wide-mode") ?? false,
            });
            return;
          case "get_graph_data": return {
            nodes: [{ id: "a", title: "Alpha", degree: 1 }, { id: "b", title: "Beta", degree: 1 }],
            edges: [{ source: "a", target: "b", weight: 1 }],
          };
          case "list_pages":
          case "list_page_summaries": return [structuredClone(note)];
          case "list_blocks": return [{
            id: "startup-block", page_id: note.id, parent_id: null, order_index: 0,
            content: "Ready to edit", block_type: "markdown", properties: {},
            created_at: 0, updated_at: 0,
          }];
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
  }, { cache, holdTheme, holdSidebar: !!sidebar.hold, holdSidebarSave: !!sidebar.holdSave });
  return { page, errors };
}

const navigate = (page, title) => page.evaluate((title) => {
  window.dispatchEvent(new CustomEvent("navigate-page", { detail: title }));
}, title);

(async () => {
  const browser = await chromium.launch({
    args: ["--no-sandbox", "--enable-unsafe-swiftshader", "--use-angle=swiftshader"],
  });
  try {
    for (const cache of [undefined, null, "not valid json"]) {
      const { page, errors } = await newPage(browser, cache);
      let release;
      const gate = new Promise((resolve) => { release = resolve; });
      await page.route(/\/src\/main\.ts(?:\?|$)/, async (route) => { await gate; await route.continue(); });
      await page.goto(BASE_URL, { waitUntil: "commit" });
      await page.locator("#grafium-startup-message").waitFor();
      assert.equal(await page.locator("#app").evaluate((app) => app.childElementCount), 0);
      const expectedBackground = cache === undefined ? "rgb(0, 0, 0)" : "rgb(255, 255, 255)";
      assert.equal(await page.locator("#grafium-startup").evaluate((el) => getComputedStyle(el).backgroundColor), expectedBackground);
      release();
      await page.locator('[data-block-id="startup-block"]').first().waitFor();
      await page.locator("#grafium-startup").waitFor({ state: "detached" });
      assert.deepEqual(errors, []);
      assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem("grafium.startupTheme")).background), "#000000");
      await page.close();
    }
    console.log("PASS immediate themed shell before JavaScript, safe missing/corrupt cache, shell removed after mount");

    {
      const { page, errors } = await newPage(browser, null, true);
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await page.waitForFunction(() => typeof window.__releaseStartupTheme === "function");
      assert.equal(await page.evaluate(() => window.__startupReveals.length), 0,
        "native window must stay hidden until the actual saved theme arrives");
      await page.evaluate(() => window.__releaseStartupTheme());
      await page.waitForFunction(() => window.__startupReveals.length === 1);
      assert.deepEqual(await page.evaluate(() => window.__startupReveals[0]), {
        background: [0, 0, 0], appMounted: true, cssBackground: "#000000",
        sidebarVisible: true,
        wideMode: true,
      });
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS native window reveal waits for the saved theme and mounted app");
    }

    {
      const { page, errors } = await newPage(browser);
      await page.setViewportSize({ width: 1, height: 1 });
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await page.waitForFunction(() => window.__startupReveals.length === 1);
      await page.setViewportSize({ width: 1200, height: 900 });
      await page.locator(".sidebar-container").waitFor({ state: "visible" });
      assert.equal(await page.evaluate(() => window.__startupReveals[0].sidebarVisible), true);
      assert.deepEqual(await page.evaluate(() => window.__sidebarWrites), []);
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS first launch keeps the menu open after a hidden window's tiny initial viewport");
    }

    {
      const sidebar = {};
      const first = await newPage(browser, undefined, false, sidebar);
      await first.page.goto(BASE_URL, { waitUntil: "networkidle" });
      await first.page.keyboard.press("Alt+w");
      await first.page.waitForFunction(() => window.__layoutWrites.some((patch) => patch.wideMode === false));
      await first.page.keyboard.press("Control+b");
      await first.page.locator(".sidebar input").first().waitFor({ state: "visible" });
      await first.page.keyboard.press("Control+b");
      await first.page.waitForFunction(() => window.__sidebarWrites.at(-1) === false);
      await first.page.locator(".sidebar-container").waitFor({ state: "detached" });
      await first.page.close();
      assert.equal(sidebar.visible, false);
      assert.equal(sidebar.wideMode, false);

      const second = await newPage(browser, undefined, false, sidebar);
      await second.page.addInitScript(() => localStorage.setItem("grafium.ui.wideMode", "true"));
      await second.page.setViewportSize({ width: 1, height: 1 });
      await second.page.goto(BASE_URL, { waitUntil: "networkidle" });
      await second.page.waitForFunction(() => window.__startupReveals.length === 1);
      assert.equal(await second.page.evaluate(() => window.__startupReveals[0].sidebarVisible), false);
      assert.equal(await second.page.evaluate(() => window.__startupReveals[0].wideMode), false);
      await second.page.setViewportSize({ width: 1200, height: 900 });
      assert.equal(await second.page.locator(".app-shell.wide-mode").count(), 0);
      assert.deepEqual(await second.page.evaluate(() => window.__sidebarWrites), []);
      await second.page.keyboard.press("Control+b");
      await second.page.waitForFunction(() => window.__sidebarWrites.at(-1) === true);
      await second.page.close();
      assert.equal(sidebar.visible, true);

      const third = await newPage(browser, undefined, false, sidebar);
      await third.page.goto(BASE_URL, { waitUntil: "networkidle" });
      await third.page.locator(".sidebar-container").waitFor({ state: "visible" });
      assert.equal(await third.page.locator(".app-shell.wide-mode").count(), 0);
      await third.page.keyboard.press("Alt+z");
      await third.page.locator(".app-shell.zen").waitFor();
      assert.deepEqual(await third.page.evaluate(() => window.__sidebarWrites), [],
        "Zen's temporary hiding must not replace the saved menu preference");
      await third.page.keyboard.press("Control+b");
      await third.page.locator(".sidebar-container").waitFor({ state: "visible" });
      assert.equal(await third.page.locator(".app-shell.zen").count(), 0);
      for (const { errors } of [first, second, third]) assert.deepEqual(errors, []);
      await third.page.close();
      console.log("PASS closed/open and narrow choices survive fresh browser contexts, stale browser cache and a tiny initial viewport");
    }

    {
      const sidebar = { visible: false, hold: true, holdSave: true };
      const { page, errors } = await newPage(browser, undefined, false, sidebar);
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await page.waitForFunction(() => typeof window.__releaseSidebar === "function");
      assert.equal(await page.evaluate(() => window.__startupReveals.length), 0,
        "native reveal must wait for the persisted menu state");
      await page.keyboard.press("Control+b");
      await page.keyboard.press("Control+b");
      await page.waitForFunction(() => typeof window.__releaseSidebarSave === "function");
      await page.keyboard.press("Control+b");
      await page.locator(".sidebar-container").waitFor({ state: "visible" });
      await page.keyboard.press("Alt+w");
      assert.deepEqual(await page.evaluate(() => window.__sidebarSaveRequests), [false],
        "a second write must wait for the first write to finish");
      await page.evaluate(() => window.__releaseSidebarSave());
      await page.waitForFunction(() => window.__sidebarWrites.length === 2);
      await page.waitForFunction(() => window.__layoutWrites.length === 3);
      assert.deepEqual(await page.evaluate(() => window.__sidebarWrites), [false, true]);
      await page.evaluate(() => window.__releaseSidebar());
      await page.waitForFunction(() => window.__startupReveals.length === 1);
      assert.equal(await page.evaluate(() => window.__startupReveals[0].sidebarVisible), true,
        "a stale startup read must not undo a more recent explicit choice");
      assert.equal(sidebar.visible, true);
      assert.equal(sidebar.wideMode, false);
      assert.equal(await page.evaluate(() => window.__startupReveals[0].wideMode), false);
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS menu restoration gates reveal, ignores stale reads, and serializes rapid saves");
    }

    {
      const sidebar = { readError: true, failSaveOnce: true };
      const { page, errors } = await newPage(browser, undefined, false, sidebar);
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await page.getByText(/Could not restore layout settings:/).waitFor();
      await page.waitForFunction(() => window.__startupReveals.length === 1);
      await page.keyboard.press("Control+b");
      await page.keyboard.press("Control+b");
      await page.getByText(/Could not save layout settings:/).waitFor();
      await page.keyboard.press("Control+b");
      await page.waitForFunction(() => window.__sidebarWrites.at(-1) === true);
      assert.equal(sidebar.visible, true, "a failed write must not poison later saves");
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS menu preference failures are visible and later choices can still be saved");
    }

    {
      const { page } = await newPage(browser);
      let failed = false;
      await page.route(/\/src\/main\.ts(?:\?|$)/, async (route) => {
        if (!failed) { failed = true; await route.abort(); }
        else await route.continue();
      });
      await page.goto(BASE_URL, { waitUntil: "load" });
      await page.locator('#grafium-startup[role="alert"]').waitFor();
      assert.ok(await page.evaluate(() => window.__startupReveals.length > 0),
        "failed bootstrap must reveal its recovery controls");
      await page.getByRole("button", { name: "Retry", exact: true }).click();
      await page.locator('[data-block-id="startup-block"]').first().waitFor();
      await page.locator("#grafium-startup").waitFor({ state: "detached" });
      await page.close();
      console.log("PASS bootstrap failure is visible and reload retry recovers");
    }

    {
      const { page, errors } = await newPage(browser);
      const requests = [];
      page.on("request", (request) => requests.push(new URL(request.url()).pathname));
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await page.locator('[data-block-id="startup-block"]').first().waitFor();
      for (const unused of ["AllPages", "GraphView", "GraphView3D", "3d-force-graph", "Statistics", "FlashcardReview", "ChatView", "Settings", "JobsView", "ReferencePanel"]) {
        assert.ok(!requests.some((url) => url.includes(unused)), `${unused} must not load on editor startup`);
      }
      let release;
      const gate = new Promise((resolve) => { release = resolve; });
      await page.route(/\/src\/components\/AllPages\.svelte(?:\?|$)/, async (route) => { await gate; await route.continue(); });
      await navigate(page, "__all_pages__");
      await page.getByRole("status").filter({ hasText: "Loading all pages..." }).waitFor();
      await navigate(page, "Startup regression");
      await page.locator('[data-block-id="startup-block"]').first().waitFor();
      release();
      await page.waitForResponse((response) => new URL(response.url()).pathname.endsWith("/AllPages.svelte"));
      assert.equal(await page.locator(".all-pages").count(), 0, "late import must not change navigation");
      await navigate(page, "__all_pages__");
      await page.locator(".all-pages").waitFor();
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS optional screens load only on demand; navigation during import does not mount a stale screen");
    }

    {
      const { page, errors } = await newPage(browser);
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await navigate(page, "__graph__");
      await page.getByRole("button", { name: "3D", exact: true }).click();
      await page.waitForFunction(() => {
        const start = document.querySelector('button[aria-label="Space flight"]');
        return start && !start.disabled;
      });
      await page.locator(".graph-view-3d canvas").waitFor();
      await page.getByRole("button", { name: "Space flight", exact: true }).click();
      await page.getByRole("button", { name: "Stop flight", exact: true }).waitFor();
      await page.getByRole("button", { name: "Stop flight", exact: true }).click();
      await page.getByRole("button", { name: "2D", exact: true }).click();
      await page.locator(".graph-view-3d").waitFor({ state: "detached" });
      await navigate(page, "Startup regression");
      await page.locator('[data-block-id="startup-block"]').first().waitFor();
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS deferred 3D graph renders, starts/stops flight and returns to the editor");
    }

    {
      const { page } = await newPage(browser);
      let failed = false;
      await page.route(/\/src\/components\/AllPages\.svelte(?:\?|$)/, async (route) => {
        if (!failed) { failed = true; await route.abort(); }
        else await route.continue();
      });
      await page.goto(BASE_URL, { waitUntil: "networkidle" });
      await navigate(page, "__all_pages__");
      await page.getByRole("alert").filter({ hasText: "Could not load all pages:" }).waitFor();
      // Browsers cache failed module imports for the lifetime of the document.
      await page.getByRole("button", { name: "Reload app", exact: true }).click();
      await page.locator(".all-pages").waitFor();
      await page.close();
      console.log("PASS a failed optional-screen import is visible and can be retried");
    }
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
