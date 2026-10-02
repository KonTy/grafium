import { assistantCancel, assistantChat, copyAssistantContext, type AssistantContext } from "./assistant";
import { showToast } from "./toast.svelte";

export async function collectAssistantPlan(
  graphPath: string, question: string, context: AssistantContext, signal: AbortSignal,
): Promise<string> {
  const checkCancelled = () => {
    if (signal.aborted) throw new DOMException("Planning stopped. No changes were saved.", "AbortError");
  };
  checkCancelled();
  const requestId = crypto.randomUUID();
  let answer = "";
  let error = "";
  const cancel = () => {
    void assistantCancel(requestId).catch(cause => {
      console.error("Could not cancel edit planning", cause);
      showToast(`Could not confirm model cancellation: ${String(cause)}. Its result will be ignored.`, "error");
    });
  };
  signal.addEventListener("abort", cancel, { once: true });
  try {
    await assistantChat({
      graphPath, question, requestId, context: copyAssistantContext(context), history: [], mode: "answer",
    }, {
      onChunk: delta => { if (!signal.aborted) answer += delta; },
      onDone: () => {},
      onError: message => { error = message; },
      shouldContinue: () => !signal.aborted,
    });
    checkCancelled();
    if (error) throw new Error(error);
    if (!answer.trim()) throw new Error("The model returned no edit plan.");
    return answer;
  } finally { signal.removeEventListener("abort", cancel); }
}
