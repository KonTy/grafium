/**
 * Storing chat conversations, and deciding when a chat has to wait its turn.
 *
 * Conversations live in the graph's `.grafium/index.db`, which sync never
 * touches. They are working state, not notes: keeping them out of `pages/`
 * means a half-finished thread never lands on a USB stick or a shared file
 * server, and a conversation that quotes private notes cannot leak through a
 * sync target.
 */

import { invoke } from "@tauri-apps/api/core";
import type { AssistantContext, AssistantMode } from "./assistant";

export interface StoredChatThread {
  id: string;
  title: string;
  sourcePageId: string | null;
  sourcePageTitle: string;
  sourceIsBook: boolean;
  mode: string;
  contextJson: string;
  createdAt: number;
  updatedAt: number;
}

export interface StoredChatMessage {
  id: string;
  role: string;
  content: string;
  contextLabel: string;
  mode: string;
  webResearch: boolean;
  sourcesJson: string | null;
  createdAt: number;
}

export interface StoredChatThreadWithMessages extends StoredChatThread {
  messages: StoredChatMessage[];
}

export function listChatThreads(): Promise<StoredChatThread[]> {
  return invoke("list_chat_threads", {});
}

export function loadChatThread(threadId: string): Promise<StoredChatThreadWithMessages | null> {
  return invoke("load_chat_thread", { threadId });
}

export function saveChatThread(
  thread: StoredChatThread,
  messages: StoredChatMessage[],
): Promise<void> {
  return invoke("save_chat_thread", { thread, messages });
}

export function renameChatThread(threadId: string, title: string): Promise<void> {
  return invoke("rename_chat_thread", { threadId, title });
}

export function deleteChatThread(threadId: string): Promise<void> {
  return invoke("delete_chat_thread", { threadId });
}

export interface ChatConcurrency {
  /** False when the provider serialises requests, so extra chats must queue. */
  parallel: boolean;
  slots: number | null;
  provider: string;
}

export function chatConcurrency(): Promise<ChatConcurrency> {
  return invoke("chat_concurrency", {});
}

/** Assumed until the backend answers: never make the user wait on a guess. */
const OPTIMISTIC: ChatConcurrency = { parallel: true, slots: null, provider: "" };

let capability: ChatConcurrency = OPTIMISTIC;
let capabilityLoad: Promise<ChatConcurrency> | null = null;

/**
 * Ask the backend how many chats can run at once, once, and cache it.
 *
 * The answer depends on transport, not on local-vs-cloud: anything over HTTP
 * batches server-side, while the embedded llama.cpp path has one worker and
 * one model resident in VRAM.
 */
export async function loadChatConcurrency(): Promise<ChatConcurrency> {
  if (!capabilityLoad) {
    capabilityLoad = chatConcurrency()
      .then((value) => {
        capability = value;
        return value;
      })
      .catch(() => {
        // A provider that can't be asked is not a reason to queue.
        capability = OPTIMISTIC;
        capabilityLoad = null;
        return OPTIMISTIC;
      });
  }
  return capabilityLoad;
}

/** Re-ask after the provider changes in Settings. */
export function resetChatConcurrency(): void {
  capability = OPTIMISTIC;
  capabilityLoad = null;
}

export function currentChatConcurrency(): ChatConcurrency {
  return capability;
}

interface QueueEntry {
  id: string;
  onPosition: (position: number) => void;
  grant: () => void;
  granted: boolean;
}

const entries: QueueEntry[] = [];

function slots(): number {
  if (capability.parallel) return Number.MAX_SAFE_INTEGER;
  return Math.max(1, capability.slots ?? 1);
}

function runningCount(): number {
  return entries.filter((entry) => entry.granted).length;
}

function pump(): void {
  for (const entry of entries) {
    if (entry.granted) continue;
    if (runningCount() >= slots()) break;
    entry.granted = true;
    entry.onPosition(0);
    entry.grant();
  }
  let ahead = runningCount();
  for (const entry of entries) {
    if (entry.granted) continue;
    entry.onPosition(ahead);
    ahead += 1;
  }
}

function drop(id: string): void {
  const index = entries.findIndex((entry) => entry.id === id);
  if (index < 0) return;
  entries.splice(index, 1);
  pump();
}

/**
 * Wait until the provider can actually serve this chat.
 *
 * Without this a second chat against the embedded model doesn't fail — it
 * blocks inside the Rust worker with no feedback, which is indistinguishable
 * from a hang. The Rust queue is FIFO, so the position reported here is the
 * order requests really run in rather than a guess.
 *
 * Returns a function that must be called when the request finishes, however
 * it finishes.
 */
export async function acquireChatSlot(
  id: string,
  onPosition: (position: number) => void,
): Promise<() => void> {
  await loadChatConcurrency();
  if (capability.parallel) {
    // Nothing to coordinate: let it run and make releasing a no-op.
    return () => {};
  }

  await new Promise<void>((resolve) => {
    entries.push({ id, onPosition, grant: resolve, granted: false });
    pump();
  });
  return () => drop(id);
}

/**
 * Give up a place in the queue without waiting for it to come up.
 *
 * A chat stopped while queued has no backend request to cancel, but it must
 * still stand aside immediately — otherwise everyone behind it waits out a
 * turn that will never be used.
 */
export function cancelChatSlot(id: string): void {
  const entry = entries.find((queued) => queued.id === id);
  if (!entry) return;
  drop(id);
  // Wake the awaiting caller so its cleanup runs; it will see the run is no
  // longer current and return without calling the model.
  if (!entry.granted) entry.grant();
}

/** How many chats are running or waiting, for tests and diagnostics. */
export function chatQueueDepth(): number {
  return entries.length;
}

/** Drop all queue state. Used when the graph changes and by tests. */
export function resetChatQueue(): void {
  entries.length = 0;
}

/** Describe a wait in words the user can act on. */
export function queueWaitLabel(position: number, provider: string): string {
  if (position <= 0) return "";
  const who = provider ? `the ${provider} model` : "the model";
  if (position === 1) return `Waiting for ${who} — next in line`;
  return `Waiting for ${who} — ${position} ahead`;
}

/**
 * A conversation's name, taken from its opening question.
 *
 * Users don't name chats up front, but an unnamed list of twenty threads is
 * useless. The first question is what someone actually remembers a
 * conversation by.
 */
export function deriveChatTitle(firstQuestion: string): string {
  const cleaned = firstQuestion.replace(/\s+/g, " ").trim();
  if (!cleaned) return "New chat";
  if (cleaned.length <= 48) return cleaned;
  const clipped = cleaned.slice(0, 48);
  const lastSpace = clipped.lastIndexOf(" ");
  return `${lastSpace > 24 ? clipped.slice(0, lastSpace) : clipped}…`;
}

export function serializeContext(context: AssistantContext): string {
  try {
    return JSON.stringify(context);
  } catch {
    return JSON.stringify({ kind: "none" });
  }
}

export function parseContext(json: string): AssistantContext {
  try {
    const parsed = JSON.parse(json);
    if (parsed && typeof parsed === "object" && typeof parsed.kind === "string") {
      return parsed as AssistantContext;
    }
  } catch {
    // A context we can't read is not worth failing a whole thread over.
  }
  return { kind: "none" };
}

export function parseMode(mode: string): AssistantMode {
  return mode === "web" || mode === "deep" ? mode : "answer";
}
