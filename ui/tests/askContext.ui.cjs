// Former Ask safety regressions: imported media stays a source, never a giant question.
const {
  assert, input, context, mode, button, send, runCases, focus, focusContinuous,
} = require("./assistantFixture.cjs");

const cases = [];
for (const unifiedPage of [false, true]) {
  cases.push([
    `Chat keeps long video content source-separated in ${unifiedPage ? "continuous" : "classic"} mode`,
    { unifiedPage, video: true }, async (page) => {
      // Source Chat now defaults to its page; the old "Current block" is explicit context.
      await context(page).selectOption("block");
      const question = "According to this video how do I activate the lantern?";
      let call = await send(page, question);
      assert.equal(call.args.question, question);
      assert.deepEqual(call.args.context, { kind: "block", pageId: "selection-page", blockId: "b0" });
      assert.deepEqual(call.args.history, []);
      assert.ok(JSON.stringify(call.args).length < 1000);
      await context(page).selectOption("page");
      call = await send(page, "What else does this video recommend?");
      assert.deepEqual(call.args.context, { kind: "page", pageId: "selection-page" });
      assert.deepEqual(call.args.history, [
        { role: "user", content: question }, { role: "assistant", content: "Synthetic scoped answer." },
      ]);
      assert.ok(JSON.stringify(call.args).length < 1000);
      await context(page).selectOption("graph");
      call = await send(page, "How does it compare with my other notes?");
      assert.deepEqual(call.args.context, { kind: "graph" });
      assert.ok(JSON.stringify(call.args).length < 1500);
      await mode(page).selectOption("web");
      call = await send(page, "Verify the lantern instructions on the web");
      assert.equal(call.args.mode, "web");
      assert.equal(call.cmd, "assistant_chat");
      assert.ok(!JSON.stringify(call.args).includes("LONG-TRANSCRIPT-MARKER"));
    },
  ], [
    `Chat waits for current video editor persistence in ${unifiedPage ? "continuous" : "classic"} mode`,
    { unifiedPage, video: true, allowEditorWrites: true }, async (page) => {
      await context(page).selectOption("block");
      if (unifiedPage) await focusContinuous(page, "selection-page", "b0");
      else await focus(page, "b0");
      await page.evaluate(() => { window.__selectionState.holdUpdate = true; });
      await page.keyboard.press("End");
      await page.keyboard.type(" fresh draft");
      await input(page).fill("What does this video block say?");
      await button(page, "Send").click();
      await page.waitForFunction(() => window.__selectionState.updateWaiters.length > 0);
      assert.equal(await page.evaluate(() => window.__assistantFixture.requests.length), 0);
      await page.evaluate(() => {
        window.__selectionState.holdUpdate = false;
        window.__selectionState.updateWaiters.splice(0).forEach((resolve) => resolve());
      });
      await page.waitForFunction(() => window.__assistantFixture.requests.length === 1);
      const call = await page.evaluate(() => window.__assistantFixture.requests[0]);
      assert.match(call.rootContent, /fresh draft/);
      assert.deepEqual(call.args.context, { kind: "block", pageId: "selection-page", blockId: "b0" });
      assert.equal(call.args.question, "What does this video block say?");
      await button(page, "Send").waitFor();
    },
  ]);
}
cases.push(["Chat video page context stays on the focused journal day", { journal: true, video: true }, async (page) => {
  await context(page).selectOption("page");
  const call = await send(page, "What did I note on this day?");
  assert.deepEqual(call.args.context, { kind: "page", pageId: "day-1" });
}]);

if (require.main === module) runCases(cases).catch((error) => { console.error(error); process.exitCode = 1; });
