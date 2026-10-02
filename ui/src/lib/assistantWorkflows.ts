import { invoke } from "@tauri-apps/api/core";
import type { Block } from "./api";
import type { AssistantContext } from "./assistant";
import { groupAssistantTasks, type WorkflowTask } from "./assistantTaskGrouping";
import { buildCleanedNoteDraft, buildRelatedTopicDraft, type WorkflowPage } from "./assistantNoteWorkflows";
import { showToast } from "./toast.svelte";

export type AssistantWorkflowKind = "tasks" | "topics" | "rewrite";
export interface WorkflowSnapshot {
  token: string;
  graphPath: string;
  kind: AssistantWorkflowKind;
  sourcePageId: string | null;
  sourcePageTitle: string | null;
  pages: WorkflowPage[];
  tasks: WorkflowTask[];
  coverage: string;
}
export interface WorkflowProposal {
  snapshot: WorkflowSnapshot;
  request: string;
  title: string;
  content: string;
  summary: string;
}
export interface WorkflowApplied {
  pageId: string;
  pageTitle: string;
  insertedBlocks: Block[];
  pageCreated: boolean;
}

export const WORKFLOW_LABELS: Record<AssistantWorkflowKind, string> = {
  tasks: "Group open tasks",
  topics: "Find related topics",
  rewrite: "Clean up draft",
};
export const WORKFLOW_REQUESTS: Record<AssistantWorkflowKind, string> = {
  tasks: "Group all my open TODOs by similarity and flag possible duplicates. Keep the original tasks unchanged.",
  topics: "Find notes related to this page and propose a new page with references and explanations.",
  rewrite: "Clean up these rough notes into a clearer draft below the original. Preserve the meaning and flag ambiguities.",
};

export function detectAssistantWorkflow(request: string): AssistantWorkflowKind | null {
  const text = request.trim().toLowerCase();
  if (!text || /^(?:how|why|what|when|where|is it|does|explain|tell me)\b/.test(text)) return null;
  if (/\b(?:todo(?:s)?|to-do(?:s)?|tasks)\b/.test(text)
    && /\b(?:group|organize|organise|similarity|similar|dedup(?:licate)?|duplicates?|clean\s*up)\b/.test(text)) return "tasks";
  if (/\b(?:related|similar|connections|same topics?)\b/.test(text)
    && /\b(?:notes|pages|topics|graph)\b/.test(text)
    && /\b(?:find|scan|search|create|make|build|collect|gather)\b/.test(text)) return "topics";
  if (/\b(?:clean\s*up|tidy|rewrite|polish|clarify)\b/.test(text)
    && /\b(?:page|notes|draft|text|selection|this|these)\b/.test(text)) return "rewrite";
  return null;
}

function checkActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("Analysis cancelled. No changes were saved.", "AbortError");
}

async function completeWorkflow(graphPath: string, prompt: string, signal: AbortSignal): Promise<string> {
  checkActive(signal);
  const requestId = crypto.randomUUID();
  const cancel = () => {
    void invoke("assistant_workflow_cancel", { requestId }).catch(cause => {
      console.error("Could not cancel ASK analysis", cause);
      showToast(`Could not confirm model cancellation: ${String(cause)}. Its result will be ignored.`, "error");
    });
  };
  signal.addEventListener("abort", cancel, { once: true });
  try {
    const answer = await invoke<string>("assistant_workflow_complete", { graphPath, prompt, requestId });
    checkActive(signal);
    return answer;
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

export async function prepareAssistantWorkflow(
  kind: AssistantWorkflowKind,
  graphPath: string,
  context: AssistantContext,
  sourcePageId: string | null,
  request: string,
  options: { signal: AbortSignal; onProgress: (message: string) => void },
): Promise<WorkflowProposal> {
  checkActive(options.signal);
  if (!graphPath) throw new Error("Open a graph before asking for note actions.");
  if (kind !== "tasks" && !sourcePageId) throw new Error("Open the source page and choose its context first.");
  if (kind === "rewrite" && (context.kind === "graph" || context.kind === "none"))
    throw new Error("Choose Selection, Block, Section, or This page before cleaning up a draft.");
  const blockIds = kind === "rewrite"
    ? context.kind === "selection" ? context.selection.blockIds
      : "blockId" in context ? [context.blockId] : undefined
    : undefined;
  options.onProgress(kind === "tasks" ? "Reading open tasks in this graph…" : "Reading saved notes in this graph…");
  const snapshot = await invoke<WorkflowSnapshot>("assistant_workflow_snapshot", {
    graphPath, kind, ...(kind !== "tasks" ? { pageId: sourcePageId } : {}), ...(blockIds ? { blockIds } : {}),
    ...(kind === "rewrite" && (context.kind === "block" || context.kind === "section") ? { includeDescendants: true } : {}),
  });
  checkActive(options.signal);
  if (snapshot.graphPath !== graphPath || snapshot.kind !== kind || !snapshot.token)
    throw new Error("The graph or action snapshot changed. Start the request again.");
  const generation = {
    signal: options.signal,
    onProgress: options.onProgress,
    complete: (prompt: string) => completeWorkflow(graphPath, prompt, options.signal),
  };
  let draft: { title: string; content: string; summary: string };
  if (kind === "tasks") draft = await groupAssistantTasks(snapshot.tasks, request, generation);
  else {
    const source = snapshot.pages.find(page => page.id === snapshot.sourcePageId);
    if (!source) throw new Error("The source page is not available in this snapshot.");
    draft = kind === "topics"
      ? await buildRelatedTopicDraft(source, snapshot.pages, request, generation)
      : await buildCleanedNoteDraft(source, request, generation, context.kind === "selection" ? context.selection.text : undefined);
  }
  checkActive(options.signal);
  if (!draft.content.trim()) throw new Error("The model produced no draft. Nothing was saved.");
  const suffix = new Date().toISOString().slice(0, 19).replace("T", " ").replaceAll(":", "-");
  return {
    snapshot, request, content: draft.content,
    title: kind === "rewrite" ? "Suggested rewrite" : `${draft.title} - ${suffix}`,
    summary: [snapshot.coverage, draft.summary].filter(Boolean).join(" "),
  };
}

export async function applyAssistantWorkflow(proposal: WorkflowProposal): Promise<WorkflowApplied> {
  if (!proposal.title.trim() || !proposal.content.trim()) throw new Error("Keep a title and draft text before applying.");
  return invoke<WorkflowApplied>("assistant_workflow_apply", {
    graphPath: proposal.snapshot.graphPath,
    token: proposal.snapshot.token,
    title: proposal.title.trim(),
    reviewedMarkdown: proposal.content,
    blocks: [{ content: proposal.snapshot.kind === "rewrite" ? proposal.title.trim() : "AI review" }],
  });
}

export interface WorkflowPreviewPart {
  text: string;
  source?: { id: string; title: string; blockId: string };
}
export function workflowPreviewLine(line: string, snapshot: WorkflowSnapshot): WorkflowPreviewPart[] {
  const result: WorkflowPreviewPart[] = [];
  const pattern = /\(\(([0-9a-f-]{36})\)\)/gi;
  let start = 0;
  for (const match of line.matchAll(pattern)) {
    if (match.index > start) result.push({ text: line.slice(start, match.index) });
    const task = snapshot.tasks.find(task => task.id === match[1]);
    const page = task ? undefined : snapshot.pages.find(page => page.blocks.some(block => block.id === match[1]));
    const block = page?.blocks.find(block => block.id === match[1]);
    result.push(task
      ? { text: task.content, source: { id: task.pageId, title: task.pageTitle, blockId: task.id } }
      : page && block ? { text: block.content, source: { id: page.id, title: page.title, blockId: block.id } }
        : { text: match[0] });
    start = match.index + match[0].length;
  }
  if (start < line.length) result.push({ text: line.slice(start) });
  return result;
}
