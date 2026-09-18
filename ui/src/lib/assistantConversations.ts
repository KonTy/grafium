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
import {
  acquireChatSlot, cancelChatSlot, currentChatConcurrency, deriveChatTitle, deleteChatThread,
  listChatThreads, loadChatThread, parseContext, parseMode, queueWaitLabel,
  renameChatThread, saveChatThread, serializeContext, shouldApplyChatTitle, suggestChatTitle,
  type StoredChatMessage,
} from "./chatStore";

export interface AssistantMessage extends ChatMessageModel {
  contextLabel?: string;
  mode?: AssistantMode;
}

export interface AssistantThread {
  id: string;
  /** What the user sees in the switcher. Derived from the first question
   *  unless they renamed it, in which case `titleIsCustom` protects it. */
  title: string;
  titleIsCustom: boolean;
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
  /** How many requests are ahead of this one, or 0 when it isn't waiting. */
  queuePosition: number;
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
  const thread = blankThread(graphPath, pageId, title, isBook);
  conversations.set(thread.id, thread);
  sourceConversations.set(key, thread.id);
  return thread;
}

function blankThread(
  graphPath: string, pageId: string | null, title: string, isBook: boolean,
  id: string = crypto.randomUUID(),
): AssistantThread {
  return {
    id, title: "", titleIsCustom: false,
    graphPath, sourcePageId: pageId, sourcePageTitle: title, sourceIsBook: isBook,
    context: defaultContext(pageId, isBook), contextLabel: pageId ? title : "No notes",
    mode: "answer", messages: [], history: [], historyContexts: [], draft: "", selection: null, selectionError: null,
    state: initialState(), error: null, note: "", requestId: null, generation: 0, pendingIndex: null, runContext: null,
    queuePosition: 0,
  };
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
  // Name it from the question straight away so the switcher never shows a
  // blank row, then let the model improve on it once there's an answer to
  // summarise. Showing the question first is what every other chat app does,
  // and it means a slow or missing model costs nothing.
  const shouldAutoName = !thread.titleIsCustom && !thread.title.trim();
  if (shouldAutoName) thread.title = deriveChatTitle(question);
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
      if (shouldAutoName) void autoNameConversation(thread, question, answer.content, generation);
    }
    // Keep what the run did next to the answer it produced; thread.state is
    // reset by the next question.
    answer.steps = thread.state.steps.map((step) => ({ ...step }));
    updateAssistantConversation();
    void persistConversation(thread);
  };
  let releaseSlot: (() => void) | null = null;
  try {
    if ((await getGraphInfo()).path !== thread.graphPath) throw new Error("The graph changed. Return to the original graph before asking.");
    if (!live()) return;
    // An embedded model serves one request at a time. Waiting here, visibly,
    // beats blocking inside the Rust worker where the user sees only a
    // spinner that looks like a hang.
    releaseSlot = await acquireChatSlot(requestId, (position) => {
      if (!live()) return;
      thread.queuePosition = position;
      const label = queueWaitLabel(position, currentChatConcurrency().provider);
      thread.note = label;
      dispatch(thread, { type: "note", at: Date.now(), text: label });
    });
    if (!live()) {
      releaseSlot();
      return;
    }
    thread.queuePosition = 0;
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
  } finally {
    releaseSlot?.();
    thread.queuePosition = 0;
  }
}

export async function stopAssistantConversation(thread: AssistantThread): Promise<void> {
  const requestId = thread.requestId;
  if (!assistantConversationRunning(thread)) return;
  const generation = ++thread.generation;
  thread.requestId = null;
  thread.pendingIndex = null;
  thread.note = "";
  thread.queuePosition = 0;
  dispatch(thread, { type: "cancel", at: Date.now() });
  // A chat stopped while queued never reached the backend, so there is
  // nothing to cancel there -- but it must still give up its place or
  // everyone behind it waits on a request that will never run.
  if (requestId) cancelChatSlot(requestId);
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

/**
 * Replace the placeholder name with one the model wrote.
 *
 * Deliberately fire-and-forget and deliberately late: the chat already has a
 * usable name from its first question, so this can fail, be slow, or find no
 * model at all without anyone noticing. It runs after the answer is complete
 * so the summary has something to summarise, and it takes its turn in the
 * queue like any other request rather than jumping ahead of a real question.
 */
async function autoNameConversation(
  thread: AssistantThread, question: string, answer: string, generation: number,
): Promise<void> {
  const placeholder = thread.title;
  let suggested: string;
  try {
    suggested = await suggestChatTitle(question, answer);
  } catch {
    return;
  }
  // A name the user typed while this was in flight wins.
  const applies = shouldApplyChatTitle(suggested, {
    placeholder, current: thread.title, titleIsCustom: thread.titleIsCustom,
  });
  if (!applies || thread.generation !== generation) return;
  thread.title = suggested.trim();
  updateAssistantConversation();
  void persistConversation(thread);
}

/**
 * Save a conversation so it survives a reload.
 *
 * Failures are swallowed on purpose: not being able to store a transcript is
 * an annoyance, but surfacing it as a chat error would bury the answer the
 * user actually asked for behind a storage complaint.
 */
export async function persistConversation(thread: AssistantThread): Promise<void> {
  if (!thread.messages.length) return;
  const now = Date.now();
  const messages: StoredChatMessage[] = thread.messages.map((message, index) => ({
    id: `${thread.id}:${index}`,
    role: message.role,
    content: message.content,
    contextLabel: message.contextLabel ?? "",
    mode: message.mode ?? "answer",
    webResearch: message.webResearch === true,
    sourcesJson: message.sources?.length ? JSON.stringify(message.sources) : null,
    createdAt: now,
  }));
  try {
    await saveChatThread({
      id: thread.id,
      title: thread.title,
      sourcePageId: thread.sourcePageId,
      sourcePageTitle: thread.sourcePageTitle,
      sourceIsBook: thread.sourceIsBook,
      mode: thread.mode,
      contextJson: serializeContext(thread.context),
      createdAt: 0,
      updatedAt: now,
    }, messages);
  } catch (error) {
    console.warn("[chat] Could not store the conversation:", error);
  }
}

/** Every conversation in this graph, newest first, for the switcher. */
export function listAssistantConversations(graphPath: string): AssistantThread[] {
  return [...conversations.values()]
    .filter((thread) => thread.graphPath === graphPath)
    .sort((a, b) => b.generation - a.generation || a.title.localeCompare(b.title));
}

/** Start an additional conversation that isn't tied to a page. */
export function createAssistantConversation(graphPath: string): AssistantThread {
  if (!graphPath.trim()) throw new Error("Open a graph before starting Chat.");
  const thread = blankThread(graphPath, null, "", false);
  conversations.set(thread.id, thread);
  return thread;
}

export async function renameAssistantConversation(thread: AssistantThread, title: string): Promise<void> {
  const cleaned = title.replace(/\s+/g, " ").trim();
  if (!cleaned) return;
  thread.title = cleaned;
  // A name the user typed must not be overwritten by the next question.
  thread.titleIsCustom = true;
  updateAssistantConversation();
  try {
    await renameChatThread(thread.id, cleaned);
  } catch (error) {
    console.warn("[chat] Could not store the new name:", error);
  }
}

export async function deleteAssistantConversation(thread: AssistantThread): Promise<void> {
  await stopAssistantConversation(thread);
  conversations.delete(thread.id);
  for (const [key, id] of sourceConversations) {
    if (id === thread.id) sourceConversations.delete(key);
  }
  updateAssistantConversation();
  try {
    await deleteChatThread(thread.id);
  } catch (error) {
    console.warn("[chat] Could not delete the stored conversation:", error);
  }
}

/**
 * Bring stored conversations back after a restart.
 *
 * Threads already in memory win: a live conversation must never be clobbered
 * by an older snapshot of itself.
 */
export async function restoreAssistantConversations(graphPath: string): Promise<void> {
  if (!graphPath.trim()) return;
  let stored;
  try {
    stored = await listChatThreads();
  } catch (error) {
    console.warn("[chat] Could not read stored conversations:", error);
    return;
  }
  for (const record of stored) {
    if (conversations.has(record.id)) continue;
    let full;
    try {
      full = await loadChatThread(record.id);
    } catch {
      continue;
    }
    if (!full) continue;
    const thread = blankThread(
      graphPath, full.sourcePageId, full.sourcePageTitle, full.sourceIsBook, full.id,
    );
    thread.title = full.title;
    thread.titleIsCustom = full.title.trim().length > 0;
    thread.mode = parseMode(full.mode);
    thread.context = parseContext(full.contextJson);
    thread.contextLabel = thread.sourcePageId ? thread.sourcePageTitle : "No notes";
    thread.messages = full.messages.map((message) => ({
      role: message.role === "assistant" ? "assistant" : "user",
      content: message.content,
      contextLabel: message.contextLabel,
      mode: parseMode(message.mode),
      webResearch: message.webResearch,
      sources: message.sourcesJson ? safeParseSources(message.sourcesJson) : undefined,
    }));
    // History drives follow-up questions, so it has to be rebuilt from the
    // stored turns -- a restored chat that "forgets" what was just said is
    // worse than one that wasn't restored at all.
    thread.history = thread.messages
      .filter((message) => message.content.trim().length > 0)
      .map((message) => ({ role: message.role, content: message.content }));
    thread.historyContexts = thread.history.map(() => copyAssistantContext(thread.context));
    conversations.set(thread.id, thread);
    if (thread.sourcePageId) {
      sourceConversations.set(JSON.stringify([graphPath, thread.sourcePageId]), thread.id);
    }
  }
  updateAssistantConversation();
}

function safeParseSources(json: string): AssistantMessage["sources"] {
  try {
    const parsed = JSON.parse(json);
    return Array.isArray(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}
