// Former scoped Research coverage now exercises the same Chat used in full view.
const {
  assert, panel, input, context, mode, button, send, finish, runCases,
  focus, focusContinuous, row, frames,
} = require("./assistantFixture.cjs");

const navigate = (page, title) => page.evaluate((detail) =>
  window.dispatchEvent(new CustomEvent("navigate-page", { detail })), title);
const togglePanel = (page) => page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));

const cases = [
  ["Chat is the only assistant tab beside manual Notes", {}, async (page) => {
    assert.deepEqual(await page.getByRole("tablist", { name: "Reading panel tabs" })
      .getByRole("tab").allTextContents(), ["Chat", "Notes"]);
    assert.equal(await panel(page).getByRole("checkbox", { name: /Internet|Research/ }).count(), 0);
    assert.equal(await panel(page).getByRole("combobox", { name: "Mode", exact: true }).count(), 1);
    assert.deepEqual(await context(page).locator("option").evaluateAll((options) => options.map(({ value }) => value).sort()),
      ["block", "book", "graph", "none", "page", "section", "selection"]);
    assert.equal(await context(page).inputValue(), "page");
    assert.equal(await mode(page).inputValue(), "answer");
    await input(page).fill("Keep this Chat draft while taking manual notes");
    await page.getByRole("tab", { name: "Notes", exact: true }).click();
    await page.getByRole("tab", { name: "Chat", exact: true }).click();
    assert.equal(await input(page).inputValue(), "Keep this Chat draft while taking manual notes");
  }],
  ["Chat defaults to the current book and keeps long source text out of IPC", { book: true }, async (page) => {
    assert.equal(await context(page).inputValue(), "book");
    const question = "What supports this book's claims?";
    const call = await send(page, question);
    assert.deepEqual(call.args.context, { kind: "book", pageId: "selection-page" });
    assert.equal(call.args.question, question);
    assert.equal(call.args.mode, "answer");
    assert.equal(call.args.graphPath, "/synthetic/keyboard-selection");
    assert.deepEqual(call.args.history, []);
    assert.ok(JSON.stringify(call.args).length < 1500, "book content is retrieved natively, not concatenated into IPC");
    const followup = await send(page, "What does it recommend next?");
    assert.deepEqual(followup.args.history, [
      { role: "user", content: question }, { role: "assistant", content: "Synthetic scoped answer." },
    ]);
  }],
  ["Chat defaults to only the focused journal day", { journal: true }, async (page) => {
    const call = await send(page, "What did I record today?");
    assert.deepEqual(call.args.context, { kind: "page", pageId: "day-1" });
    assert.equal(await context(page).locator('option[value="page"]').textContent(), "This day");
    assert.equal(await context(page).locator('option[value="section"]').evaluate((option) => option.disabled), true);
    assert.equal(await context(page).locator('option[value="book"]').evaluate((option) => option.disabled), true);
  }],
  ["Chat rejects a rendered selection spanning journal days instead of widening it", { journal: true }, async (page) => {
    await page.evaluate(() => {
      const first = document.querySelector('.block-item[data-block-id="day-0-b14"] .rendered-content');
      const last = document.querySelector('.block-item[data-block-id="day-1-b1"] .rendered-content');
      if (!first || !last) throw new Error("Missing synthetic rendered journal blocks");
      const range = document.createRange();
      range.setStart(first, 0);
      range.setEnd(last, last.childNodes.length);
      const selected = window.getSelection();
      selected.removeAllRanges();
      selected.addRange(range);
      document.dispatchEvent(new Event("selectionchange"));
    });
    await panel(page).getByRole("alert").filter({ hasText: /within one page or journal day, not across pages/ }).waitFor();
    assert.equal(await context(page).locator('option[value="selection"]').evaluate((option) => option.disabled), true);
    assert.equal(await panel(page).locator(".selection-preview").count(), 0);
    assert.equal(await context(page).inputValue(), "page", "invalid selection does not silently broaden context");
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 0);
  }],
  ["Chat exposes explicit block section graph and no-notes contexts with one mode selector", {}, async (page) => {
    await context(page).selectOption("block");
    let call = await send(page, "Explain this block");
    assert.deepEqual(call.args.context, { kind: "block", pageId: "selection-page", blockId: "b0" });
    await context(page).selectOption("section");
    call = await send(page, "Explain this section");
    assert.deepEqual(call.args.context, { kind: "section", pageId: "selection-page", blockId: "b0" });
    await context(page).selectOption("graph");
    await mode(page).selectOption("web");
    call = await send(page, "Find evidence in my graph and on the web");
    assert.deepEqual(call.args.context, { kind: "graph" });
    assert.equal(call.args.mode, "web");
    await mode(page).selectOption("deep");
    call = await send(page, "Research the evidence");
    assert.equal(call.args.mode, "deep");
    await context(page).selectOption("none");
    await mode(page).selectOption("answer");
    call = await send(page, "Answer without my notes");
    assert.deepEqual(call.args.context, { kind: "none" });
    assert.deepEqual(call.args.history, [], "No notes must exclude prior note-backed questions and answers");
  }],
  ["Chat finishes on its original source while hidden and preserves independent A B threads and drafts", {}, async (page) => {
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const original = await send(page, "Question on source A", true);
    await navigate(page, "Other research source");
    await row(page, "other-research-block").waitFor();
    await page.waitForFunction(() => !document.querySelector(".reference-panel .assistant-conversation")?.textContent.includes("Question on source A"));
    await input(page).fill("Draft on source B");
    await finish(page, original.args.requestId, "Answer for original source only.");
    assert.equal(await input(page).inputValue(), "Draft on source B");
    assert.equal(await panel(page).getByText("Answer for original source only.", { exact: true }).count(), 0);
    await page.evaluate(() => { window.__assistantFixture.hold = false; });
    const other = await send(page, "Question on source B");
    assert.equal(other.args.context.pageId, "other-research");
    assert.deepEqual(other.args.history, []);
    await input(page).fill("Independent draft B");
    await navigate(page, "Keyboard selection");
    await row(page, "b0").waitFor();
    await panel(page).getByText("Answer for original source only.", { exact: true }).waitFor();
    await togglePanel(page);
    await togglePanel(page);
    await panel(page).getByText("Answer for original source only.", { exact: true }).waitFor();
    const followup = await send(page, "Follow up on A");
    assert.deepEqual(followup.args.history, [
      { role: "user", content: "Question on source A" },
      { role: "assistant", content: "Answer for original source only." },
    ]);
    await navigate(page, "Other research source");
    await row(page, "other-research-block").waitFor();
    assert.equal(await input(page).inputValue(), "Independent draft B");
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), []);
  }],
  ["Expand transfers the same ongoing Chat without cancellation duplication or lost history", {}, async (page) => {
    await context(page).selectOption("section");
    await mode(page).selectOption("deep");
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "Continue this source conversation after Expand", true);
    await button(page, "Expand").click();
    await page.locator(".chat-view .assistant-conversation").waitFor();
    await button(page, "Stop").waitFor();
    assert.equal(await context(page).inputValue(), "section");
    assert.equal(await mode(page).inputValue(), "deep");
    assert.equal(await context(page).isDisabled(), true);
    assert.equal(await mode(page).isDisabled(), true);
    assert.equal(await input(page).isDisabled(), true);
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 1);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), []);
    await page.keyboard.press("Alt+c");
    await page.waitForFunction(() =>
      document.querySelector('.chat-view select[aria-label="Context"]')?.value === "none");
    assert.equal(await context(page).inputValue(), "none", "ordinary Chat navigation opens the global conversation");
    assert.equal(await mode(page).inputValue(), "answer");
    assert.equal(await panel(page).locator(".msg").count(), 0, "source turns do not leak into global Chat");
    assert.equal(await input(page).isEnabled(), true, "the independent global composer is not locked by the source request");
    await page.keyboard.press("Control+[");
    await button(page, "Stop").waitFor();
    assert.equal(await context(page).inputValue(), "section", "Back restores the expanded source conversation ID");
    assert.equal(await mode(page).inputValue(), "deep");
    await panel(page).getByText(call.args.question, { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 1);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), []);
    await page.getByRole("button", { name: "All Pages", exact: true }).first().click();
    await page.locator(".chat-view").waitFor({ state: "hidden" });
    const filter = page.getByPlaceholder("New page title...");
    await filter.fill("Do not steal focus");
    await finish(page, call.args.requestId, "Expanded answer finished while hidden.");
    assert.equal(await filter.evaluate((node) => node === document.activeElement), true);
    // Full Chat's global shortcut intentionally starts from the global thread.
    // Back restores the expanded conversation's own navigation entry instead.
    await page.keyboard.press("Control+[");
    await panel(page).getByText("Expanded answer finished while hidden.", { exact: true }).waitFor();
    await page.evaluate(() => { window.__assistantFixture.hold = false; });
    const next = await send(page, "Follow up on the expanded source");
    assert.deepEqual(next.args.context, call.args.context);
    assert.equal(next.args.mode, "deep");
    assert.deepEqual(next.args.history, [
      { role: "user", content: call.args.question },
      { role: "assistant", content: "Expanded answer finished while hidden." },
    ]);
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 2);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), []);
    await navigate(page, "Keyboard selection");
    await row(page, "b0").waitFor();
    if (!await page.locator(".reference-panel").isVisible()) await togglePanel(page);
    await panel(page).getByText("Follow up on the expanded source", { exact: true }).waitFor();
  }],
  ["Chat waits for pending editor drafts and Stop cancels the captured request", { allowEditorWrites: true }, async (page) => {
    await focus(page, "b0");
    await page.evaluate(() => { window.__selectionState.holdUpdate = true; });
    await page.keyboard.press("End");
    await page.keyboard.type(" latest research draft");
    await input(page).fill("What does it now say?");
    await button(page, "Send").click();
    await page.waitForFunction(() => window.__selectionState.updateWaiters.length > 0);
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 0);
    assert.equal(await context(page).isDisabled(), true, "context is frozen even before the native request starts");
    assert.equal(await mode(page).isDisabled(), true);
    await page.evaluate(() => {
      window.__selectionState.holdUpdate = false;
      window.__selectionState.updateWaiters.splice(0).forEach((resolve) => resolve());
    });
    await page.waitForFunction(() => window.__assistantFixture.requests.length === 1);
    assert.match(await page.evaluate(() => window.__assistantFixture.requests[0].rootContent), /latest research draft/);
    await button(page, "Send").waitFor();
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "A long research run", true);
    await button(page, "Stop").click();
    await page.waitForFunction(() => window.__assistantFixture.cancellations.length === 1);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), [call.args.requestId]);
  }],
  ["Chat source and mode remain frozen while a selected request is pending", {}, async (page) => {
    await focus(page, "b0");
    await page.keyboard.press("End");
    for (let i = 0; i < 4; i++) await page.keyboard.press("Shift+ArrowLeft");
    await frames(page);
    await context(page).selectOption("selection");
    await mode(page).selectOption("web");
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "Explain only this passage", true);
    await focus(page, "b1");
    await page.keyboard.press("Control+a");
    await frames(page);
    assert.equal(await context(page).isDisabled(), true);
    assert.equal(await mode(page).isDisabled(), true);
    assert.equal(await panel(page).locator(".selection-preview").innerText(), "line");
    await finish(page, call.args.requestId);
    assert.deepEqual(call.args.context, {
      kind: "selection", pageId: "selection-page", selection: { blockIds: ["b0"], text: "line" },
    });
    assert.equal(call.args.mode, "web");
    await panel(page).locator(".answer-badges").filter({ hasText: "Selection" }).waitFor();
  }],
  ["Chat composer remains reachable in a short narrow window", {}, async (page) => {
    await page.setViewportSize({ width: 1000, height: 600 });
    await input(page).fill("Question in a compact window");
    await frames(page);
    async function assertReachable(control) {
      const geometry = await control.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const pane = element.closest(".reference-panel").getBoundingClientRect();
        return { bottom: bounds.bottom, right: bounds.right, panelBottom: pane.bottom, panelRight: pane.right, height: innerHeight,
          reachable: element.contains(document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)) };
      });
      assert.ok(geometry.bottom <= Math.min(geometry.panelBottom, geometry.height) + 1, JSON.stringify(geometry));
      assert.ok(geometry.right <= geometry.panelRight + 1, JSON.stringify(geometry));
      assert.equal(geometry.reachable, true, "all composer controls must be visible without scrolling the whole panel");
    }
    await assertReachable(button(page, "Send"));
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "A compact Chat request", true);
    await assertReachable(button(page, "Stop"));
    await assertReachable(context(page));
    await assertReachable(mode(page));
    await button(page, "Stop").click();
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), [call.args.requestId]);
  }],
];

for (const unifiedPage of [false, true]) {
  cases.push([
    `Chat preserves a selected passage when focusing its composer in ${unifiedPage ? "continuous" : "classic"} mode`,
    { unifiedPage }, async (page) => {
      if (unifiedPage) await focusContinuous(page, "selection-page", "b0");
      else await focus(page, "b0");
      await page.keyboard.press("End");
      for (let i = 0; i < 4; i++) await page.keyboard.press("Shift+ArrowLeft");
      await frames(page);
      await context(page).selectOption("selection");
      const call = await send(page, "Explain only the highlighted word");
      assert.deepEqual(call.args.context, {
        kind: "selection", pageId: "selection-page", selection: { blockIds: ["b0"], text: "line" },
      });
    },
  ], [
    `Chat preserves rendered line boundaries in ${unifiedPage ? "continuous" : "classic"} mode`,
    { unifiedPage }, async (page) => {
      await page.evaluate((unified) => {
        const selector = unified
          ? '.unified-rendered-block[data-source-block-id="b1"] .rendered-content'
          : '.block-item[data-block-id="b1"] .rendered-content';
        const content = document.querySelector(selector);
        if (!content) throw new Error("Missing synthetic multiline content");
        const range = document.createRange();
        range.selectNodeContents(content);
        const selected = window.getSelection();
        selected.removeAllRanges();
        selected.addRange(range);
        document.dispatchEvent(new Event("selectionchange"));
      }, unifiedPage);
      await frames(page);
      await context(page).selectOption("selection");
      const call = await send(page, "Explain the two selected lines");
      assert.deepEqual(call.args.context.selection.blockIds, ["b1"]);
      assert.equal(call.args.context.selection.text.trim(), "Alpha\nBeta", "rendered line breaks must not merge words");
    },
  ]);
}

if (require.main === module) runCases(cases).catch((error) => { console.error(error); process.exitCode = 1; });
