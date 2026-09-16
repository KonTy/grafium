import { writable } from "svelte/store";
import { getGraphInfo } from "./api";
import { flushPageEditors } from "./editorPersistence";
import { initialState, reduce, type StreamEvent, type StreamPhase, type StreamState } from "./chatStatus";
import type { ChatMessageModel } from "./chatMessage";
import type { ChatTurn } from "./knowledge";
import type { ReadingSelection } from "./readingSelection";
import { researchScoped, researchCancel, type ResearchScope, type ResearchTarget, type ResearchWebMode } from "./research";

export interface ResearchThread {
  graphPath: string;
  pageId: string;
  pageTitle: string;
  messages: ChatMessageModel[];
  history: ChatTurn[];
  draft: string;
  scope: ResearchScope;
  internet: boolean;
  research: boolean;
  selection: ReadingSelection | null;
  selectionError: string | null;
  state: StreamState;
  error: string | null;
  note: string;
  requestId: string | null;
  generation: number;
  pendingIndex: number | null;
  runTarget: ResearchTarget | null;
}

const threads = new Map<string, ResearchThread>();
export const researchThreadChanges = writable(0);
export function updateResearchThread(): void { researchThreadChanges.update((version) => version + 1); }

export function getResearchThread(graphPath: string, pageId: string, pageTitle: string): ResearchThread {
  const key = JSON.stringify([graphPath, pageId]);
  let thread = threads.get(key);
  if (!thread) {
    thread = {
      graphPath, pageId, pageTitle, messages: [], history: [], draft: "", scope: "page",
      internet: false, research: false, selection: null, selectionError: null,
      state: initialState(), error: null, note: "", requestId: null, generation: 0, pendingIndex: null, runTarget: null,
    };
    threads.set(key, thread);
  }
  thread.pageTitle = pageTitle || thread.pageTitle;
  return thread;
}

export function researchThreadRunning(thread: ResearchThread): boolean {
  return thread.state.kind === "active" || thread.state.kind === "stalled";
}

export function researchWebMode(internet: boolean, research: boolean): ResearchWebMode {
  return internet ? research ? "research" : "search" : "off";
}

export function researchTarget(
  pageId: string, scope: ResearchScope, blockId: string | null,
  sectionBlockId: string | null, selection: ReadingSelection | null,
): ResearchTarget {
  if (!pageId) throw new Error("Open a page or select a journal day first.");
  if (scope === "selection") {
    if (!selection || selection.pageId !== pageId || !selection.blockIds.length || !selection.text.trim()) {
      throw new Error("Select text in this page or journal day first.");
    }
    return { pageId, scope, selection: { blockIds: [...selection.blockIds], text: selection.text } };
  }
  if (scope === "block" || scope === "section") {
    const anchor = scope === "section" ? sectionBlockId : blockId;
    if (!anchor) throw new Error(`Choose a ${scope} in this page first.`);
    return { pageId, scope, blockId: anchor };
  }
  return { pageId, scope: "page" };
}

function dispatch(thread: ResearchThread, event: StreamEvent) {
  thread.state = reduce(thread.state, event);
  updateResearchThread();
}

/** Owns the run independently of any mounted panel or its currently visible source. */
export async function sendResearchQuestion(thread: ResearchThread, target: ResearchTarget): Promise<void> {
  const question = thread.draft.trim();
  if (!question || researchThreadRunning(thread)) return;
  if (target.pageId !== thread.pageId) throw new Error("The conversation source changed. Try again.");
  const frozenTarget = JSON.parse(JSON.stringify(target)) as ResearchTarget;
  const history = thread.history.map((turn) => ({ ...turn }));
  const webMode = researchWebMode(thread.internet, thread.research);
  const generation = ++thread.generation;
  const requestId = crypto.randomUUID();
  const current = () => thread.generation === generation;
  const live = () => current() && researchThreadRunning(thread);
  const assistantIndex = thread.messages.length + 1;
  thread.messages.push({ role: "user", content: question }, { role: "assistant", content: "", webResearch: webMode !== "off" });
  thread.pendingIndex = assistantIndex;
  thread.draft = "";
  thread.error = null;
  thread.note = "Saving the source…";
  thread.runTarget = frozenTarget;
  thread.requestId = requestId;
  dispatch(thread, { type: "start", at: Date.now() });
  const finish = (message?: string) => {
    if (!live()) return;
    thread.requestId = null;
    thread.pendingIndex = null;
    thread.note = "";
    const answer = thread.messages[assistantIndex];
    if (message) {
      thread.error = message;
      dispatch(thread, { type: "error", at: Date.now(), message });
    } else {
      if (!answer.content.trim()) {
        finish("The model returned no answer. Try again or check your provider in Settings.");
        return;
      }
      thread.history.push({ role: "user", content: question }, { role: "assistant", content: answer.content });
      dispatch(thread, { type: "done", at: Date.now() });
    }
  };
  try {
    if ((await getGraphInfo()).path !== thread.graphPath) throw new Error("The graph changed. Return to the source graph before asking.");
    if (!live()) return;
    await flushPageEditors(frozenTarget.pageId);
    if (!live()) return;
    if ((await getGraphInfo()).path !== thread.graphPath) throw new Error("The graph changed while saving the source. Try again.");
    if (!live()) return;
    thread.note = "";
    updateResearchThread();
    await researchScoped({ question, requestId, graphPath: thread.graphPath, target: frozenTarget, history, webMode }, {
      shouldContinue: live,
      onChunk(delta) {
        if (!live()) return;
        thread.messages[assistantIndex].content += delta;
        dispatch(thread, { type: "delta", chars: delta.length, at: Date.now() });
      },
      onPhase(phase) { if (live()) dispatch(thread, { type: "phase", phase: phase as StreamPhase, at: Date.now() }); },
      onNote(note) {
        if (!live()) return;
        thread.note = note;
        dispatch(thread, { type: "note", at: Date.now() });
      },
      onSources(sources) {
        if (!current()) return;
        thread.messages[assistantIndex].sources = sources;
        updateResearchThread();
      },
      onWebSources(sources) {
        if (!current()) return;
        thread.messages[assistantIndex].webSources = sources;
        updateResearchThread();
      },
      onDone: () => finish(),
      onError: (message) => finish(message),
    });
    if (live()) finish("The research stream ended without a completion event. Try again.");
  } catch (error) {
    finish(String(error));
  }
}

export async function stopResearchThread(thread: ResearchThread): Promise<void> {
  const requestId = thread.requestId;
  if (!researchThreadRunning(thread)) return;
  const generation = ++thread.generation;
  thread.requestId = null;
  thread.pendingIndex = null;
  thread.note = "";
  dispatch(thread, { type: "cancel", at: Date.now() });
  if (!requestId) return;
  try {
    await researchCancel(requestId);
  } catch (error) {
    if (thread.generation !== generation) return;
    thread.error = `Could not confirm Stop: ${String(error)}`;
    updateResearchThread();
  }
}

export function newResearchConversation(thread: ResearchThread): void {
  if (researchThreadRunning(thread)) return;
  thread.generation++;
  thread.messages = [];
  thread.history = [];
  thread.draft = "";
  thread.error = null;
  thread.note = "";
  thread.runTarget = null;
  thread.pendingIndex = null;
  thread.state = initialState();
  updateResearchThread();
}
