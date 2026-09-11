import type { ChatSource, WebSource } from "./knowledge";

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
}
