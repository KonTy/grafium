import { describe, expect, it } from "vitest";
import {
  actionBlocks,
  buildPlannerPrompt,
  describeAction,
  hydratePlan,
  looksLikeEditRequest,
  normalizeTag,
  parseEditPlan,
  resolveJournalDate,
  splitIntoBlocks,
  type EditAction,
} from "./aiActions";

const TODAY = new Date(2026, 8, 17); // 2026-09-17, local

function action(overrides: Partial<EditAction> = {}): EditAction {
  return { type: "append_to_journal", title: "Notes", content: "Body", source: "text", tags: [], date: "2026-09-17", ...overrides };
}

describe("edit-request detection", () => {
  it("recognises the instructions people actually type", () => {
    for (const text of [
      "add the above answer to the journal and mark it as [[health/supplements]]",
      "save this answer as a new page and find links that it can connect to",
      "please put that in today's journal",
      "create a page called Supplements",
      "file this under [[health]]",
      "log this as a task",
      "turn the answer into a new note",
    ]) {
      expect(looksLikeEditRequest(text), text).toBe(true);
    }
  });

  it("leaves ordinary questions to the normal answer path", () => {
    for (const text of [
      "what is the supplement he is talking about?",
      "how do I add a page in grafium?",
      "why does collagen production fall with age",
      "summarise the last three paragraphs",
      "tell me more",
      "",
    ]) {
      expect(looksLikeEditRequest(text), text).toBe(false);
    }
  });

  it("treats a trailing question mark as a question even when it opens with a verb", () => {
    expect(looksLikeEditRequest("add salt to the recipe?")).toBe(false);
  });

  it("ignores very long messages, which are pasted text rather than commands", () => {
    expect(looksLikeEditRequest(`add ${"x".repeat(700)}`)).toBe(false);
  });
});

describe("tag normalisation", () => {
  it("strips the syntax people paste and keeps the namespace", () => {
    expect(normalizeTag("[[health/supplements]]")).toBe("health/supplements");
    expect(normalizeTag("#health")).toBe("health");
    expect(normalizeTag('"health/supplements"')).toBe("health/supplements");
    expect(normalizeTag("  health / supplements  ")).toBe("health/supplements");
  });

  it("collapses separator noise so one namespace cannot become two pages", () => {
    expect(normalizeTag("health//supplements")).toBe("health/supplements");
    expect(normalizeTag("/health/supplements/")).toBe("health/supplements");
  });
});

describe("journal dates", () => {
  it("resolves relative words against the local calendar", () => {
    expect(resolveJournalDate("today", TODAY)).toBe("2026-09-17");
    expect(resolveJournalDate("yesterday", TODAY)).toBe("2026-09-16");
    expect(resolveJournalDate("tomorrow", TODAY)).toBe("2026-09-18");
    expect(resolveJournalDate(undefined, TODAY)).toBe("2026-09-17");
  });

  it("passes through explicit dates and rejects anything else", () => {
    expect(resolveJournalDate("2024-01-04", TODAY)).toBe("2024-01-04");
    expect(resolveJournalDate("next week", TODAY)).toBeNull();
    expect(resolveJournalDate("soon", TODAY)).toBeNull();
  });

  it("crosses month and year boundaries correctly", () => {
    expect(resolveJournalDate("tomorrow", new Date(2026, 11, 31))).toBe("2027-01-01");
    expect(resolveJournalDate("yesterday", new Date(2026, 0, 1))).toBe("2025-12-31");
  });
});

describe("splitting an answer into blocks", () => {
  it("gives each list item its own bullet without doubling the marker", () => {
    expect(splitIntoBlocks("- one\n- two\n1. three")).toEqual(["one", "two", "three"]);
  });

  it("keeps paragraphs together and separates them on blank lines", () => {
    expect(splitIntoBlocks("first line\nstill first\n\nsecond")).toEqual(["first line\nstill first", "second"]);
  });

  it("never splits a fenced code block across bullets", () => {
    const blocks = splitIntoBlocks("intro\n\n```js\nconst a = 1;\n\nconst b = 2;\n```\n\nafter");
    expect(blocks).toEqual(["intro", "```js\nconst a = 1;\n\nconst b = 2;\n```", "after"]);
  });

  it("keeps headings as their own bullet", () => {
    expect(splitIntoBlocks("## Key Takeaway\ntext")).toEqual(["## Key Takeaway", "text"]);
  });

  it("returns nothing for empty input", () => {
    expect(splitIntoBlocks("   \n\n  ")).toEqual([]);
  });
});

describe("rendering an action into blocks", () => {
  it("puts the tags on the parent bullet so the link is filed once", () => {
    const { parent, children } = actionBlocks(action({ title: "Supplement notes", tags: ["health/supplements"], content: "- one\n- two" }));
    expect(parent).toBe("Supplement notes [[health/supplements]]");
    expect(children).toEqual(["one", "two"]);
  });

  it("promotes the first idea when the model gave no title, so tags never land on a blank line", () => {
    const { parent, children } = actionBlocks(action({ title: "", tags: ["health"], content: "First idea\n\nSecond idea" }));
    expect(parent).toBe("First idea [[health]]");
    expect(children).toEqual(["Second idea"]);
  });

  it("marks tasks with TODO so the task board picks them up", () => {
    const { parent } = actionBlocks(action({ type: "create_task", title: "Buy magnesium", tags: ["health"] }));
    expect(parent).toBe("TODO Buy magnesium [[health]]");
  });
});

describe("plan parsing", () => {
  it("reads a clean response", () => {
    const plan = parseEditPlan('{"actions":[{"type":"append_to_journal","title":"Supplements","source":"answer","content":"","tags":["health/supplements"],"date":"today"}]}');
    expect(plan.actions).toHaveLength(1);
    expect(plan.actions[0].type).toBe("append_to_journal");
    expect(plan.actions[0].tags).toEqual(["health/supplements"]);
    expect(plan.actions[0].source).toBe("answer");
  });

  it("survives fences, prose and reasoning tags around the JSON", () => {
    const plan = parseEditPlan(
      '<think>the user wants a journal entry</think>\nSure! Here is the plan:\n```json\n{"actions":[{"type":"create_page","title":"Supplements","page":"Supplements","source":"answer","tags":[],"findLinks":true}]}\n```\nHope that helps.'
    );
    expect(plan.actions).toHaveLength(1);
    expect(plan.actions[0].type).toBe("create_page");
    expect(plan.actions[0].findLinks).toBe(true);
  });

  it("accepts a bare array or a single bare action", () => {
    expect(parseEditPlan('[{"type":"find_links","page":"Supplements"}]').actions).toHaveLength(1);
    expect(parseEditPlan('{"type":"find_links","page":"Supplements"}').actions).toHaveLength(1);
  });

  it("reports no actions for an ordinary question rather than throwing", () => {
    expect(parseEditPlan('{"actions":[]}').actions).toEqual([]);
    expect(parseEditPlan("I'm not sure what you mean.").actions).toEqual([]);
    expect(parseEditPlan("").actions).toEqual([]);
  });

  it("drops actions with an unknown type instead of failing the whole plan", () => {
    const plan = parseEditPlan('{"actions":[{"type":"delete_everything"},{"type":"append_to_journal","title":"Keep"}]}');
    expect(plan.actions).toHaveLength(1);
    expect(plan.actions[0].title).toBe("Keep");
  });

  it("drops page actions that name no page, which would otherwise write to nowhere", () => {
    expect(parseEditPlan('{"actions":[{"type":"append_to_page","title":"x"}]}').actions).toEqual([]);
    expect(parseEditPlan('{"actions":[{"type":"add_tags","page":"Notes","tags":[]}]}').actions).toEqual([]);
  });

  it("assumes the previous answer when the model omits source but writes no content", () => {
    const plan = parseEditPlan('{"actions":[{"type":"append_to_journal","title":"Notes"}]}');
    expect(plan.actions[0].source).toBe("answer");
  });

  it("keeps the model's own text when it supplied content", () => {
    const plan = parseEditPlan('{"actions":[{"type":"append_to_journal","title":"Notes","content":"Custom body"}]}');
    expect(plan.actions[0].source).toBe("text");
    expect(plan.actions[0].content).toBe("Custom body");
  });

  it("normalises tags the model wrapped in brackets and de-duplicates them", () => {
    const plan = parseEditPlan('{"actions":[{"type":"append_to_journal","title":"n","tags":["[[health]]","health","#health/supplements"]}]}');
    expect(plan.actions[0].tags).toEqual(["health", "health/supplements"]);
  });

  it("defaults a nonsense date to today rather than inventing a page title", () => {
    const plan = parseEditPlan('{"actions":[{"type":"append_to_journal","title":"n","date":"sometime next week"}]}');
    expect(plan.actions[0].date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });
});

describe("hydrating a plan", () => {
  it("substitutes the real answer so a long answer is never retyped by the model", () => {
    const plan = hydratePlan({ actions: [action({ source: "answer", content: "" })] }, "The real answer text");
    expect(plan.actions[0].content).toBe("The real answer text");
  });

  it("leaves the model's own text alone", () => {
    const plan = hydratePlan({ actions: [action({ source: "text", content: "Mine" })] }, "The real answer text");
    expect(plan.actions[0].content).toBe("Mine");
  });

  it("drops a content action left with nothing to write", () => {
    expect(hydratePlan({ actions: [action({ source: "answer", content: "", title: "" })] }, "   ").actions).toEqual([]);
  });

  it("keeps actions that need no body", () => {
    const plan = hydratePlan({ actions: [action({ type: "find_links", page: "Notes", title: "", content: "", source: "answer" })] }, "");
    expect(plan.actions).toHaveLength(1);
  });
});

describe("plan descriptions", () => {
  it("says plainly what will happen, so Apply is an informed click", () => {
    expect(describeAction(action({ tags: ["health/supplements"] }))).toBe("Add to your 2026-09-17 journal under [[health/supplements]]");
    expect(describeAction(action({ type: "create_page", page: "Supplements", tags: [], findLinks: true })))
      .toBe("Create the page “Supplements”, then look for links");
    expect(describeAction(action({ type: "add_tags", page: "Notes", tags: ["health"] }))).toBe("Tag “Notes” with [[health]]");
    expect(describeAction(action({ type: "replace_block" }))).toBe("Replace the block this conversation is about");
  });
});

describe("planner prompt", () => {
  it("asks for the answer to be reused rather than retyped", () => {
    const prompt = buildPlannerPrompt("add this to my journal", "A long answer", true);
    expect(prompt).toContain("Never retype or summarise the answer");
    expect(prompt).toContain("A long answer");
    expect(prompt).toContain("add this to my journal");
  });

  it("forbids replace_block when the conversation has no block", () => {
    expect(buildPlannerPrompt("x", "y", false)).toContain('never use "replace_block"');
    expect(buildPlannerPrompt("x", "y", true)).not.toContain('never use "replace_block"');
  });

  it("truncates a huge answer so the planning call stays cheap", () => {
    const prompt = buildPlannerPrompt("x", "y".repeat(5000), true);
    expect(prompt).toContain("…");
    expect(prompt.length).toBeLessThan(4000);
  });
});
