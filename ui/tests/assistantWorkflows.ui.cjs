const { chromium } = require("playwright");
const { assert, openAssistant, panel, input, button, frames } = require("./assistantFixture.cjs");

async function installWorkflows(page) {
  await page.evaluate(() => {
    const base = window.__TAURI_INTERNALS__.invoke;
    const state = window.__selectionState;
    const pageId = "00000000-0000-4000-8000-000000000010";
    const otherId = "00000000-0000-4000-8000-000000000011";
    const ids = ["00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002"];
    const source = { ...structuredClone(state.pages[0]), id: pageId, title: "Passport planning", file_path: "pages/passport.md" };
    state.pages.push(source, { ...source, id: otherId, title: "Travel preparation", file_path: "pages/travel.md" });
    const originals = ids.map((id, index) => ({ ...structuredClone(state.blocks[0]), id, page_id: index ? otherId : pageId,
      parent_id: null, order_index: 0, content: index ? "TODO renew my passport" : "TODO arrange passport renewal" }));
    state.blocks.push(...originals);
    const fixture = window.__workflowFixture = { calls: [], writes: [], kind: "", revision: 0, hold: false,
      originals: structuredClone(originals), pageId, otherId, ids, modelReplies: [] };
    const tokens = new Map(), pending = new Map();
    window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
      if (!command.startsWith("assistant_workflow_")) return base(command, args);
      fixture.calls.push({ command, args: structuredClone(args) });
      if (command === "assistant_workflow_snapshot") {
        fixture.kind = args.kind;
        const token = crypto.randomUUID();
        tokens.set(token, { revision: fixture.revision, kind: args.kind });
        return {
          token, graphPath: args.graphPath, kind: args.kind,
          sourcePageId: args.kind === "tasks" ? null : pageId,
          sourcePageTitle: args.kind === "tasks" ? null : source.title,
          coverage: args.kind === "tasks" ? "2 of 2 open tasks." : "2 saved Markdown pages.",
          tasks: originals.map((block, index) => ({ id: block.id, pageId: block.page_id,
            pageTitle: index ? "Travel preparation" : source.title, content: block.content, state: "TODO" })),
          pages: [pageId, otherId].map(id => ({
            id, title: state.pages.find(page => page.id === id).title,
            blocks: state.blocks.filter(block => block.page_id === id).map(block => ({
              id: block.id, content: block.content, parentId: block.parent_id, orderIndex: block.order_index,
            })),
          })),
        };
      }
      if (command === "assistant_workflow_complete") {
        if (fixture.hold) return new Promise((resolve, reject) => pending.set(args.requestId, { resolve, reject }));
        if (fixture.modelReplies.length) return JSON.stringify(fixture.modelReplies.shift());
        const marker = "\nINPUT_JSON:\n";
        if (args.prompt.includes(marker)) {
          const payload = JSON.parse(args.prompt.slice(args.prompt.lastIndexOf(marker) + marker.length));
          if (payload.candidates) return JSON.stringify({ results: payload.candidates.map(segment => ({
            segmentId: segment.segmentId, pageId: segment.pageId, related: true,
            connection: "Also covers passport renewal planning.", blockIds: segment.blockId ? [segment.blockId] : [],
          })) });
          return JSON.stringify({ edits: [{ before: "arrange passport renewal", after: "schedule passport renewal" }], ambiguities: [] });
        }
        const payload = JSON.parse(args.prompt.split("DATA_JSON:\n")[1]);
        const taskIds = payload.tasks.map(task => task.id);
        return JSON.stringify({
          groups: [{ groupId: null, label: "Passport renewal", description: "Arrange passport renewal", taskIds }],
          possibleDuplicates: [{ taskIds, reason: "Both refer to renewing the same passport" }],
        });
      }
      if (command === "assistant_workflow_cancel") {
        pending.get(args.requestId)?.reject(new Error("Analysis cancelled"));
        pending.delete(args.requestId); return;
      }
      if (command === "assistant_workflow_apply") {
        const capture = tokens.get(args.token);
        if (!capture || capture.revision !== fixture.revision) throw new Error("Source notes changed. Prepare a fresh proposal.");
        const existing = state.pages.find(page => page.title === args.title);
        if (capture.kind !== "rewrite" && existing) throw new Error("Destination already exists");
        const destination = capture.kind === "rewrite" ? source : { ...source, id: crypto.randomUUID(), title: args.title, file_path: "pages/review.md" };
        const created = capture.kind !== "rewrite";
        const rootOrder = created ? 0 : Math.max(...state.blocks.filter(block => block.page_id === destination.id && !block.parent_id).map(block => block.order_index)) + 1;
        assertReviewedMarkdown(args);
        const draft = [...args.blocks, { content: args.reviewedMarkdown, parentIndex: 0 }];
        const inserted = draft.map((block, index) => ({ ...structuredClone(originals[0]), id: crypto.randomUUID(),
          page_id: destination.id, content: block.content, parent_id: null, order_index: index ? index - 1 : rootOrder }));
        draft.forEach((block, index) => { if (block.parentIndex !== undefined) inserted[index].parent_id = inserted[block.parentIndex].id; });
        if (created) state.pages.push(destination);
        state.blocks.push(...inserted);
        fixture.writes.push(structuredClone(args));
        tokens.delete(args.token);
        return { pageId: destination.id, pageTitle: destination.title, insertedBlocks: inserted, pageCreated: created };
      }
      throw new Error(`Unexpected workflow command ${command}`);
    };
    function assertReviewedMarkdown(args) {
      if (typeof args.reviewedMarkdown !== "string" || !args.reviewedMarkdown.trim() || args.blocks.length !== 1)
        throw new Error("Expected intact reviewed Markdown and one root for native parsing");
    }
  });
}

async function group(page) {
  await input(page).fill("Please go through all TODOs and group them by similarity so I can dedup.");
  await button(page, "Send").click();
  const card = panel(page).getByRole("region", { name: "Review ASK action" });
  await card.waitFor();
  return card;
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const fixture = await openAssistant(browser, { global: true });
    const { page } = fixture;
    await installWorkflows(page);
    const card = await group(page);
    assert.equal(await page.evaluate(() => window.__workflowFixture.writes.length), 0);
    assert(await card.getByRole("button", { name: "TODO arrange passport renewal", exact: true }).first().isVisible());
    assert((await card.innerText()).includes("Possible duplicates"));
    await input(page).fill("A different unsent question");
    await frames(page);
    assert(await card.isVisible(), "Draft edits must not discard a prepared action");
    await card.getByLabel("New page title").fill("Reviewed task groups");
    await card.getByRole("button", { name: "Edit draft", exact: true }).click();
    await card.getByRole("textbox", { name: "Draft text" }).fill(
      "## My reviewed group\n- ((00000000-0000-4000-8000-000000000001))\n- ((00000000-0000-4000-8000-000000000002))");
    await card.getByRole("button", { name: "Apply", exact: true }).click();
    await page.waitForFunction(() => window.__workflowFixture.writes.length === 1);
    await card.waitFor({ state: "detached" });
    const result = await page.evaluate(() => {
      const f = window.__workflowFixture;
      return { title: f.writes[0].title, text: f.writes[0].reviewedMarkdown,
        current: window.__selectionState.blocks.filter(block => f.ids.includes(block.id)), original: f.originals };
    });
    assert.equal(result.title, "Reviewed task groups");
    assert(result.text.includes("My reviewed group"));
    assert(!result.text.includes("TODO"));
    assert.deepEqual(result.current, result.original);

    await group(page);
    await page.evaluate(() => { window.__workflowFixture.revision++; });
    await card.getByRole("button", { name: "Apply", exact: true }).click();
    await card.getByRole("alert").getByText(/Source notes changed/).waitFor();
    assert.equal(await page.evaluate(() => window.__workflowFixture.writes.length), 1);
    await card.getByRole("button", { name: "Dismiss", exact: true }).click();
    await page.evaluate(() => { window.__workflowFixture.hold = true; });
    await input(page).fill("Group all my tasks by similarity");
    await button(page, "Send").click();
    await page.waitForFunction(() => window.__workflowFixture.calls.filter(call => call.command === "assistant_workflow_complete").length === 3);
    await button(page, "Stop analysis").click();
    await panel(page).getByRole("alert").getByText("Analysis stopped. No changes were saved.", { exact: true }).waitFor();
    assert.equal(await card.count(), 0);
    assert.equal(await page.evaluate(() => window.__workflowFixture.writes.length), 1);
    await page.evaluate(async () => {
      const fixture = window.__workflowFixture;
      fixture.hold = false;
      const { getGlobalConversation, updateAssistantConversation } = await import("/src/lib/assistantConversations.ts");
      const thread = getGlobalConversation("/synthetic/keyboard-selection");
      thread.sourcePageId = fixture.pageId;
      thread.sourcePageTitle = "Passport planning";
      thread.context = { kind: "page", pageId: fixture.pageId };
      thread.contextLabel = "This page - Passport planning";
      updateAssistantConversation();
    });
    await input(page).fill("Find notes with similar topics to this page and create a page with references.");
    await button(page, "Send").click();
    await card.waitFor();
    assert((await card.innerText()).includes("Travel preparation"));
    assert((await card.innerText()).includes("Also covers passport renewal planning"));
    assert.equal(await page.evaluate(() => window.__workflowFixture.writes.length), 1);
    await card.getByRole("button", { name: "Apply", exact: true }).click();
    await page.waitForFunction(() => window.__workflowFixture.writes.length === 2);
    await card.waitFor({ state: "detached" });
    await input(page).fill("Clean up this page and append a clearer draft below my original notes.");
    await button(page, "Send").click();
    await card.waitFor();
    assert((await card.innerText()).includes("schedule passport renewal"));
    assert((await card.innerText()).includes("Original notes stay unchanged"));
    await card.getByRole("button", { name: "Apply", exact: true }).click();
    await page.waitForFunction(() => window.__workflowFixture.writes.length === 3);
    await card.waitFor({ state: "detached" });
    const rewrite = await page.evaluate(() => {
      const fixture = window.__workflowFixture;
      return { draft: fixture.writes.at(-1).reviewedMarkdown, original: fixture.originals,
        current: window.__selectionState.blocks.filter(block => fixture.ids.includes(block.id)) };
    });
    assert.deepEqual(rewrite.current, rewrite.original);
    assert(rewrite.draft.includes("schedule passport renewal"));
    const beforeSwitch = await page.evaluate(() => {
      window.__workflowFixture.hold = true;
      return window.__workflowFixture.calls.filter(call => call.command === "assistant_workflow_cancel").length;
    });
    await input(page).fill("Group my open tasks by similarity");
    await button(page, "Send").click();
    await button(page, "Stop analysis").waitFor();
    await page.waitForFunction(() => window.__workflowFixture.calls.at(-1)?.command === "assistant_workflow_complete");
    await page.locator(".graph-selector").click();
    await page.getByRole("menuitem", { name: "Other test graph", exact: true }).click();
    await page.waitForFunction(before => window.__workflowFixture.calls.filter(call => call.command === "assistant_workflow_cancel").length === before + 1, beforeSwitch);
    await page.keyboard.press("Alt+c");
    await input(page).waitFor();
    assert.equal(await card.count(), 0, "Proposals from the old graph cannot appear in a new graph");
    assert.equal(await page.evaluate(() => window.__workflowFixture.writes.length), 3);
    assert.deepEqual(await page.evaluate(() => window.__assistantFixture.legacyCalls), []);
    assert.deepEqual(fixture.errors, []);
    assert.deepEqual(page.assistantExternalRequests, []);
    console.log("ASK workflows: task groups, related topics, appended cleanup, editable approval, preserved originals, stale-source refusal, cancellation PASS");
    await page.close();
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
