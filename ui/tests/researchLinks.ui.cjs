const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, focusContinuous, frames, row } = require("./keyboardSelection.ui.cjs");

const LINK_SOURCE = "Imported concept: [[Insulin resistance|#insulin_resistance]].";
const GRAPH_PATH = "/synthetic/research-links";

async function installLinkFixture(page, { summary = false, partialAccept = false } = {}) {
  await page.addInitScript(({ source, graphPath, summary, partialAccept }) => {
    function install(internals) {
      const state = window.__selectionState;
      const original = internals.invoke;
      const makePage = (id, title) => ({
        ...structuredClone(state.pages[0]), id, title, file_path: `pages/${id}.md`,
      });
      const targets = [
        makePage("insulin-resistance", "Insulin resistance"),
        makePage("insulin-sensitivity", "Insulin sensitivity"),
      ];
      state.pages.push(...targets);
      for (const target of targets) state.blocks.push({
        ...structuredClone(state.blocks[0]), id: `${target.id}-block`, page_id: target.id,
        content: `Canonical concept: ${target.title}.`,
      });
      const content = {
        b0: source,
        b1: "Glucose signaling appears in this synthetic source.",
        b2: "Glucose signaling appears again.",
        b3: "Insulin response needs an explicit target choice.",
        b4: "Insulin resistance already has a canonical page.",
      };
      for (const block of state.blocks) if (content[block.id]) block.content = content[block.id];
      const candidate = (id, blockId, anchor, title, extra = {}) => {
        const text = state.blocks.find((block) => block.id === blockId).content;
        const start = text.indexOf(anchor);
        return {
          id, from_block_id: blockId, from_page_id: "selection-page", from_page_title: "Keyboard selection",
          to_page_id: null, to_page_title: title, proposed_title: title, resolution: "new",
          alternatives: [], reason: `Proposed concept: ${title}.`, anchor_text: anchor,
          anchor_start: new TextEncoder().encode(text.slice(0, start)).length,
          anchor_end: new TextEncoder().encode(text.slice(0, start + anchor.length)).length,
          status: "pending", source: "semantic_concept", confidence: 0.9, created_at: 1, updated_at: 1,
          ...extra,
        };
      };
      const links = window.__researchLinksFixture = {
        calls: [], navigation: [], acceptedBefore: {}, candidates: [
          candidate("new-signaling-first", "b1", "Glucose signaling", "Glucose signaling"),
          candidate("new-signaling-second", "b2", "Glucose signaling", "Glucose signaling"),
          candidate("new-transport", "b1", "Glucose signaling", "Glucose transport"),
          candidate("ambiguous-response", "b3", "Insulin response", "Insulin response", {
            resolution: "ambiguous", reason: "Two existing insulin concepts need review.",
            alternatives: targets.map(({ id, title }) => ({ id, title })),
          }),
          candidate("reuse-resistance", "b4", "Insulin resistance", "Insulin resistance", {
            to_page_id: "insulin-resistance", resolution: "reuse", reason: "An existing canonical page matches.",
          }),
        ],
      };
      if (summary) {
        const sourceBlocks = structuredClone(state.blocks.filter((block) => block.page_id === "selection-page"));
        const root = {
          ...structuredClone(sourceBlocks[0]), id: "summary-root", order_index: 1, content: "## Research summary",
        };
        const child = {
          ...structuredClone(root), id: "summary-topic", parent_id: root.id, order_index: 0,
          content: "Synthetic summary: [[Glucose signaling|#glucose_signaling]]; Insulin response.",
        };
        const created = makePage("created-glucose", "Glucose signaling");
        links.summary = {
          title_answer: "A synthetic research answer.",
          topics: [{
            topic: "Metabolism", summary: "A synthetic summary of glucose signaling and insulin response.",
            tags: [{ term: "Glucose signaling" }, { term: "Insulin response" }],
          }],
        };
        links.sourceBlocks = sourceBlocks;
        links.receipt = {
          graphPath, pageId: "selection-page", insertedBlockId: root.id, insertedContent: root.content,
          insertedAfterBlockId: "b0", insertedBlocks: [root, child],
          siblingOrderBefore: sourceBlocks.filter((block) => block.parent_id === null)
            .map((block) => ({ blockId: block.id, orderIndex: block.order_index })),
          resolvedTargets: [{ pageId: "insulin-resistance", title: "Insulin resistance" }],
          createdTargets: [created],
          unlinkedTargets: [{
            sourcePhrase: "Insulin response", targetTitle: "Insulin response", reason: "Multiple approved aliases match.",
          }],
          wrapChanges: [],
        };
      }
      function acceptCandidate(item, target, createNew) {
        const block = state.blocks.find((block) => block.id === item.from_block_id);
        links.acceptedBefore[item.id] = block.content;
        const slug = target.title.toLowerCase().replaceAll(" ", "_");
        block.content = block.content.replace(item.anchor_text, `[[${target.title}|#${slug}]]`);
        Object.assign(item, {
          to_page_id: target.id, to_page_title: target.title, status: "accepted",
          resolution: createNew ? "new" : "reuse", alternatives: [],
        });
        return structuredClone(item);
      }
      window.addEventListener("navigate-page", (event) => links.navigation.push(structuredClone(event.detail)));
      internals.invoke = async (cmd, args = {}) => {
        // Native IPC serializes Svelte proxies as JSON rather than structured-cloning them.
        links.calls.push({ cmd, args: JSON.parse(JSON.stringify(args)) });
        if (cmd === "get_graph_info") return { name: "Synthetic research links", path: graphPath };
        if (summary && cmd === "ai_health_check") {
          return { enabled: true, llm_available: true, embedder_available: false, vector_count: 0 };
        }
        if (summary && cmd === "ai_generate_references") {
          return {
            page_id: args.pageId, generated_at: Date.now(), content_hash: "synthetic", reference_count: 0,
            references: [], summary: structuredClone(links.summary),
          };
        }
        if (summary && cmd === "ai_insert_page_summary") {
          for (const block of state.blocks) {
            if (block.page_id === "selection-page" && block.parent_id === null && block.order_index > 0) block.order_index++;
          }
          state.blocks.push(...structuredClone(links.receipt.insertedBlocks));
          state.pages.push(...structuredClone(links.receipt.createdTargets));
          return structuredClone(links.receipt);
        }
        if (summary && cmd === "ai_undo_summary_insert") {
          const receipt = args.receipt;
          const insertedIds = new Set(receipt.insertedBlocks.map((block) => block.id));
          const createdIds = new Set(receipt.createdTargets.map((target) => target.id));
          state.blocks = state.blocks.filter((block) => !insertedIds.has(block.id));
          for (const before of receipt.siblingOrderBefore) {
            state.blocks.find((block) => block.id === before.blockId).order_index = before.orderIndex;
          }
          state.pages = state.pages.filter((target) => !createdIds.has(target.id));
          return { retainedTargets: [] };
        }
        if (cmd === "discover_link_candidates" || cmd === "list_link_candidates") {
          return structuredClone(links.candidates.filter((item) =>
            item.from_page_id === args.pageId && item.status === "pending"));
        }
        if (cmd === "resolve_link_candidate") {
          const item = links.candidates.find((item) => item.id === args.candidateId);
          if (!item) throw new Error(`Unknown synthetic candidate: ${args.candidateId}`);
          let target = targets.find((target) => target.id === args.targetPageId);
          if (!args.createNew && !target) throw new Error("A specific existing target must be chosen.");
          if (args.createNew) {
            target = makePage("chosen-new-concept", item.proposed_title);
            state.pages.push(target);
          }
          return acceptCandidate(item, target, args.createNew);
        }
        if (partialAccept && cmd === "accept_link_candidate") {
          if (args.candidateId !== "new-signaling-first") {
            throw new Error("Candidate source changed; refresh suggestions.");
          }
          const item = links.candidates.find((item) => item.id === args.candidateId);
          const target = makePage("partially-created-concept", item.proposed_title);
          state.pages.push(target);
          return acceptCandidate(item, target, true);
        }
        if (cmd === "undo_link_candidate_accept") {
          const item = links.candidates.find((item) => item.id === args.candidateId);
          state.blocks.find((block) => block.id === item.from_block_id).content = links.acceptedBefore[item.id];
          item.status = "pending";
          return structuredClone(item);
        }
        if (["create_page", "accept_link_candidate", "dismiss_link_candidate"].includes(cmd)) {
          throw new Error(`Preview and target selection must not call ${cmd}.`);
        }
        return original(cmd, args);
      };
      return internals;
    }
    let internals = window.__TAURI_INTERNALS__;
    if (internals) internals = install(internals);
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true, get: () => internals, set: (value) => { internals = install(value); },
    });
  }, { source: LINK_SOURCE, graphPath: GRAPH_PATH, summary, partialAccept });
}

const group = (page, title) => page.locator(".link-candidate-row").filter({
  has: page.locator(".link-candidate-target").getByText(title, { exact: true }),
});
const acceptButton = (candidate) => candidate.getByRole("button", { name: /^(Link|Link all|Fix \+ link)$/ });
const mutationCommands = new Set([
  "create_page", "create_block", "create_blocks", "update_block", "update_page_source",
  "delete_block", "delete_blocks", "accept_link_candidate", "dismiss_link_candidate", "resolve_link_candidate",
]);

async function assertReadOnly(page, { allowResolve = false, allowNavigation = false } = {}) {
  await frames(page);
  const { calls, navigation } = await page.evaluate(() => window.__researchLinksFixture);
  assert.deepEqual(calls.filter(({ cmd }) => mutationCommands.has(cmd) && !(allowResolve && cmd === "resolve_link_candidate")), [],
    "preview must not create pages, accept candidates, or change source content");
  if (!allowNavigation) assert.deepEqual(navigation, [], "candidate previews must not navigate");
}

async function revealCandidates(page) {
  await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-find-links", {
    detail: { pageId: "selection-page", exactOnly: true },
  })));
  await page.locator(".link-candidate-row").first().waitFor();
  assert.match(await page.locator(".link-candidates-summary").innerText(), /Reviewing 5 occurrences grouped into 4 concepts/);
}

const cases = [
  ...[false, true].map((continuous) => [
    `${continuous ? "continuous" : "classic"} imported concept aliases display their slug but open the canonical page`,
    { unifiedPage: continuous },
    async (page) => {
      if (continuous) await focusContinuous(page, "selection-page", "b1");
      else await focus(page, "b1");
      const block = continuous ? page.locator('[data-source-block-id="b0"]') : row(page, "b0");
      const link = block.locator('a.page-link[data-page="Insulin resistance"]');
      await link.waitFor();
      assert.equal(await link.innerText(), "#insulin_resistance");
      assert.equal(await block.locator("[data-tag]").count(), 0, "the display slug is not a separate tag");
      await link.click();
      await row(page, "insulin-resistance-block").waitFor();
      const { calls, navigation } = await page.evaluate(() => window.__researchLinksFixture);
      assert.equal(navigation.length, 1);
      const destination = typeof navigation[0] === "string" ? navigation[0] : navigation[0].pageName;
      assert.equal(destination, "Insulin resistance");
      assert.ok(calls.some(({ cmd, args }) => cmd === "get_page" && args.title === "Insulin resistance"));
      assert.equal(calls.some(({ cmd, args }) => cmd === "get_page" && /insulin_resistance|\|/.test(args.title ?? "")), false);
      await assertReadOnly(page, { allowNavigation: true });
    },
  ]),
  ["null-target proposals stay distinct while previews remain read-only", {}, async (page) => {
    await revealCandidates(page);
    const signaling = group(page, "Glucose signaling");
    const transport = group(page, "Glucose transport");
    assert.equal(await signaling.count(), 1);
    assert.equal(await transport.count(), 1);
    assert.match(await signaling.innerText(), /2 occurrences on this page/);
    assert.match(await transport.innerText(), /1 occurrence on this page/);
    for (const title of ["Glucose signaling", "Glucose transport", "Insulin response"]) {
      const candidate = group(page, title);
      const target = candidate.locator(".link-candidate-target");
      assert.equal(await target.isDisabled(), true, `${title} has no navigable page yet`);
      await target.evaluate((button) => button.click());
      await candidate.locator(".link-candidate-context").click();
      await candidate.locator(".link-candidate-anchor").click();
      await assertReadOnly(page);
    }
    assert.equal(await acceptButton(signaling).isEnabled(), true);
    assert.equal(await acceptButton(transport).isEnabled(), true);
    assert.equal(await page.locator(".link-candidate-row").count(), 4);
  }],
  ...[false, true].map((createNew) => [
    `explicit ${createNew ? "new" : "existing"} concept choice accepts atomically and records one undo action`,
    {},
    async (page) => {
      await revealCandidates(page);
      const before = await page.evaluate(() => ({
        blocks: window.__selectionState.blocks, pages: window.__selectionState.pages,
      }));
      const ambiguous = group(page, "Insulin response");
      assert.match(await ambiguous.innerText(), /Two existing insulin concepts need review/);
      assert.equal(await acceptButton(ambiguous).isDisabled(), true);
      await acceptButton(ambiguous).evaluate((button) => button.click());
      const chooser = ambiguous.getByRole("group", { name: "Choose concept target" });
      assert.equal(await chooser.getByRole("button").count(), 3);
      await chooser.getByRole("button", { name: "Use Insulin resistance", exact: true }).waitFor();
      await chooser.getByRole("button", { name: "Use Insulin sensitivity", exact: true }).waitFor();
      await assertReadOnly(page);
      assert.deepEqual(await page.evaluate(() => ({
        blocks: window.__selectionState.blocks, pages: window.__selectionState.pages,
      })), before, "no proposal preview may create a page or edit notes");

      await chooser.getByRole("button", {
        name: createNew ? "Create and link new concept" : "Use Insulin sensitivity", exact: true,
      }).click();
      await ambiguous.waitFor({ state: "detached" });
      const targetTitle = createNew ? "Insulin response" : "Insulin sensitivity";
      const sourceLink = row(page, "b3").locator(`a.page-link[data-page="${targetTitle}"]`);
      await sourceLink.waitFor();
      const accepted = await page.evaluate(() => ({
        calls: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "resolve_link_candidate"),
        candidate: window.__researchLinksFixture.candidates.find(({ id }) => id === "ambiguous-response"),
        pages: window.__selectionState.pages, undo: window.__undoStack,
      }));
      assert.deepEqual(accepted.calls.map(({ args }) => args), [{
        candidateId: "ambiguous-response", targetPageId: createNew ? null : "insulin-sensitivity",
        createNew, graphPath: GRAPH_PATH,
      }]);
      assert.equal(accepted.candidate.status, "accepted");
      assert.equal(accepted.pages.length, before.pages.length + Number(createNew));
      assert.deepEqual(accepted.undo, [{
        type: "accept_link_candidates", pageId: "selection-page", candidateIds: ["ambiguous-response"],
      }]);
      await assertReadOnly(page, { allowResolve: true });

      await focus(page, "b4");
      await page.keyboard.press("Control+z");
      await sourceLink.waitFor({ state: "detached" });
      const undone = await page.evaluate(() => ({
        calls: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "undo_link_candidate_accept"),
        blocks: window.__selectionState.blocks, undo: window.__undoStack, redo: window.__redoStack,
      }));
      assert.deepEqual(undone.calls, [{
        cmd: "undo_link_candidate_accept", args: { candidateId: "ambiguous-response" },
      }]);
      assert.deepEqual(undone.blocks, before.blocks);
      assert.deepEqual(undone.undo, []);
      assert.deepEqual(undone.redo, accepted.undo);
      await assertReadOnly(page, { allowResolve: true });
    },
  ]),
  ["a stale second occurrence reports failure while the first accepted link remains undoable", { partialAccept: true }, async (page) => {
    await revealCandidates(page);
    const before = await page.evaluate(() => window.__selectionState.blocks);
    await group(page, "Glucose signaling").getByRole("button", { name: "Link all", exact: true }).click();
    const sourceLink = row(page, "b1").locator('a.page-link[data-page="Glucose signaling"]');
    await sourceLink.waitFor();
    await page.locator(".link-candidates-summary").getByText(
      "Reviewing 4 occurrences grouped into 4 concepts.", { exact: true },
    ).waitFor();
    await frames(page);
    const after = await page.evaluate(() => ({
      calls: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "accept_link_candidate"),
      blocks: window.__selectionState.blocks, undo: window.__undoStack,
      candidates: window.__researchLinksFixture.candidates,
    }));
    assert.deepEqual(after.calls.map(({ args }) => args.candidateId), ["new-signaling-first", "new-signaling-second"]);
    assert.equal(after.candidates.find(({ id }) => id === "new-signaling-first").status, "accepted");
    assert.equal(after.candidates.find(({ id }) => id === "new-signaling-second").status, "pending");
    assert.deepEqual(after.blocks.filter(({ id }) => id !== "b1"), before.filter(({ id }) => id !== "b1"));
    assert.deepEqual(after.undo, [{
      type: "accept_link_candidates", pageId: "selection-page", candidateIds: ["new-signaling-first"],
    }], "the successfully returned candidate must be recorded even when the next native guard rejects");
    const error = page.locator(".link-candidates-error");
    const visibleError = await error.isVisible() ? await error.innerText() : "";
    assert.match(await page.locator(".link-candidates-undo").innerText(), /Linked\s+1\s+occurrence/);

    await focus(page, "b4");
    await page.keyboard.press("Control+z");
    await sourceLink.waitFor({ state: "detached" });
    const undone = await page.evaluate(() => ({
      calls: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "undo_link_candidate_accept"),
      blocks: window.__selectionState.blocks, undo: window.__undoStack, redo: window.__redoStack,
    }));
    assert.deepEqual(undone.calls, [{
      cmd: "undo_link_candidate_accept", args: { candidateId: "new-signaling-first" },
    }]);
    assert.deepEqual(undone.blocks, before);
    assert.deepEqual(undone.undo, []);
    assert.deepEqual(undone.redo, after.undo);
    assert.match(visibleError, /Candidate source changed; refresh suggestions/,
      "refreshing partial successes must not erase the native failure message");
  }],
  ["existing candidate targets still navigate to their canonical page", {}, async (page) => {
    await revealCandidates(page);
    const target = group(page, "Insulin resistance").locator(".link-candidate-target");
    assert.equal(await target.isEnabled(), true);
    await target.click();
    await row(page, "insulin-resistance-block").waitFor();
    await assertReadOnly(page, { allowNavigation: true });
  }],
  ["Chat summary uses captured guards, warns about ambiguity, and undoes its full receipt once", { summary: true }, async (page) => {
    const before = await page.evaluate(() => structuredClone({
      blocks: window.__selectionState.blocks, pages: window.__selectionState.pages,
    }));
    await focus(page, "b0");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    await page.locator(".reference-panel summary").filter({ hasText: "Page / selection tools" }).click();
    const panel = page.locator(".reference-panel");
    await panel.getByRole("button", { name: "Summarize this Page", exact: true }).click();
    await panel.getByRole("region", { name: "Page summary", exact: true }).waitFor();
    const captured = await page.evaluate(() => window.__researchLinksFixture.calls
      .find(({ cmd }) => cmd === "ai_generate_references").args);
    const sourceBlocks = before.blocks.filter((block) => block.page_id === "selection-page");
    assert.equal(captured.graphPath, GRAPH_PATH);
    assert.deepEqual(captured.expectedBlocks, sourceBlocks);
    await assertReadOnly(page);
    assert.deepEqual(await page.evaluate(() => window.__undoStack), []);

    await focus(page, "b4");
    await panel.getByRole("button", { name: "Insert into page", exact: true }).click();
    await row(page, "summary-root").waitFor();
    await row(page, "summary-topic").waitFor();
    const inserted = await page.evaluate(() => ({
      calls: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "ai_insert_page_summary"),
      receipt: window.__researchLinksFixture.receipt, summary: window.__researchLinksFixture.summary,
      undo: window.__undoStack, blocks: window.__selectionState.blocks,
    }));
    assert.equal(inserted.calls.length, 1);
    assert.deepEqual(inserted.calls[0].args, {
      pageId: "selection-page", titleAnswer: inserted.summary.title_answer, topics: inserted.summary.topics,
      afterBlockId: "b0", graphPath: GRAPH_PATH, expectedBlocks: captured.expectedBlocks,
    }, "insertion uses the generation-time source snapshot and anchor, without source wrapping");
    assert.deepEqual(inserted.undo, [{ type: "insert_summary", ...inserted.receipt }]);
    const contentById = (blocks) => blocks.map(({ id, content, parent_id }) => ({ id, content, parent_id }));
    const originalIds = new Set(before.blocks.map((block) => block.id));
    assert.deepEqual(contentById(inserted.blocks.filter((block) => originalIds.has(block.id))), contentById(before.blocks));
    assert.match(await panel.getByRole("status").innerText(),
      /Left ambiguous concepts unlinked: Insulin response \(Multiple approved aliases match\.\)/);
    assert.equal(await panel.getByRole("button", { name: "Inserted into page ✓", exact: true }).isDisabled(), true);
    await assertReadOnly(page);

    await focus(page, "b4");
    await page.keyboard.press("Control+z");
    await row(page, "summary-root").waitFor({ state: "detached" });
    await row(page, "summary-topic").waitFor({ state: "detached" });
    const undone = await page.evaluate(() => ({
      calls: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "ai_undo_summary_insert"),
      undo: window.__undoStack, redo: window.__redoStack,
      blocks: window.__selectionState.blocks, pages: window.__selectionState.pages,
    }));
    assert.deepEqual(undone.calls, [{
      cmd: "ai_undo_summary_insert", args: { receipt: { type: "insert_summary", ...inserted.receipt } },
    }]);
    assert.deepEqual(undone.undo, []);
    assert.deepEqual(undone.redo, inserted.undo);
    assert.deepEqual({ blocks: undone.blocks, pages: undone.pages }, before);
    await assertReadOnly(page);
  }],
];

if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failed = 0;
  try {
    for (const [name, options, run] of cases) {
      let fixture;
      try {
        fixture = await openEditor(browser, {
          ...options, beforeNavigate: (page) => installLinkFixture(page, options),
        });
        await run(fixture.page);
        assert.deepEqual(fixture.errors, []);
        console.log(`PASS ${name}`);
      } catch (error) {
        failed++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error("Link fixture:", await fixture.page.evaluate(() => ({
          navigation: window.__researchLinksFixture.navigation,
          lookups: window.__researchLinksFixture.calls.filter(({ cmd }) => cmd === "get_page").slice(-4),
          title: document.querySelector(".main-content .page-title")?.textContent,
          summaryCalls: window.__researchLinksFixture.calls.filter(({ cmd }) =>
            ["ai_insert_page_summary", "ai_undo_summary_insert"].includes(cmd)),
          errors: [...document.querySelectorAll(".reference-panel .error-msg")].map((node) => node.textContent),
          undo: window.__undoStack?.map(({ type }) => type),
        })), fixture.errors);
      } finally { await fixture?.page.close(); }
    }
  } finally { await browser.close(); }
  if (failed) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });
