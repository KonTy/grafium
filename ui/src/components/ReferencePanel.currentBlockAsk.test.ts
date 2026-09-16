import { describe, expect, it } from "vitest";
import panel from "./ReferencePanel.svelte?raw";
import conversation from "./AssistantConversation.svelte?raw";
import tools from "./PageAssistantTools.svelte?raw";
import chat from "./ChatView.svelte?raw";
import pageContent from "./PageContent.svelte?raw";
import anchor from "../lib/currentBlockAnchor.ts?raw";

describe("Unified conversation components", () => {
  it("shares one transcript, composer and controller between compact and full Chat", () => {
    expect(panel).toContain("<AssistantConversation");
    expect(chat).toContain("<AssistantConversation");
    expect(conversation).toContain("<ChatMessageBubble");
    expect(conversation).toContain("sendAssistantQuestion(thread, thread.context, thread.contextLabel)");
    expect(conversation).toContain("stopAssistantConversation(thread)");
    expect(chat).not.toContain("aiAskStream");
    expect(panel).not.toContain("aiAsk(");
    expect(conversation).not.toContain("shouldUseWebResearchForBlock");
  });

  it("offers exactly one explicit context and mode picker without keyword routing", () => {
    expect(conversation.match(/aria-label="Mode"/g)).toHaveLength(1);
    expect(conversation.match(/aria-label="Context"/g)).toHaveLength(1);
    for (const kind of ["selection", "block", "section", "page", "book", "graph", "none"]) {
      expect(conversation).toContain(`<option value="${kind}"`);
    }
    expect(conversation).not.toContain('type="checkbox"');
    expect(conversation).not.toContain("searchedBlockAnchorFromQuestion");
    expect(conversation).not.toContain("visibleBlockAnchorFromQuestion");
    expect(conversation).toContain("message.contextLabel");
    expect(conversation).toContain("message.mode");
  });

  it("restores explicit block anchors and keeps journal page scope on the focused day", () => {
    expect(panel).toContain("getLatestCurrentBlockAnchor()");
    expect(panel).toContain('preferFocusedPageForPageScope ? askBlockAnchor?.pageId ?? pageId : pageId');
    expect(anchor).toContain('new CustomEvent("page-content-focus-changed"');
    expect(pageContent).toContain("setCurrentBlockAnchor(page.id, blockId)");
    expect(conversation).toContain("Block including children");
    expect(conversation).toContain("Section / Chapter");
    expect(conversation).toContain('info?.isJournal ? "This day"');
  });

  it("uses only identified book roots for whole-book context", () => {
    expect(panel).toContain("info.isBook && (!info.book || info.book.pageId === id)");
    expect(conversation).toContain('context = { kind: "book", pageId: info.book.pageId }');
  });

  it("captures selection explicitly, retains truthful selection errors and disables changes during runs", () => {
    expect(conversation).toContain("destination.selectionError = captured.error");
    expect(conversation).toContain("if (untrack(() => assistantConversationRunning(destination))) return");
    expect(conversation).toContain("blockIds: [...view.selection.blockIds]");
    expect(conversation).toContain("Use selection");
    expect(conversation).toContain('value={view.context.kind} disabled={running}');
    expect(conversation).toContain('value={view.mode} disabled={running}');
  });

  it("expands the identical conversation ID without cancelling streams on unmount", () => {
    expect(conversation).toContain("onExpand?.(thread.id)");
    expect(chat).toContain("getAssistantConversation(id)");
    expect(conversation).not.toContain("onDestroy");
    expect(chat).not.toContain("researchCancel");
    expect(panel).not.toContain("researchCancel");
  });

  it("retains explicit answer summaries, editable merge previews, canonical tags and safe insertion", () => {
    for (const label of ["Summarize answers", "Merge answer with block", "Merged block draft", "Replace captured block", "Insert into page"]) {
      expect(tools).toContain(label);
    }
    expect(tools).toContain("bind:value={mergedDraft}");
    expect(tools).toContain("formatConceptTag(tag.qualified ?? tag.term)");
    expect(tools).toContain('pushUndo({ type: "insert_summary", ...result })');
    expect(tools).toContain("expectedBlocks: source.snapshot");
    expect(tools).toContain("applyWritingChanges(source.graphPath, source.pageId, changes, source.snapshot)");
    expect(tools).toContain("assertResearchSourceGraph(source)");
    expect(tools).toContain("withPageEditorsLocked(source.pageId");
    expect(tools).toContain("await flushPageEditors(source.pageId)");
    expect(tools).not.toContain("await updateBlock(");
  });

  it("keeps only Chat and Notes tabs and exposes other actions under tools", () => {
    expect(panel.match(/role="tab"/g)).toHaveLength(2);
    expect(panel).toContain('let activeTab = $state<"chat" | "notes">("chat")');
    expect(tools).toContain("Page / selection tools");
    expect(tools).toContain("Writing assistance");
    expect(tools).toContain("<AIWritingPanel");
  });

  it("keeps the composer reachable beneath a scrolling transcript in narrow panels", () => {
    expect(panel).toContain("max-width: calc(100vw - 24px)");
    expect(conversation).toMatch(/\.conversation-scroll \{[^}]*flex: 1;[^}]*overflow-y: auto/);
    expect(conversation).toMatch(/\.conversation-controls \{[^}]*flex-shrink: 0/);
    expect(conversation).toContain("@container (max-width: 380px)");
    expect(conversation).toContain("resizeComposer");
    expect(conversation).toContain('role="group" aria-label="Chat composer"');
    expect(conversation).toContain('aria-label="Message"');
    expect(conversation).toContain("Math.floor(paneEl.clientHeight / 2) - (footerEl?.offsetHeight");
  });

  it("restores full Chat focus without collapsing transcript or outside-to-inside drag selections", () => {
    expect(conversation).toContain("selectionIntersectsTranscript(window.getSelection(), scrollEl ?? null)");
    expect(conversation).toContain("if (pointerDown) { refocusPending = true; return; }");
    expect(conversation).toContain('document.addEventListener("mousedown", down)');
    expect(conversation).toContain('document.addEventListener("mouseup", up)');
    expect(conversation).toContain("if (!active || compact || running)");
    expect(conversation).toContain('!(afterRun && focused.matches(".send-button"))');
    expect(conversation).toContain("requestAnimationFrame(() => restoreInputFocus())");
  });
});
