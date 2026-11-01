import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { IqNorms, IqPackSummary, IqResult } from "../lib/iq";

const api = vi.hoisted(() => ({ start: vi.fn(), answer: vi.fn(), finish: vi.fn() }));
vi.mock("../lib/iq", () => ({ startIqSession: api.start, answerIqItem: api.answer, finishIqSession: api.finish }));
import IqAssessment from "./IqAssessment.svelte";

let component: ReturnType<typeof mount> | undefined;
const pack: IqPackSummary = { id: "p", title: "Practice pack", itemCount: 2, norms: null };
const session = { id: "s", graphPath: "/graph", title: pack.title, questions: ["Question one", "Question two"], expiresAt: null };
const result: IqResult = {
  sessionId: "s", title: pack.title, correct: 1, total: 2, unanswered: 1,
  percentage: 50, elapsedSeconds: 3, estimatedIq: null,
  iqUnavailableReason: "No population norms.", normCitation: null,
  items: [{ question: "Question one", answer: "answer", acceptedAnswers: ["answer"], explanation: "Hidden explanation", correct: true }],
};
function button(text: string): HTMLButtonElement {
  const button = Array.from(document.querySelectorAll("button")).find(b => b.textContent?.includes(text));
  if (!button) throw new Error(`Missing button: ${text}`);
  return button;
}
function check(selector = "input[type=checkbox]") {
  document.querySelector<HTMLInputElement>(selector)!.click();
  flushSync();
}
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("IQ-marked assessment flow", () => {
  it("requires consent, withholds answers, submits/skips and shows reasoning-only results", async () => {
    api.start.mockResolvedValue(session);
    api.answer.mockResolvedValue(undefined);
    api.finish.mockResolvedValue(result);
    component = mount(IqAssessment, { target: document.body, props: { pack, onExit: vi.fn() } });
    expect(button("Start assessment").disabled).toBe(true);
    expect(document.body.textContent).toContain("reasoning score, not IQ");
    check();
    button("Start assessment").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Question one"));
    expect(document.body.textContent).not.toContain("Hidden explanation");
    const input = document.querySelector<HTMLTextAreaElement>("textarea")!;
    input.value = "answer";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.body.textContent).toContain("Question two"));
    expect(api.answer).toHaveBeenLastCalledWith(session, 0, "answer");
    button("Skip").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Reasoning score: 1 / 2"));
    expect(api.answer).toHaveBeenLastCalledWith(session, 1, "");
    expect(document.body.textContent).not.toContain("Approximate IQ:");
    expect(document.body.textContent).toContain("Hidden explanation");
  });

  it("keeps the current answer on save failure and exposes retry errors", async () => {
    api.start.mockResolvedValue(session);
    api.answer.mockRejectedValueOnce(new Error("disk full")).mockResolvedValue(undefined);
    component = mount(IqAssessment, { target: document.body, props: { pack, onExit: vi.fn() } });
    check(); button("Start assessment").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Question one"));
    const input = document.querySelector<HTMLTextAreaElement>("textarea")!;
    input.value = "draft";
    input.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
    document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.querySelector("[role=alert]")?.textContent).toContain("disk full"));
    expect(input.value).toBe("draft");
    expect(document.body.textContent).toContain("Question one");
    expect(api.finish).not.toHaveBeenCalled();
  });

  it("finishes expired sessions without submitting a late answer", async () => {
    api.start.mockResolvedValue({ ...session, expiresAt: Date.now() - 1 });
    api.finish.mockResolvedValue(result);
    component = mount(IqAssessment, { target: document.body, props: { pack, onExit: vi.fn() } });
    check(); button("Start assessment").click();
    await vi.waitFor(() => expect(api.finish).toHaveBeenCalledOnce());
    expect(api.answer).not.toHaveBeenCalled();
  });

  it("shows estimates only from the backend result, with an unverified-norm warning", async () => {
    const norms: IqNorms = {
      citation: "https://example.org/fixture", population: "Synthetic fixture", sampleSize: 100,
      ageMin: 18, ageMax: 65, language: "en", testVersion: "1", itemSetHash: "hash",
      timeLimitSeconds: null, instructions: "Unaided.", zScores: [-1, 0, 1],
    };
    api.start.mockResolvedValue({ ...session, questions: ["Question one"] });
    api.answer.mockResolvedValue(undefined);
    api.finish.mockResolvedValue({ ...result, estimatedIq: 115, iqUnavailableReason: null, normCitation: norms.citation });
    component = mount(IqAssessment, { target: document.body, props: { pack: { ...pack, norms }, onExit: vi.fn() } });
    const age = document.querySelector<HTMLInputElement>('input[type="number"]')!;
    age.value = "30"; age.dispatchEvent(new Event("input", { bubbles: true }));
    const language = document.querySelector<HTMLInputElement>('input[placeholder="en"]')!;
    language.value = "en"; language.dispatchEvent(new Event("input", { bubbles: true }));
    for (const checkbox of document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')) checkbox.click();
    flushSync(); button("Start assessment").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Question one"));
    expect(api.start).toHaveBeenCalledWith("p", { age: 30, language: "en", unseen: true });
    button("Skip").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Approximate IQ: 115"));
    expect(document.body.textContent).toContain("unverified, pack-supplied");
    expect(document.querySelector("section")?.dataset.helpContext).toBe("iq");
  });
});
