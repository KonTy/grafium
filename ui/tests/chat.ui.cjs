// Global Chat keeps its focus/layout/history/privacy regressions after unification.
// The removed local/internet preference persistence tests are replaced by explicit
// context/mode routing and fresh-session defaults; no obsolete preference API is needed.
const {
  assert, panel, input, context, mode, button, composer, send, finish, runCases, frames,
} = require("./assistantFixture.cjs");

async function leaveChat(page) {
  await page.getByRole("button", { name: "All Pages", exact: true }).first().click();
  await page.locator(".chat-view").waitFor({ state: "hidden" });
  const filter = page.getByPlaceholder("New page title...");
  await filter.fill("Focus stays outside Chat");
  return filter;
}
async function returnToChat(page) {
  await page.keyboard.press("Alt+c");
  await input(page).waitFor();
  await frames(page);
}
async function focused(page) {
  await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "Message");
}
const metrics = (page) => composer(page).evaluate((node) => {
  const input = node.querySelector("textarea");
  const pane = node.closest(".assistant-conversation");
  const style = getComputedStyle(pane);
  const bounds = node.getBoundingClientRect();
  return {
    height: bounds.height, bottom: bounds.bottom,
    limit: (pane.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom)) / 2,
    inputHeight: input.clientHeight, scrollHeight: input.scrollHeight, overflow: getComputedStyle(input).overflowY,
    transcriptHeight: pane.querySelector(".chat-log").clientHeight,
    contained: [...node.querySelectorAll("textarea, select, label, button")].every((control) => {
      const rect = control.getBoundingClientRect();
      return rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1
        && rect.top >= bounds.top - 1 && rect.bottom <= bounds.bottom + 1;
    }),
    unobscured: [...node.querySelectorAll("select, button")].every((control) => {
      const rect = control.getBoundingClientRect();
      return control.contains(document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2));
    }),
  };
});

const cases = [
  ["Global Chat defaults to No notes and Answer without automatic web from research wording", { global: true }, async (page) => {
    await focused(page);
    assert.equal(await context(page).inputValue(), "none");
    assert.equal(await mode(page).inputValue(), "answer");
    assert.equal(await composer(page).getByRole("combobox").count(), 2, "one context and one mode selector");
    assert.equal(await panel(page).getByRole("checkbox", { name: /Internet|Research/ }).count(), 0);
    assert.equal(await button(page, "Send").isDisabled(), true);
    let call = await send(page, "Research and verify this claim; search the internet for test results");
    assert.equal(call.cmd, "assistant_chat");
    assert.deepEqual(call.args.context, { kind: "none" });
    assert.equal(call.args.mode, "answer", "wording must not enable web behind the user's back");
    assert.deepEqual(call.args.history, []);
    assert.equal(await panel(page).locator(".web-source-chip").count(), 0);
    await context(page).selectOption("graph");
    call = await send(page, "Use my graph for this comparison");
    assert.deepEqual(call.args.context, { kind: "graph" });
    assert.equal(call.args.history.length, 2);
    await context(page).selectOption("none");
    call = await send(page, "Continue without any note context");
    assert.deepEqual(call.args.context, { kind: "none" });
    assert.deepEqual(call.args.history, [
      { role: "user", content: "Research and verify this claim; search the internet for test results" },
      { role: "assistant", content: "Synthetic scoped answer." },
    ], "No notes retains safe conversation but removes all note-backed turns");
    await page.reload({ waitUntil: "networkidle" });
    await returnToChat(page);
    assert.equal(await context(page).inputValue(), "none");
    assert.equal(await mode(page).inputValue(), "answer");
    call = await send(page, "A fresh graph session");
    assert.deepEqual(call.args.history, []);
  }],
  ["Spark API endpoint offers all three Grafium modes without vendor tools or exposed keys", { global: true }, async (page) => {
    await panel(page).getByRole("button", { name: /Model server \/ API endpoint/ }).waitFor();
    assert.ok((await panel(page).innerText()).includes("spark.lan:8000"));
    assert.equal(await panel(page).getByRole("button", { name: /Cloud service/ }).count(), 0);
    assert.deepEqual(await mode(page).locator("option").evaluateAll((options) =>
      options.map(({ value, disabled }) => ({ value, disabled }))), [
      { value: "answer", disabled: false }, { value: "web", disabled: false }, { value: "deep", disabled: false },
    ]);
    await panel(page).getByText("Model & web privacy", { exact: true }).click();
    await panel(page).getByText(/Grafium contacts search engines and websites/).waitFor();
    for (const selected of ["answer", "web", "deep"]) {
      await mode(page).selectOption(selected);
      await mode(page).focus();
      await frames(page);
      assert.equal(await mode(page).evaluate((node) => node === document.activeElement), true,
        "changing a controller must not steal keyboard focus");
      const call = await send(page, `A synthetic ${selected} question`);
      assert.equal(call.cmd, "assistant_chat", "Grafium orchestration is provider-independent");
      assert.equal(call.args.mode, selected);
      assert.deepEqual(call.args.context, { kind: "none" });
      assert.equal(Object.hasOwn(call.args, "tools"), false);
      assert.equal(Object.hasOwn(call.args, "webMode"), false);
      assert.equal(JSON.stringify(call.args).includes("DO_NOT_RENDER"), false);
    }
    assert.equal((await page.locator("body").innerHTML()).includes("DO_NOT_RENDER"), false, "API keys are never rendered, including attributes");
    assert.equal(await panel(page).locator(".web-source-chip").count(), 2);
  }],
  ["Chat bordered composer grows shrinks and supports Enter Shift Enter and keyboard controls", { global: true }, async (page) => {
    const initial = await metrics(page);
    await input(page).fill(Array.from({ length: 9 }, (_, i) => `Draft line ${i + 1}`).join("\n"));
    await frames(page);
    const expanded = await metrics(page);
    assert.ok(expanded.height > initial.height + 60, "multiline drafts grow upward");
    assert.ok(Math.abs(expanded.bottom - initial.bottom) <= 1, "composer stays anchored at the bottom");
    assert.ok(expanded.height <= expanded.limit + 1);
    await input(page).fill("");
    await frames(page);
    assert.ok((await metrics(page)).height <= initial.height + 1, "deleting a draft shrinks the composer");
    await input(page).fill("First line");
    await input(page).press("Shift+Enter");
    await page.keyboard.type("Second line");
    assert.equal(await input(page).inputValue(), "First line\nSecond line");
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 0);
    await input(page).press("Enter");
    await page.waitForFunction(() => window.__assistantFixture.requests.length === 1);
    await button(page, "Send").waitFor();
    await focused(page);
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests[0].args.question), "First line\nSecond line");
    assert.equal(await input(page).inputValue(), "");
    await frames(page);
    assert.ok((await metrics(page)).height <= initial.height + 1, "sending resets the composer height");
    await input(page).fill("Keyboard-accessible controls");
    await input(page).press("Tab");
    assert.equal(await context(page).evaluate((node) => node === document.activeElement), true);
    await context(page).press("Tab");
    assert.equal(await mode(page).evaluate((node) => node === document.activeElement), true);
    await mode(page).press("Tab");
    assert.equal(await button(page, "Send").evaluate((node) => node === document.activeElement), true);
  }],
  ["Chat caps the whole composer at half the pane across themes sizes and soft wrapping", { global: true }, async (page) => {
    if (await page.locator(".sidebar").isVisible()) {
      await page.locator(".sidebar button").first().focus();
      await page.keyboard.press("Control+b");
      await page.locator(".sidebar").waitFor({ state: "hidden" });
    }
    for (const [width, height, theme] of [[1200, 900, "github-dark"], [420, 620, "github"], [760, 480, "github-dark"]]) {
      await page.setViewportSize({ width, height });
      await page.evaluate(async (id) => {
        const { applyTheme, getThemeById } = await import("/src/lib/themes.ts");
        applyTheme(getThemeById(id).colors);
      }, theme);
      await input(page).fill(Array.from({ length: 100 }, (_, i) => `Line ${i + 1}: a long draft that wraps as the pane narrows.`).join("\n"));
      await frames(page);
      let size = await metrics(page);
      assert.ok(size.height <= size.limit + 1, `${width}x${height}: whole composer is capped at half the pane: ${JSON.stringify(size)}`);
      assert.ok(size.height >= size.limit - 2, "a long draft reaches the half-pane cap");
      assert.equal(size.overflow, "auto");
      assert.ok(size.scrollHeight > size.inputHeight);
      assert.ok(size.transcriptHeight > 60, "transcript remains visible");
      assert.equal(size.contained, true);
      assert.equal(size.unobscured, true, "bottom navigation must not cover controls");
      const draft = await input(page).inputValue();
      await page.setViewportSize({ width: Math.max(320, width - 160), height: height - 100 });
      await frames(page);
      const resized = await metrics(page);
      assert.ok(resized.height <= resized.limit + 1, "resizing recalculates the cap without typing");
      assert.ok(resized.height >= resized.limit - 2);
      assert.equal(resized.contained && resized.unobscured, true,
        `resized ${Math.max(320, width - 160)}x${height - 100}: ${JSON.stringify(resized)}`);
      assert.equal(await input(page).inputValue(), draft, "resizing never changes draft text");
      await page.setViewportSize({ width, height });
      await input(page).fill("A long wrapping line. ".repeat(200));
      await frames(page);
      size = await metrics(page);
      assert.ok(size.height <= size.limit + 1, "soft wrapping respects the same cap");
      await input(page).fill("Short again");
      await frames(page);
      assert.ok((await metrics(page)).height < size.height);
      const colors = await input(page).evaluate((node) => {
        const style = getComputedStyle(node);
        return { foreground: style.color, background: getComputedStyle(node.closest("form")).backgroundColor };
      });
      assert.notEqual(colors.foreground, colors.background, "composer remains readable in light and dark themes");
    }
  }],
  ["Chat completed answers remain selectable and copy Markdown with graph and web sources", { global: true }, async (page) => {
    await context(page).selectOption("graph");
    await mode(page).selectOption("web");
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "Give a copyable answer", true);
    await finish(page, call.args.requestId, "**Cobalt dial** activates the lantern.");
    const answer = panel(page).locator(".msg:not(.user)").first();
    await answer.getByRole("button", { name: "Copy", exact: true }).click();
    const copied = await page.evaluate(() => window.__selectionState.clipboard.at(-1));
    assert.ok(copied.startsWith("**Cobalt dial** activates the lantern."));
    assert.match(copied, /\*\*Sources\*\*/);
    assert.match(copied, /Synthetic source/);
    assert.match(copied, /https:\/\/example.org\/evidence/);
    const next = await send(page, "Continue while I select the earlier answer", true);
    await answer.locator(".msg-content").evaluate((node) => {
      const range = document.createRange();
      range.selectNodeContents(node);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      document.dispatchEvent(new Event("selectionchange"));
    });
    const selected = await page.evaluate(() => window.getSelection().toString());
    assert.equal(selected, "Cobalt dial activates the lantern.");
    await page.evaluate((id) => window.__assistantFixture.emit("ai://chat_stream", {
      request_id: id, delta: "A later streaming answer.", done: false,
    }), next.args.requestId);
    await frames(page);
    assert.equal(await page.evaluate(() => window.getSelection().toString()), selected,
      "streaming another answer must not replace the selected completed response");
    await finish(page, next.args.requestId, "");
  }],
  ["Chat completed conversation draft height and follow-up history survive navigation", { global: true }, async (page) => {
    await send(page, "Remember this conversation");
    const draft = "Unsent draft\nSecond line\nThird line";
    await input(page).fill(draft);
    await frames(page);
    const draftHeight = (await metrics(page)).height;
    await leaveChat(page);
    await returnToChat(page);
    assert.equal(await input(page).inputValue(), draft);
    assert.ok(Math.abs((await metrics(page)).height - draftHeight) <= 1);
    assert.equal(await panel(page).locator(".msg").count(), 2);
    const next = await send(page, "Follow up after navigating");
    assert.deepEqual(next.args.history, [
      { role: "user", content: "Remember this conversation" },
      { role: "assistant", content: "Synthetic scoped answer." },
    ]);
  }],
  ["Chat background errors New conversation and Stop after returning preserve lifecycle safety", { global: true }, async (page) => {
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "Report errors even while away", true);
    const filter = await leaveChat(page);
    await finish(page, call.args.requestId, { error: "Synthetic provider failure", done: true });
    assert.equal(await filter.evaluate((node) => node === document.activeElement), true);
    await returnToChat(page);
    await panel(page).getByRole("alert").filter({ hasText: "Synthetic provider failure" }).waitFor();
    await button(page, "New conversation").click();
    assert.equal(await panel(page).locator(".msg").count(), 0);
    assert.equal(await panel(page).getByRole("alert").count(), 0);
    const next = await send(page, "Stop after returning", true);
    await leaveChat(page);
    await returnToChat(page);
    assert.equal(await context(page).isDisabled(), true);
    assert.equal(await mode(page).isDisabled(), true);
    assert.equal(await input(page).isDisabled(), true);
    assert.equal(await button(page, "Stop").count(), 1);
    await button(page, "Stop").click();
    await page.waitForFunction(() => window.__assistantFixture.cancellations.length === 1
      && window.__assistantFixture.listenerCount() === 0);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), [next.args.requestId]);
    await focused(page);
    assert.equal(await input(page).isEnabled(), true);
  }],
  ["Chat provider refresh retains drafts and index diagnostics remain actionable", { global: true }, async (page) => {
    const draft = "Keep my draft when connection changes";
    await input(page).fill(draft);
    const checks = await page.evaluate(() => window.__assistantFixture.healthChecks);
    await leaveChat(page);
    await page.evaluate(() => { window.__assistantFixture.connected = false; });
    await returnToChat(page);
    await panel(page).getByText(/Connect a model to send questions/).waitFor();
    assert.ok(await page.evaluate(() => window.__assistantFixture.healthChecks) > checks);
    assert.equal(await input(page).inputValue(), draft);
    assert.equal(await button(page, "Send").isDisabled(), true);
    await page.evaluate(() => {
      window.__assistantFixture.connected = true;
      window.__assistantFixture.config.cloud.llm_model = "Refreshed Spark model";
      window.dispatchEvent(new Event("ai-configuration-changed"));
    });
    await panel(page).getByRole("button", { name: /Refreshed Spark model/ }).waitFor();
    assert.equal(await input(page).inputValue(), draft);
    assert.equal(await button(page, "Send").isEnabled(), true);
    await panel(page).getByText("Model & index status", { exact: true }).click();
    await panel(page).getByText(/No semantic index yet/).waitFor();
    assert.equal(await button(page, "Index now").isDisabled(), true);
    await page.evaluate(() => {
      window.__assistantFixture.index.embedder_ready = true;
      window.__assistantFixture.index.accelerator = { gpu_supported: true, on_gpu: false };
      window.__assistantFixture.emit("ai-index-updated", {});
    });
    await button(page, "Retry on GPU").waitFor();
    await button(page, "Index now").click();
    await panel(page).getByText("Indexed 2 pages; 0 failed.", { exact: true }).waitFor();
    await panel(page).getByText(/12 indexed chunks/).waitFor();
    await button(page, "Retry on GPU").click();
    await page.waitForFunction(() => window.__assistantFixture.gpuRetries === 1);
    assert.equal(await page.evaluate(() => window.__assistantFixture.config.cloud.llm_model), "Refreshed Spark model",
      "GPU retry must not switch the configured model");
    await page.evaluate(() => {
      window.__assistantFixture.indexFailure = true;
      window.__assistantFixture.emit("ai-index-updated", {});
    });
    await panel(page).getByRole("alert").filter({ hasText: "Synthetic index status failure" }).waitFor();
    await page.evaluate(() => { window.__assistantFixture.indexFailure = false; });
    await button(page, "Retry status").click();
    await button(page, "Retry status").waitFor({ state: "hidden" });
    assert.equal(await input(page).inputValue(), draft);
  }],
  ["Chat graph changes cancel old work and isolate all previous context", { global: true }, async (page) => {
    await context(page).selectOption("graph");
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const call = await send(page, "This context belongs only to the old graph", true);
    await page.locator(".graph-selector").click();
    await page.getByRole("button", { name: "Other test graph", exact: true }).click();
    await page.waitForFunction(() => window.__assistantFixture.cancellations.length === 1
      && window.__assistantFixture.listenerCount() === 0);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), [call.args.requestId]);
    await returnToChat(page);
    assert.equal(await panel(page).locator(".msg").count(), 0);
    assert.equal(await input(page).inputValue(), "");
    assert.equal(await context(page).inputValue(), "none");
    await page.evaluate(() => { window.__assistantFixture.hold = false; });
    const next = await send(page, "A new graph conversation");
    assert.deepEqual(next.args.history, []);
    assert.equal(next.args.graphPath, "/synthetic/assistant-other");
  }],
];

for (const selected of ["answer", "web", "deep"]) cases.push([
  `Chat ${selected} run finishes while hidden without focus theft duplicate requests or lost sources`,
  { global: true }, async (page) => {
    await context(page).selectOption("graph");
    await mode(page).selectOption(selected);
    await page.evaluate(() => { window.__assistantFixture.hold = true; });
    const question = `Continue ${selected} in the background`;
    const call = await send(page, question, true);
    let filter = await leaveChat(page);
    await page.evaluate((requestId) => {
      window.__assistantFixture.emit("ai://chat_stream", { request_id: requestId, phase: "thinking", delta: "Background ", done: false });
    }, call.args.requestId);
    await frames(page);
    assert.equal(await filter.evaluate((node) => node === document.activeElement), true);
    await returnToChat(page);
    await button(page, "Stop").waitFor();
    assert.equal(await panel(page).locator(".msg").last().locator(".msg-content").innerText(), "Background ");
    assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 1);
    assert.equal(await input(page).isDisabled(), true, "returning cannot submit a second concurrent request");
    assert.equal(await context(page).isDisabled(), true);
    assert.equal(await mode(page).isDisabled(), true);
    filter = await leaveChat(page);
    await finish(page, call.args.requestId, `${selected} answer.`);
    assert.equal(await filter.evaluate((node) => node === document.activeElement), true);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.cancellations), []);
    await returnToChat(page);
    await button(page, "Send").waitFor();
    assert.equal(await panel(page).locator(".msg").last().locator(".msg-content").innerText(), `Background ${selected} answer.`);
    assert.equal(await panel(page).locator(".source-title").filter({ hasText: "Synthetic source" }).count(), 1);
    assert.equal(await panel(page).locator(".web-source-chip").count(), selected === "answer" ? 0 : 1);
    assert.equal(await panel(page).locator(".chat-log").evaluate((node) =>
      Math.abs(node.scrollHeight - node.clientHeight - node.scrollTop) <= 2), true);
    await page.evaluate(() => { window.__assistantFixture.hold = false; });
    const followup = await send(page, `Follow up on ${selected}`);
    assert.deepEqual(followup.args.history, [
      { role: "user", content: question }, { role: "assistant", content: `Background ${selected} answer.` },
    ]);
  },
]);

if (require.main === module) runCases(cases).catch((error) => { console.error(error); process.exitCode = 1; });
