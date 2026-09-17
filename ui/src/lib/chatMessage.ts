import type { ChatSource, WebSource } from "./knowledge";
import type { StreamStep } from "./chatStatus";

export type ChatRole = "user" | "assistant";

export type ChatThinkingTone = "thinking" | "working" | "web" | "stalled";

export interface ChatMessageModel {
  role: ChatRole;
  content: string;
  sources?: ChatSource[];
  /** Web citations for a research answer's "From the web" section. */
  webSources?: WebSource[];
  /** True once this answer engaged web research — drives the "Web research" badge. */
  webResearch?: boolean;
  /**
   * What the assistant did to produce this answer, snapshotted when the run
   * ended. The live trail comes from the thread state, which the next question
   * resets; keeping a copy here is what lets an older answer still show its
   * steps instead of the work vanishing the moment you ask something else.
   */
  steps?: StreamStep[];
}
