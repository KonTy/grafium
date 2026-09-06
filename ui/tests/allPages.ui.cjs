// Drives the real All Pages UI in a browser with Tauri IPC stubbed.
//
// This exists because the bug that shipped last — an infinite $effect loop that
// froze every control in the view — was invisible to unit tests and to
// svelte-check. It only appears when the components run together in a browser.
//
// Fixtures deliberately make alphabetical and recency order DISAGREE. An
// earlier version of this test had them agreeing, so both sorts looked
// identical and it passed against a broken build.
const { chromium } = require("playwright");

const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function launchChromium() {
  try {
    return await chromium.launch({ args: ["--no-sandbox"] });
  } catch (cause) {
    throw new Error(
      "could not start Chromium — install it once with `npx playwright install chromium`",
      { cause },
    );
  }
}

const now = Date.now();

const node = (key, label, page_id, children, updated_at) => ({
  key, label, page_id, children, descendant_count: 1 + children.length, updated_at,
});

//  name order : mybooks, tech      | absorption, zebra
//  date order : tech,    mybooks   | zebra,      absorption
const NAMESPACE_TREE = [
  node("mybooks", "mybooks", null,
    [node("mybooks/coolbook", "coolbook", null,
      [node("mybooks/coolbook/toc", "toc", "p4", [], now - 90000),
       node("mybooks/coolbook/arc", "arc", "p5", [], now - 95000)], now - 90000)], now - 90000),
  node("tech", "tech", null, [node("tech/linux", "linux", "p3", [], now - 1000)], now - 1000),
  node("absorption", "absorption", "p2", [], now - 80000),
  node("zebra", "zebra", "p1", [], now - 2000),
];

const TAG_TREE = [node("todo", "todo", "p9", [], now - 5000)];

const PAGES = [
  { id: "p1", title: "zebra", updated_at: now - 2000, is_journal: false },
  { id: "p2", title: "absorption", updated_at: now - 80000, is_journal: false },
  { id: "p3", title: "tech/linux", updated_at: now - 1000, is_journal: false },
  { id: "p4", title: "mybooks/coolbook/toc", updated_at: now - 90000, is_journal: false },
];

const EXPECT = {
  "A–Z": ["mybooks", "tech", "absorption", "zebra"],
  Recent: ["tech", "mybooks", "zebra", "absorption"],
};

(async () => {
  const browser = await launchChromium();
  const page = await browser.newPage({ viewport: { width: 2400, height: 1300 } });

  await page.addInitScript(
    ({ pages, nsTree, tagTree }) => {
      window.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { windowLabel: "main", label: "main" },
        },
        plugins: {},
        transformCallback: (cb) => { const id = Math.random(); window[`_cb${id}`] = cb; return id; },
        invoke: async (cmd, args) => {
          switch (cmd) {
            case "count_pages": return pages.length;
            case "list_pages_window": {
              const sorted = [...pages].sort((a, b) =>
                args.sortByTitle ? a.title.localeCompare(b.title) : b.updated_at - a.updated_at);
              return sorted.slice(args.offset, args.offset + args.limit);
            }
            case "pages_namespace_tree": return nsTree;
            case "pages_tag_tree": return tagTree;
            case "get_graph_info": return { path: "/tmp/test-graph", name: "Test" };
            case "get_app_theme": return "dark";
            case "plugin:event|listen": return 0;
            // Anything else the app asks for during boot. Returning `[]`
            // rather than `null` because these commands are typed as lists;
            // `null` only crashed the sidebar because the stub was lying.
            default: return [];
          }
        },
      };
    },
    { pages: PAGES, nsTree: NAMESPACE_TREE, tagTree: TAG_TREE },
  );

  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e).split("\n")[0]));
  page.on("console", (m) => {
    if (m.type() !== "error") return;
    const t = m.text();
    // The stub does not implement everything the sidebar needs.
    if (t.includes("Sidebar") || t.includes("404")) return;
    errors.push(t);
  });

  const failures = [];
  const check = (name, actual, expected) => {
    const ok = JSON.stringify(actual) === JSON.stringify(expected);
    if (!ok) failures.push(`${name}\n      got:      ${JSON.stringify(actual)}\n      expected: ${JSON.stringify(expected)}`);
    console.log(`  ${ok ? "PASS" : "FAIL"}  ${name}`);
  };

  const rootLabels = () =>
    page.locator(".tree > .tree-group > .tree-row .node-label").allTextContents();
  const click = async (label) => {
    await page.locator(`button:text-is("${label}")`).first().click();
    await page.waitForTimeout(700);
  };

  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.waitForTimeout(1200);
  await page.locator("text=All Pages").first().click();
  await page.waitForTimeout(1200);

  console.log("\nsort");
  for (const mode of ["A–Z", "Recent", "A–Z", "Recent"]) {
    await click(mode);
    check(`${mode} order`, await rootLabels(), EXPECT[mode]);
  }

  console.log("\ntree source");
  await click("Tags");
  check("Tags switches the tree", await rootLabels(), ["todo"]);
  await click("Namespace");
  check("Namespace switches back", await rootLabels(), EXPECT.Recent);

  console.log("\nfilter");
  const filter = page.locator('input[placeholder*="Filter"]').first();
  await filter.fill("linux");
  await page.waitForTimeout(700);
  const filtered = await page.locator(".tree .node-label").allTextContents();
  check("filter narrows to the match and its ancestors", filtered, ["tech", "linux"]);
  await filter.fill("");
  await page.waitForTimeout(700);
  check("clearing the filter restores the tree", await rootLabels(), EXPECT.Recent);

  console.log("\nexpand / navigate");
  await page.locator('.tree-item:has-text("mybooks")').first().click();
  await page.waitForTimeout(600);
  const afterExpand = await page.locator(".tree .node-label").allTextContents();
  check("clicking a folder expands it", afterExpand.includes("coolbook"), true);

  console.log("\nexpansion survives a filter");
  // Expanding one folder then searching for something in the *other* used to
  // persist a pruned expansion set, silently collapsing the folder you had
  // opened once the search was cleared.
  // The previous step may already have opened it, so drive it to a known
  // state rather than assuming a click expands.
  const isOpen = async () =>
    (await page.locator(".tree .node-label").allTextContents()).includes("coolbook");
  if (!(await isOpen())) {
    await page.locator('.tree-item:has-text("mybooks")').first().click();
    await page.waitForTimeout(500);
  }
  const openedBefore = await isOpen();
  await filter.fill("linux");
  await page.waitForTimeout(600);
  await filter.fill("");
  await page.waitForTimeout(600);
  const openedAfter = await isOpen();
  check("a folder opened before a search is still open after it", [openedBefore, openedAfter], [true, true]);

  console.log("\nlayout");
  const columns = await page.evaluate(() => {
    const groups = [...document.querySelectorAll(".tree-group")];
    const xs = new Set(groups.map((g) => Math.round(g.getBoundingClientRect().left)));
    const split = groups.filter((g) => {
      const rowXs = new Set([...g.querySelectorAll(".tree-row")]
        .map((r) => Math.round(r.getBoundingClientRect().left)));
      return rowXs.size > 1;
    }).length;
    return { count: xs.size, split };
  });
  check("tree flows into multiple columns", columns.count > 1, true);
  check("no branch is split across a column", columns.split, 0);

  console.log("\nremoved controls");
  check("Expand all is gone", await page.locator('button:text-is("Expand all")').count(), 0);

  console.log(`\nerrors: ${errors.length ? JSON.stringify(errors, null, 2) : "none"}`);
  if (errors.length) failures.push(`${errors.length} console/page error(s)`);

  await browser.close();
  console.log(failures.length ? `\nFAILED (${failures.length})\n  ${failures.join("\n  ")}` : "\nALL PASSED");
  process.exit(failures.length ? 1 : 0);
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
