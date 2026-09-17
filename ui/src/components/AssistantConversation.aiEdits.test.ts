import { describe, expect, it } from "vitest";
import conversation from "./AssistantConversation.svelte?raw";
import card from "./AIEditPlanCard.svelte?raw";

describe("Chat edit actions", () => {
  it("only plans edits for instructions and still answers when planning fails", () => {
    // Planning costs a second model round trip. Questions must not pay for it.
    expect(conversation).toContain("if (looksLikeEditRequest(request) && (await proposeEdits(request))) return;");
    expect(conversation).toContain("await sendAssistantQuestion(thread, thread.context, thread.contextLabel);");
    // A thrown planner returns false rather than propagating, so `send` falls
    // through to a normal answer instead of leaving the user with silence.
    expect(conversation).toMatch(/catch \(cause\) \{\s*console\.error\("Could not plan edits:", cause\);\s*return false;/);
    expect(conversation).toContain("if (!plan.actions.length) return false;");
  });

  it("reuses the previous answer verbatim instead of asking the model to retype it", () => {
    expect(conversation).toContain("hydratePlan(parseEditPlan(response.answer), answer)");
    expect(conversation).toContain('find((message) => message.role === "assistant")?.content');
  });

  it("writes nothing until the user applies a reviewed plan", () => {
    expect(conversation).toContain("{#if pendingPlan}");
    expect(conversation).toContain("<AIEditPlanCard");
    expect(conversation).toContain("onApply={applyPlan} onDismiss={dismissPlan}");
    expect(card).toContain("bind:value={");
    expect(card).toContain("onApply");
    // The card is the only route to a write; nothing else may reach the applier.
    expect(card).not.toContain("applyEditPlan");
    expect(conversation.match(/applyEditPlan\(/g)).toHaveLength(1);
  });

  it("makes applied changes undoable and refreshes the pages it touched", () => {
    expect(conversation).toContain("await runUndoOperation(() =>");
    expect(conversation).toContain("applyEditPlan({ actions }, { blockTarget })");
    expect(conversation).toContain('new CustomEvent("page-content-reload-blocks"');
    expect(conversation).toContain("summarizeApplyResult(result)");
  });

  it("flushes and locks the open editor only when a block is being rewritten", () => {
    expect(conversation).toContain("withPageEditorsLocked(blockTarget.pageId");
    expect(conversation).toContain("await flushPageEditors(blockTarget.pageId)");
    expect(conversation).toContain('if (!actions.some((action) => action.type === "replace_block")) return null;');
    // Optimistic concurrency: a rewrite carries the snapshot it read, so a
    // concurrent edit aborts the write rather than silently overwriting it.
    expect(conversation).toContain("snapshot: source.snapshot");
  });

  it("hands a dismissed request back to the composer to be rephrased", () => {
    expect(conversation).toContain("thread.draft = pendingPlan.request;");
    expect(conversation).toContain("pendingPlan = null;");
  });

  it("reports partial failures instead of claiming success", () => {
    expect(conversation).toContain("if (result.applied.length) {");
    expect(conversation).toContain('planError = result.errors.join("; ") || "Nothing was applied.";');
  });
});
