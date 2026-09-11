import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const referencePanelSource = readFileSync(
  join(process.cwd(), "src/components/ReferencePanel.svelte"),
  "utf8",
);
const appSource = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");
const pageContentSource = readFileSync(
  join(process.cwd(), "src/components/PageContent.svelte"),
  "utf8",
);
const currentBlockAnchorSource = readFileSync(
  join(process.cwd(), "src/lib/currentBlockAnchor.ts"),
  "utf8",
);

describe("ReferencePanel current-block Ask scope", () => {
  it("exposes a current-block scope in the Ask panel", () => {
    expect(referencePanelSource).toContain('type AskScope = "block" | "page" | "graph";');
    expect(referencePanelSource).toContain('let askScope = $state<AskScope>("block");');
    expect(referencePanelSource).toContain("Current block");
    expect(referencePanelSource).toContain('class:active={askScope === "block"}');
    expect(referencePanelSource).toContain("buildCurrentBlockContext");
  });

  it("supports cited web fact-checking for the current block", () => {
    expect(referencePanelSource).toContain("shouldUseWebResearchForBlock");
    expect(referencePanelSource).toContain("runCurrentBlockWebResearch");
    expect(referencePanelSource).toContain("aiResearchWeb(");
    expect(referencePanelSource).toContain("webSourcesFromResearch");
    expect(referencePanelSource).not.toContain("Fact-check block");
  });

  it("receives focused-block anchors from PageContent", () => {
    expect(currentBlockAnchorSource).toContain('new CustomEvent("page-content-focus-changed"');
    expect(currentBlockAnchorSource).toContain("getLatestCurrentBlockAnchor");
    expect(pageContentSource).toContain("setCurrentBlockAnchor(page.id, blockId);");
    expect(pageContentSource).toContain("emitFocusChanged(blockId);");
    expect(pageContentSource).toContain("emitFocusChanged(null);");
    expect(pageContentSource).toContain("onAnchor={handleBlockAnchor}");
  });

  it("restores the last block anchor when the panel mounts after a block click", () => {
    expect(referencePanelSource).toContain("getLatestCurrentBlockAnchor()");
    expect(referencePanelSource).toContain("askBlockAnchor");
  });

  it("can recover a visible block from the question text if no click anchor was captured", () => {
    expect(referencePanelSource).toContain("visibleBlockAnchorFromQuestion");
    expect(referencePanelSource).toContain("searchedBlockAnchorFromQuestion");
    expect(referencePanelSource).toContain("searchFts(");
    expect(referencePanelSource).toContain(".block-item[data-block-id][data-page-id]");
  });

  it("keeps Ask answers as a follow-up thread", () => {
    expect(referencePanelSource).toContain("let askTurns = $state<AskTurn[]>([])");
    expect(referencePanelSource).toContain("let askMessages = $state<ChatMessageModel[]>([])");
    expect(referencePanelSource).toContain("formatAskThreadForPrompt");
    expect(referencePanelSource).toContain("addAskTurn");
    expect(referencePanelSource).toContain("Summarize answers");
    expect(referencePanelSource).toContain("Clear thread");
  });

  it("renders Ask with the shared chat bubble and adds pending turns immediately", () => {
    expect(referencePanelSource).toContain('import ChatMessageBubble from "./ChatMessageBubble.svelte";');
    expect(referencePanelSource).toContain("beginAskMessage(question, willUseWebResearch)");
    expect(referencePanelSource).toContain('{ role: "user", content: question }');
    expect(referencePanelSource).toContain('{ role: "assistant", content: "", webResearch }');
    expect(referencePanelSource).toContain("askPendingAssistantIndex");
    expect(referencePanelSource).toContain("<ChatMessageBubble");
    expect(referencePanelSource).toContain("thinkingLabel={askPendingAssistantIndex === index ? askThinkingLabel : \"\"}");
  });

  it("can draft and apply an improved current block from the Ask thread", () => {
    expect(referencePanelSource).toContain("Merge answer with block");
    expect(referencePanelSource).toContain("Merged block draft");
    expect(referencePanelSource).toContain("Replace current block");
    expect(referencePanelSource).toContain('type: "update_block"');
    expect(referencePanelSource).toContain("Act like a careful code-edit assistant");
  });

  it("pins the Ask composer below a scrollable answer thread", () => {
    expect(referencePanelSource).toContain('class="tab-content ask-tab"');
    expect(referencePanelSource).toContain('class="ask-scroll"');
    expect(referencePanelSource).toContain('class="ask-composer"');
    expect(referencePanelSource).toContain('class="search-form ask-form"');
    expect(referencePanelSource).toContain("onkeydown={handleAskKeydown}");
  });

  it("allows the right panel to resize nearly to the viewport edge", () => {
    expect(appSource).toContain("REFERENCE_PANEL_VIEWPORT_EDGE_GAP");
    expect(appSource).not.toContain("REFERENCE_PANEL_MAX_WIDTH");
    expect(referencePanelSource).toContain("max-width: calc(100vw - 24px)");
  });

  it("keeps journal page-scoped Ask on the focused journal day", () => {
    expect(appSource).toContain('preferFocusedPageForPageScope={currentView === "journal"}');
    expect(referencePanelSource).toContain("buildPageContextForAsk");
    expect(referencePanelSource).toContain("resolvePageContextTarget");
    expect(referencePanelSource).toContain("not the whole journal feed");
  });
});
