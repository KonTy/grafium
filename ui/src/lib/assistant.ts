import { invoke } from "@tauri-apps/api/core";
import type { ChatTurn } from "./knowledge";
import { researchCancel, researchStream, type ResearchScopeInfo, type ResearchStreamHandlers } from "./research";

export type AssistantMode = "answer" | "web" | "deep";
export type AssistantContext =
  | { kind: "none" | "graph" }
  | { kind: "page" | "book"; pageId: string }
  | { kind: "block" | "section"; pageId: string; blockId: string }
  | { kind: "selection"; pageId: string; selection: { blockIds: string[]; text: string } };

export interface AssistantContextInfo extends ResearchScopeInfo {
  book: { pageId: string; title: string } | null;
}

export interface AssistantRequest {
  graphPath: string;
  question: string;
  requestId: string;
  context: AssistantContext;
  history: ChatTurn[];
  mode: AssistantMode;
}

export function assistantContextInfo(graphPath: string, pageId: string, blockId?: string): Promise<AssistantContextInfo> {
  return invoke("assistant_context_info", { graphPath, pageId, blockId });
}

export function assistantChat(request: AssistantRequest, handlers: ResearchStreamHandlers): Promise<void> {
  const { requestId, ...args } = request;
  return researchStream("assistant_chat", args, handlers, requestId);
}

export const assistantCancel = researchCancel;

export function assistantContextPageId(context: AssistantContext): string | null {
  return "pageId" in context ? context.pageId : null;
}

export function copyAssistantContext(context: AssistantContext): AssistantContext {
  if ("pageId" in context && !context.pageId.trim()) throw new Error("Choose a source page first.");
  if ("blockId" in context && !context.blockId.trim()) throw new Error("Choose a block or section first.");
  if (context.kind === "selection") {
    const { blockIds, text } = context.selection;
    if (!text.trim() || !blockIds.length || blockIds.some((id) => !id.trim())
      || new Set(blockIds).size !== blockIds.length) {
      throw new Error("Select a passage within one page or journal day first.");
    }
    return { ...context, selection: { blockIds: [...blockIds], text } };
  }
  return { ...context };
}

export const assistantModeLabels: Record<AssistantMode, string> = {
  answer: "Answer - no web",
  web: "Web search",
  deep: "Deep web research",
};
