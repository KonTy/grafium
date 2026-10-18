// The Tasks activity view has two heatmaps that differ only by colour: task
// completions (green) and note edits (blue). Each carries a title and a
// Less/More key, but two layout bugs hid them:
//  - wide windows: the title and key sat inside the horizontal scroller, and the
//    grid opens scrolled to the latest week, so on any long history they were
//    pushed off the left edge;
//  - phones and narrow windows: the stacked layout sized the heatmap to its full
//    multi-year width, so the page itself scrolled sideways, opened on the oldest
//    weeks and left the key off the right edge.
// This checks each heatmap's title and key stay on screen next to its grid, and
// that only the grid scrolls.
const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";
const SHOTS = process.env.UI_TEST_SHOTS;

async function fixture(browser, { width, firstActivity }) {
  const page = await browser.newPage({ viewport: { width, height: 900 } });
  page.setDefaultTimeout(10_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.clock.setFixedTime(new Date("2026-09-13T12:00:00"));
  await page.addInitScript(({ firstActivity }) => {
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "page", title: "Fixture" }));
    const note = {
      id: "fixture-page", title: "Fixture", properties: {}, is_journal: false,
      file_path: "pages/Fixture.md", created_at: 0, updated_at: 0,
    };
    // Deterministic, sparse daily counts from `firstActivity` to yesterday.
    const counts = (seed) => {
      const out = [];
      let state = seed;
      const end = new Date("2026-09-12T00:00:00");
      for (let day = new Date(`${firstActivity}T00:00:00`); day <= end; day.setDate(day.getDate() + 1)) {
        state = (state * 1103515245 + 12345) & 0x7fffffff;
        const count = state % 4 === 0 ? 0 : (state >> 8) % 9;
        if (count > 0) {
          const key = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}`;
          out.push([key, count]);
        }
      }
      return out;
    };
    let sequence = 0;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        switch (cmd) {
          case "get_app_theme": return "github";
          case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
          case "get_graph_info": return { name: "Heatmap fixture", path: "/synthetic/heatmaps" };
          case "get_page":
            if (args.id === note.id || args.title === note.title) return note;
            throw new Error("Page not found");
          case "get_parent_page": return null;
          case "list_blocks": return [];
          case "get_page_source": return "- Fixture\n";
          case "list_open_task_rows": return [{
            block_id: "task-1", content: "TODO Open task", page_title: "Fixture",
            state: "TODO", priority: null, scheduled_date: null, scheduled_time: null,
            deadline_date: null, created_at: 0, updated_at: 0,
          }];
          case "task_flow_stats": return {
            throughput_7d: 0, throughput_prev_7d: 0, weekly_completions: [], median_cycle_ms: null,
            median_wait_ms: null, on_time_rate: null, oldest_open_days: null, open_count: 1,
            done_count: 0, by_page: [],
          };
          case "get_completion_counts": return counts(7);
          case "get_note_edit_counts": return counts(31);
          case "get_completed_tasks": return [];
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
  }, { firstActivity });
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.locator(".main-content").waitFor({ state: "attached" });
  return { page, errors };
}

const navigate = (page, title) => page.evaluate((title) =>
  window.dispatchEvent(new CustomEvent("navigate-page", { detail: title })), title);

// `isVisible` ignores scroll clipping, so hit-test the element's centre instead:
// something scrolled out of an overflow box is not what sits at that point.
const onScreen = (locator) => locator.evaluate((el) => {
  const box = el.getBoundingClientRect();
  if (box.width === 0 || box.height === 0) return false;
  const x = box.left + box.width / 2;
  const y = box.top + box.height / 2;
  if (x < 0 || y < 0 || x > innerWidth || y > innerHeight) return false;
  const hit = document.elementFromPoint(x, y);
  return hit === el || el.contains(hit);
});

async function check(browser, { name, width, firstActivity, scrolls }) {
  const { page, errors } = await fixture(browser, { width, firstActivity });
  try {
    await navigate(page, "__statistics__");
    await page.locator(".task-item.open").first().waitFor();
    await page.getByRole("button", { name: /Show activity/ }).click();
    const panels = page.locator(".activity-heatmap-panel");
    await panels.nth(1).waitFor();
    assert.equal(await panels.count(), 2, `${name}: two heatmap panels`);
    // The jump to the latest week is scheduled a frame after render. Wait until
    // every scroll position has held still for a few frames instead of guessing.
    await page.evaluate(() => new Promise((resolve) => {
      let last = "";
      let still = 0;
      const sample = () => {
        const now = [...document.querySelectorAll(".statistics-view, .activity-heatmap-panel, .heatmap-scroll")]
          .map((el) => el.scrollLeft).join(",");
        still = now === last ? still + 1 : 0;
        last = now;
        if (still >= 3) resolve(); else requestAnimationFrame(sample);
      };
      sample();
    }));

    // Stacked layouts used to size the heatmap to its full multi-year width, so
    // the whole page scrolled sideways and opened on the oldest weeks.
    const view = await page.locator(".statistics-view").evaluate((el) => ({
      scrollWidth: el.scrollWidth, clientWidth: el.clientWidth,
    }));
    assert.ok(
      view.scrollWidth <= view.clientWidth + 1,
      `${name}: the page does not scroll sideways (${view.scrollWidth}px content in ${view.clientWidth}px)`,
    );

    const labels = [
      ["Task completions", panels.nth(0)],
      ["Note edits", panels.nth(1)],
    ];
    for (const [title, panel] of labels) {
      const heading = panel.getByRole("heading", { name: title });
      assert.ok(await onScreen(heading), `${name}: "${title}" is on screen beside its grid`);
      assert.ok(await onScreen(panel.getByText("Less", { exact: true })), `${name}: "${title}" key shows Less`);
      assert.ok(await onScreen(panel.getByText("More", { exact: true })), `${name}: "${title}" key shows More`);
      assert.ok(await onScreen(panel.locator(".heatmap-cell").last()), `${name}: "${title}" shows the latest day`);
      assert.equal(
        await panel.getAttribute("aria-labelledby"),
        await heading.getAttribute("id"),
        `${name}: "${title}" names its group`,
      );
    }

    const scrollers = page.locator(".activity-heatmap-panel .heatmap-scroll");
    assert.equal(await scrollers.count(), 2, `${name}: each heatmap scrolls inside its own wrapper`);
    for (const index of [0, 1]) {
      const metrics = await scrollers.nth(index).evaluate((el) => ({
        overflowing: el.scrollWidth > el.clientWidth + 1,
        atLatest: Math.abs(el.scrollLeft + el.clientWidth - el.scrollWidth) <= 2,
      }));
      assert.equal(metrics.overflowing, scrolls, `${name}: heatmap ${index} ${scrolls ? "overflows" : "fits"}`);
      if (scrolls) assert.ok(metrics.atLatest, `${name}: heatmap ${index} opens on the latest week`);
    }

    if (SHOTS) {
      fs.mkdirSync(SHOTS, { recursive: true });
      await page.screenshot({ path: `${SHOTS}/${name.replace(/\W+/g, "-")}.png` });
    }
    assert.deepEqual(errors, [], `${name}: no page errors`);
    console.log(`PASS ${name}: both heatmaps keep their title and key on screen`);
  } finally {
    await page.close();
  }
}

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    await check(browser, { name: "phone, years of history", width: 390, firstActivity: "2024-03-10", scrolls: true });
    await check(browser, { name: "narrow window, years of history", width: 800, firstActivity: "2024-03-10", scrolls: true });
    await check(browser, { name: "wide window, years of history", width: 1600, firstActivity: "2024-03-10", scrolls: true });
    await check(browser, { name: "wide window, recent history", width: 1600, firstActivity: "2026-08-20", scrolls: false });
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
