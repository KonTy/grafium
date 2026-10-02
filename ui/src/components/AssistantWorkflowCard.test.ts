import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import AssistantWorkflowCard from "./AssistantWorkflowCard.svelte";
import type { WorkflowProposal } from "../lib/assistantWorkflows";

let component: ReturnType<typeof mount> | undefined;
const id = "12345678-1234-4234-8234-123456789abc";
let proposal: WorkflowProposal;
const button = (name: string) => [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === name)!;
const edit = (update: Partial<Pick<WorkflowProposal, "title" | "content">>) => { Object.assign(proposal, update); };
beforeEach(() => {
  proposal = { request: "Group tasks", title: "Task review", content: `## Errands\n- ((${id}))\n<img src="https://never-fetch.test/pixel">`,
    summary: "1 of 1 open tasks considered.", snapshot: {
      token: "token", graphPath: "/fixture", kind: "tasks", sourcePageId: null, sourcePageTitle: null,
      pages: [], tasks: [{ id, pageId: "source", pageTitle: "My original note", content: "TODO call the clinic",
        state: "TODO" }], coverage: "1 open task",
    } };
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.restoreAllMocks();
});

describe("reviewable ASK drafts", () => {
  it("shows readable live-reference labels without executing generated HTML or saving anything", () => {
    const apply = vi.fn();
    component = mount(AssistantWorkflowCard, { target: document.body, props: { proposal, onApply: apply, onDismiss: vi.fn(), onEdit: edit } });
    flushSync();
    expect(document.body.textContent).toContain("TODO call the clinic");
    expect(document.body.textContent).toContain("Nothing is saved");
    expect(document.querySelector("img")).toBeNull();
    expect(apply).not.toHaveBeenCalled();
    button("Apply").click();
    expect(apply).toHaveBeenCalledOnce();
  });
  it("edits the actual reviewed body rather than applying an invisible original answer", () => {
    component = mount(AssistantWorkflowCard, { target: document.body, props: { proposal, onApply: vi.fn(), onDismiss: vi.fn(), onEdit: edit } });
    flushSync(); button("Edit draft").click(); flushSync();
    const text = document.querySelector("textarea")!;
    text.value = "My corrected group"; text.dispatchEvent(new Event("input", { bubbles: true }));
    expect(proposal.content).toBe("My corrected group");
    expect(document.body.textContent).toContain("Draft text");
  });
  it("opens the exact original block through the existing navigation event", () => {
    const navigate = vi.fn();
    window.addEventListener("navigate-page", navigate);
    try {
      component = mount(AssistantWorkflowCard, { target: document.body, props: { proposal, onApply: vi.fn(), onDismiss: vi.fn(), onEdit: edit } });
      flushSync(); button("TODO call the clinic").click();
      expect(navigate.mock.calls[0][0].detail).toEqual({ pageName: "My original note", targetBlockId: id });
    } finally { window.removeEventListener("navigate-page", navigate); }
  });
  it("keeps source preservation and errors visible while applying", () => {
    proposal.snapshot.kind = "rewrite"; proposal.snapshot.sourcePageTitle = "Rough notes";
    proposal.content = "TODO quoted draft";
    component = mount(AssistantWorkflowCard, { target: document.body, props: {
      proposal, applying: true, error: "Source changed", onApply: vi.fn(), onDismiss: vi.fn(), onEdit: edit,
    } });
    flushSync();
    expect(document.body.textContent).toContain("Original notes stay unchanged");
    expect(document.body.textContent).toContain("creates no extra tasks");
    expect(document.querySelector('[role="alert"]')?.textContent).toBe("Source changed");
    expect(button("Applying…").disabled).toBe(true);
    expect(button("Dismiss").disabled).toBe(true);
  });
});
