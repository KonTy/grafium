import { writable } from "svelte/store";
import { getGraphInfo } from "./api";
import { flushAllPageEditors, flushPageEditors } from "./editorPersistence";
import { initialState, isStreamPhase, reduce, type StreamEvent, type StreamState } from "./chatStatus";
import type { ChatMessageModel } from "./chatMessage";
import type { ChatTurn } from "./knowledge";
import type { ReadingSelection } from "./readingSelection";
import {
  assistantChat, assistantCancel, assistantContextPageId, copyAssistantContext,
  type AssistantContext, type AssistantMode,
} from "./assistant";

export interface AssistantMessage extends ChatMessageModel {
  contextLabel?: string;
  mode?: AssistantMode;
}

export interface AssistantThread {
  id: string;
  graphPath: string;
  sourcePageId: string | null;
  sourcePageTitle: string;
  sourceIsBook: boolean;
  context: AssistantContext;
  contextLabel: string;
  mode: AssistantMode;
  messages: AssistantMessage[];
  history: ChatTurn[];
  historyContexts: AssistantContext[];
  draft: string;
  selection: ReadingSelection | null;
  selectionError: string | null;
  state: StreamState;
  error: string | null;
  note: string;
  requestId: string | null;
  generation: number;
  pendingIndex: number | null;
  runContext: AssistantContext | null;
}

const conversations = new Map<string, AssistantThread>();
const sourceConversations = new Map<string, string>();
export const assistantConversationChanges = writable(0);
export function updateAssistantConversation(): void { assistantConversationChanges.update((version) => version + 1); }

function defaultContext(pageId: string | null, isBook: boolean): AssistantContext {
  return pageId ? { kind: isBook ? "book" : "page", pageId } : { kind: "none" };
}

function conversation(graphPath: string, pageId: string | null, title: string, isBook: boolean): AssistantThread {
  if (!graphPath.trim()) throw new Error("Open a graph before starting Chat.");
  const key = JSON.stringify([graphPath, pageId]);
  const existingId = sourceConversations.get(key);
  const existing = existingId ? conversations.get(existingId) : undefined;
  if (existing) {
    const previousTitle = existing.sourcePageTitle;
    existing.sourcePageTitle = title || previousTitle;
    existing.sourceIsBook = isBook;
    if (existing.contextLabel === previousTitle) existing.contextLabel = existing.sourcePageTitle;
    return existing;
  }
  const thread: AssistantThread = {
    id: crypto.randomUUID(), graphPath, sourcePageId: pageId, sourcePageTitle: title, sourceIsBook: isBook,
    context: defaultContext(pageId, isBook), contextLabel: pageId ? title : "No notes",
    mode: "answer", messages: [], history: [], historyContexts: [], draft: "", selection: null, selectionError: null,
    state: initialState(), error: null, note: "", requestId: null, generation: 0, pendingIndex: null, runContext: null,
  };
  conversations.set(thread.id, thread);
  sourceConversations.set(key, thread.id);
  return thread;
}

export function getGlobalConversation(graphPath: string): AssistantThread {
  return conversation(graphPath, null, "", false);
}

export function getSourceConversation(graphPath: string, pageId: string, pageTitle: string, isBook = false): AssistantThread {
  if (!pageId.trim()) throw new Error("Open a page, book, or journal day first.");
  return conversation(graphPath, pageId, pageTitle, isBook);
}

export function getAssistantConversation(id: string): AssistantThread | undefined { return conversations.get(id); }

export function assistantConversationRunning(thread: AssistantThread): boolean {
  return thread.state.kind === "active" || thread.state.kind === "stalled";
}

function dispatch(thread: AssistantThread, event: StreamEvent): void {
  thread.state = reduce(thread.state, event);
  updateAssistantConversation();
}

/** The controller, not either view, owns snapshots, listeners and pending work. */
export async function sendAssistantQuestion(
  thread: AssistantThread, context = thread.context, contextLabel = thread.contextLabel,
): Promise<void> {
  const question = thread.draft.trim();
  if (!question || assistantConversationRunning(thread)) return;
  let frozenContext: AssistantContext;
  try {
    frozenContext = copyAssistantContext(context);
  } catch (error) {
    thread.error = String(error);
    updateAssistantConversation();
    return;
  }
  // "No notes" must not smuggle earlier note-backed answers into the model through history.
  const eligible = thread.history.map((turn, index) => ({ turn, context: thread.historyContexts[index] }))
    .filter((entry) => frozenContext.kind !== "none" || entry.context?.kind === "none");
  const history = eligible.map(({ turn }) => ({ ...turn }));
  const historyContexts = eligible.map((entry) => copyAssistantContext(entry.context ?? thread.context));
  const mode = thread.mode;
  const generation = ++thread.generation;
  const requestId = crypto.randomUUID();
  const current = () => thread.generation === generation;
  const live = () => current() && assistantConversationRunning(thread);
  const assistantIndex = thread.messages.length + 1;
  thread.messages.push(
    { role: "user", content: question, contextLabel, mode },
    { role: "assistant", content: "", contextLabel, mode, webResearch: mode !== "answer" },
  );
  thread.pendingIndex = assistantIndex;
  thread.draft = "";
  thread.error = null;
  thread.note = frozenContext.kind === "none" ? "" : "Saving source edits...";
  thread.runContext = frozenContext;
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
      if (!thread.draft) thread.draft = question;
      dispatch(thread, { type: "error", at: Date.now(), message });
    } else {
      if (!answer.content.trim()) {
        finish("The model returned no answer. Try again or check your provider in Settings.");
        return;
      }
      thread.history = [...history, { role: "user", content: question }, { role: "assistant", content: answer.content }];
      thread.historyContexts = [...historyContexts, copyAssistantContext(frozenContext), copyAssistantContext(frozenContext)];
      dispatch(thread, { type: "done", at: Date.now() });
    }
    // Keep what the run did next to the answer it produced; thread.state is
    // reset by the next question.
    answer.steps = thread.state.steps.map((step) => ({ ...step }));
    updateAssistantConversation();
  };
  try {
    if ((await getGraphInfo()).path !== thread.graphPath) throw new Error("The graph changed. Return to the original graph before asking.");
    if (!live()) return;
    if (frozenContext.kind === "graph" || frozenContext.kind === "book") {
      await flushAllPageEditors();
    } else {
      const pageId = assistantContextPageId(frozenContext);
      if (pageId) await flushPageEditors(pageId);
    }
    if (!live()) return;
    if ((await getGraphInfo()).path !== thread.graphPath) throw new Error("The graph changed while saving source edits. Try again.");
    if (!live()) return;
    thread.note = "";
    updateAssistantConversation();
    await assistantChat({ question, requestId, graphPath: thread.graphPath, context: frozenContext, history, mode }, {
      shouldContinue: live,
      onChunk(delta) {
        if (!live()) return;
        thread.messages[assistantIndex].content += delta;
        dispatch(thread, { type: "delta", chars: delta.length, at: Date.now() });
      },
      onPhase(phase) {
        if (!live()) return;
        if (isStreamPhase(phase)) {
          dispatch(thread, { type: "phase", phase, at: Date.now() });
        } else {
          console.warn("[chat] Unknown progress phase:", phase);
          dispatch(thread, { type: "note", at: Date.now() });
        }
      },
      onNote(note) {
        if (!live()) return;
        thread.note = note;
        dispatch(thread, { type: "note", at: Date.now(), text: note });
      },
      onSources(sources) {
        if (!current()) return;
        thread.messages[assistantIndex].sources = sources;
        updateAssistantConversation();
      },
      onWebSources(sources) {
        if (!current()) return;
        thread.messages[assistantIndex].webSources = sources;
        updateAssistantConversation();
      },
      onDone: () => finish(),
      onError: (message) => finish(message),
    });
    if (live()) finish("The answer stream ended without a completion event. Try again.");
  } catch (error) {
    finish(String(error));
  }
}

export async function stopAssistantConversation(thread: AssistantThread): Promise<void> {
  const requestId = thread.requestId;
  if (!assistantConversationRunning(thread)) return;
  const generation = ++thread.generation;
  thread.requestId = null;
  thread.pendingIndex = null;
  thread.note = "";
  dispatch(thread, { type: "cancel", at: Date.now() });
  if (!requestId) return;
  try {
    await assistantCancel(requestId);
  } catch (error) {
    if (thread.generation !== generation) return;
    thread.error = `Could not confirm Stop: ${String(error)}`;
    updateAssistantConversation();
  }
}

export function newAssistantConversation(thread: AssistantThread): void {
  if (assistantConversationRunning(thread)) return;
  thread.generation++;
  thread.messages = [];
  thread.history = [];
  thread.historyContexts = [];
  thread.draft = "";
  thread.error = null;
  thread.note = "";
  thread.runContext = null;
  thread.pendingIndex = null;
  thread.context = defaultContext(thread.sourcePageId, thread.sourceIsBook);
  thread.contextLabel = thread.sourcePageId ? thread.sourcePageTitle : "No notes";
  thread.mode = "answer";
  thread.selection = null;
  thread.selectionError = null;
  thread.state = initialState();
  updateAssistantConversation();
}

export async function stopAllAssistantConversations(): Promise<void> {
  await Promise.all([...conversations.values()].map(stopAssistantConversation));
}
