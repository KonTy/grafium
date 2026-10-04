// The title-bar bell opens the Jobs page and shows what finished. Synthetic IPC
// jobs only; no personal graph, settings or job history is touched.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

const finishedAt = (seconds) => Date.parse(`2026-09-13T12:00:${String(seconds).padStart(2, "0")}`);

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser);
    const update = (job) => page.evaluate((job) => {
      window.__selectionState.emit("job://update", {
        kind: "library_index", progress: null, message: null, link: null, error: null, details: null,
        cancellable: false, started_at: Date.parse("2026-09-13T11:59:00"), finished_at: null, ...job,
      });
    }, job);
    const bell = page.locator(".titlebar button.job-toggle");
    const badge = page.locator(".titlebar .job-badge");
    const view = page.locator(".jobs-view");

    assert.equal(await bell.count(), 0, "no bell before there is a job");
    await update({ id: "index", title: "Synthetic Library indexing", status: "running" });
    await bell.waitFor();
    assert.equal(await bell.getAttribute("aria-label"), "Jobs: 1 running");
    await update({ id: "index", title: "Synthetic Library indexing", status: "succeeded",
      message: "Indexed 3 books", finished_at: finishedAt(5) });
    await update({ id: "import", title: "Synthetic book import", status: "failed",
      error: "Synthetic import failure", finished_at: finishedAt(7) });
    await update({ id: "stopped", title: "Synthetic transcription", status: "cancelled",
      message: "Cancelled", finished_at: finishedAt(9) });
    await page.waitForFunction(() => document.querySelector(".titlebar .job-badge")?.textContent === "2");
    assert.equal(await bell.getAttribute("aria-label"), "Jobs: 2 new results, 1 failed");

    // Nothing covers the bell, and its whole square title-bar slot takes the click.
    const box = await bell.boundingBox();
    for (const [x, y] of [[box.width / 2, box.height / 2], [2, 2], [box.width - 2, box.height - 2]]) {
      const topmost = await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest(".job-toggle") !== null,
        [box.x + x, box.y + y]);
      assert.ok(topmost, `the bell is topmost at ${x},${y}`);
    }

    await bell.click({ position: { x: 2, y: 2 } });
    await view.waitFor();
    assert.equal(await page.locator(".job-panel, .job-activity [role=dialog]").count(), 0, "no hidden pop-up");
    const completed = page.getByRole("region", { name: "Completed" });
    const failed = page.getByRole("region", { name: "Failed" });
    const cancelled = page.getByRole("region", { name: "Cancelled" });
    await completed.getByRole("heading", { name: "Synthetic Library indexing" }).waitFor();
    await completed.getByText("Indexed 3 books").waitFor();
    await failed.getByRole("heading", { name: "Synthetic book import" }).waitFor();
    await cancelled.getByRole("heading", { name: "Synthetic transcription" }).waitFor();
    assert.deepEqual(await page.locator(".job-section > h2").allTextContents(), ["Failed", "Completed", "Cancelled"]);
    assert.deepEqual(await page.locator(".job-card:has(.new-label) h3").allTextContents(),
      ["Synthetic book import", "Synthetic Library indexing"], "the announced results are marked New");
    assert.equal(await page.locator(".summary-card", { hasText: "Completed" }).locator(".summary-count").textContent(), "1");

    // Opening Jobs clears the number but keeps the bell, and remembers it.
    await badge.waitFor({ state: "detached" });
    assert.equal(await bell.getAttribute("aria-label"), "Jobs");
    assert.equal(await page.evaluate(() => localStorage.getItem("grafium.jobs.seenUntil")), String(finishedAt(9)));

    // A result that lands while Jobs is open is shown as New and not announced.
    await update({ id: "later", title: "Synthetic second import", status: "succeeded", finished_at: finishedAt(20) });
    await completed.getByRole("heading", { name: "Synthetic second import" }).waitFor();
    assert.equal(await completed.locator(".job-card").first().locator("h3").textContent(), "Synthetic second import");
    assert.equal(await completed.locator(".job-card").first().locator(".new-label").count(), 1);
    await page.waitForFunction((until) => localStorage.getItem("grafium.jobs.seenUntil") === String(until), finishedAt(20));
    assert.equal(await badge.count(), 0);

    // Back leaves Jobs; the next visit has nothing new until another job finishes.
    await page.getByRole("button", { name: "Back", exact: true }).click();
    await view.waitFor({ state: "detached" });
    await bell.click();
    await view.waitFor();
    assert.equal(await page.locator(".new-label").count(), 0);
    await page.getByRole("button", { name: "Back", exact: true }).click();
    await view.waitFor({ state: "detached" });
    await update({ id: "last", title: "Synthetic third import", status: "succeeded", finished_at: finishedAt(30) });
    await page.waitForFunction(() => document.querySelector(".titlebar .job-badge")?.textContent === "1");
    assert.equal(await bell.getAttribute("aria-label"), "Jobs: 1 new result");

    assert.deepEqual(errors, []);
    console.log("PASS jobs bell: unobstructed click opens Jobs, completed/failed/cancelled groups, New results and clearing");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
