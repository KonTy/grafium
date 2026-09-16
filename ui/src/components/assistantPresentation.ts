import type { AiConfig } from "../lib/knowledge";
import type { AssistantMode } from "../lib/assistant";

export const assistantModes: Record<AssistantMode, { label: string; description: string }> = {
  answer: { label: "Answer — no web", description: "Answer using the chosen notes and model knowledge. No web requests." },
  web: { label: "Web search", description: "Grafium searches the web and forwards source results to your configured model." },
  deep: { label: "Deep web research", description: "Grafium searches and forwards sources to your model in multiple rounds. Takes longer." },
};

export function assistantProvider(config: AiConfig | null): { label: string; detail: string } {
  if (!config?.enabled) return { label: "Configure model", detail: "Choose a model in Settings." };
  const local = config.mode === "local";
  const provider = (local ? config.local?.provider : config.cloud?.llm_provider)?.toLowerCase().replace(/[-_]/g, "") ?? "";
  const endpoint = local ? config.local?.base_url : config.cloud?.llm_base_url;
  const embedded = local && ["local", "embedded", "llamacpp", "huggingface"].includes(provider);
  const model = local ? embedded ? config.local?.local_llm?.model : config.local?.llm_model : config.cloud?.llm_model;
  if (embedded) return { label: "Embedded / On this computer", detail: model?.split(/[\\/]/).pop() || "Embedded model" };
  let host = "";
  try { host = endpoint ? new URL(endpoint).host : ""; } catch { /* Never expose credentials or invalid endpoint text. */ }
  const knownCloudHost = host === "api.openai.com" || host === "api.anthropic.com";
  const cloudService = !local && ["openai", "anthropic"].includes(provider) && (!endpoint || knownCloudHost);
  return {
    label: cloudService ? "Cloud service" : "Model server / API endpoint",
    detail: [model, host].filter(Boolean).join(" · ") || "Configured model",
  };
}
