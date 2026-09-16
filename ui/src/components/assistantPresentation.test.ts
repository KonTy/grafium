import { describe, expect, it } from "vitest";
import { assistantModes, assistantProvider } from "./assistantPresentation";
import type { AiConfig } from "../lib/knowledge";
import search from "./GlobalSearchDialog.svelte?raw";
import diagnostics from "./AssistantDiagnostics.svelte?raw";

describe("Assistant model and mode labels", () => {
  it("makes web use explicit with exactly three mutually exclusive modes", () => {
    expect(Object.keys(assistantModes)).toEqual(["answer", "web", "deep"]);
    expect(assistantModes.answer.label).toBe("Answer — no web");
    expect(assistantModes.web.description).toContain("Grafium");
    expect(assistantModes.deep.description).toContain("Grafium");
  });
  it("does not interpret cloud mode as proof that an endpoint is a public cloud", () => {
    const config = { enabled: true, mode: "cloud", cloud: { llm_provider: "openai_compatible", llm_model: "DGX model", llm_base_url: "http://spark.local:8000/v1" } } as AiConfig;
    expect(assistantProvider(config)).toEqual({ label: "Model server / API endpoint", detail: "DGX model · spark.local:8000" });
    config.cloud!.llm_base_url = "http://localhost:8000/v1";
    expect(assistantProvider(config).label).toBe("Model server / API endpoint");
  });
  it("recognizes hosted services and handles custom OpenAI endpoints without guessing", () => {
    const config = { enabled: true, mode: "cloud", cloud: { llm_provider: "openai", llm_model: "model" } } as AiConfig;
    expect(assistantProvider(config).label).toBe("Cloud service");
    config.cloud!.llm_base_url = "https://server.example/v1";
    expect(assistantProvider(config).label).toBe("Model server / API endpoint");
  });
  it("distinguishes embedded inference and never prints endpoint credentials", () => {
    const config = { enabled: true, mode: "local", local: { provider: "hugging_face", local_llm: { model: "/models/example.gguf" } } } as AiConfig;
    expect(assistantProvider(config)).toEqual({ label: "Embedded / On this computer", detail: "example.gguf" });
    config.local!.provider = "ollama";
    config.local!.base_url = "https://user:secret@server.example:8443/v1?key=private";
    const value = assistantProvider(config);
    expect(value.label).toBe("Model server / API endpoint");
    expect(value.detail).toContain("server.example:8443");
    expect(value.detail).not.toMatch(/secret|private|user/);
  });
});

describe("Global search and model diagnostics", () => {
  it("keeps ordinary prefix/title/block search independent of semantic AI Search", () => {
    expect(search).toContain("searchPageTitles(text, 8), searchFts(text, 15)");
    expect(search).toContain("request !== version");
    expect(search).toContain("aiSearch(question, 20)");
    expect(search).toContain('if (event.key === "Enter")');
    expect(search).toContain("else queueSearch(true)");
    expect(search).toContain('disabled={!query.trim()}');
    expect(search).toContain("dialog.showModal()");
    expect(search).toContain('aria-label="Search pages and blocks"');
    expect(search).toContain("targetBlockId: entry.match.block.id");
  });
  it("preserves index coverage, indexing, CPU warnings and explicit GPU retry", () => {
    for (const token of ["aiIndexStatus", "aiIndexAllPages", "aiRetryLlmOnGpu", "Running on CPU", "Index now", "Configure provider"]) {
      expect(diagnostics).toContain(token);
    }
    expect(diagnostics).toContain("retrying || running");
  });
});
