import { describe, it, expect } from "vitest";
import {
  snakeToCamelDeep,
  camelToSnakeDeep,
  defaultResearchConfig,
  clampResearchNumbers,
  validateEngineDraft,
  engineFromDraft,
  RESEARCH_PROMPT_STEPS,
  DEFAULT_ENGINES,
  type EngineDraft,
  type ResearchConfig,
} from "./research";

// ─── Casing codec ─────────────────────────────────────────────────────────────
// These verify the *mechanism* that the one-line PAYLOAD_CASE flip drives, so
// the round-trip is proven regardless of which convention is active today.
describe("research payload casing codec", () => {
  it("rewrites nested object keys snake→camel", () => {
    const out = snakeToCamelDeep({
      url_template: "https://x/?q={query}",
      json_paths: { results: "data", url: "u", title: "t", snippet: "s" },
      max_rounds: 3,
    });
    expect(out).toEqual({
      urlTemplate: "https://x/?q={query}",
      jsonPaths: { results: "data", url: "u", title: "t", snippet: "s" },
      maxRounds: 3,
    });
  });

  it("leaves values (enum variants, templated URLs) untouched — only keys change", () => {
    const out = snakeToCamelDeep<{ kind: string; url_template: string }>({
      kind: "Html",
      url_template: "https://s/?q={query}",
    });
    expect(out.kind).toBe("Html");
    expect(out.url_template).toBeUndefined();
    expect((out as any).urlTemplate).toBe("https://s/?q={query}");
  });

  it("round-trips a full config through camel and back with no loss", () => {
    const cfg = defaultResearchConfig();
    const back = camelToSnakeDeep<ResearchConfig>(snakeToCamelDeep(cfg));
    expect(back).toEqual(cfg);
  });

  it("descends into arrays of engines", () => {
    const out = snakeToCamelDeep<any>({ engines: [{ url_template: "a", json_paths: null }] });
    expect(out.engines[0].urlTemplate).toBe("a");
    expect(out.engines[0].jsonPaths).toBeNull();
  });
});

// ─── Default config ───────────────────────────────────────────────────────────
describe("defaultResearchConfig", () => {
  it("ships every built-in engine from the contract", () => {
    const cfg = defaultResearchConfig();
    const ids = cfg.engines.map((e) => e.id).sort();
    expect(ids).toEqual(
      [
        "arxiv",
        "brave",
        "crossref",
        "doaj",
        "duckduckgo",
        "europepmc",
        "mojeek",
        "openalex",
        "semanticscholar",
        "startpage",
      ].sort(),
    );
  });

  it("marks all defaults built-in and enabled", () => {
    for (const e of defaultResearchConfig().engines) {
      expect(e.builtin).toBe(true);
      expect(e.enabled).toBe(true);
    }
  });

  it("ships no shadow-library engines", () => {
    const banned = /libgen|anna|sci-?hub/i;
    for (const e of DEFAULT_ENGINES) {
      expect(banned.test(e.id)).toBe(false);
      expect(banned.test(e.name)).toBe(false);
      expect(banned.test(e.url_template)).toBe(false);
    }
  });

  it("carries a prompt and a plain-English explanation for every step", () => {
    const cfg = defaultResearchConfig();
    for (const step of RESEARCH_PROMPT_STEPS) {
      expect(cfg.prompts[step.key].length).toBeGreaterThan(0);
      expect(step.explanation.length).toBeGreaterThan(0);
    }
  });

  it("orders the prompt steps as the pipeline runs", () => {
    expect(RESEARCH_PROMPT_STEPS.map((s) => s.key)).toEqual([
      "plan_queries",
      "select_sources",
      "assess_sufficiency",
      "refine_queries",
      "synthesize",
    ]);
  });

  it("returns a fresh copy each call so form edits don't leak into the constant", () => {
    const a = defaultResearchConfig();
    a.engines[0].enabled = false;
    a.prompts.synthesize = "edited";
    const b = defaultResearchConfig();
    expect(b.engines[0].enabled).toBe(true);
    expect(b.prompts.synthesize).not.toBe("edited");
  });
});

// ─── Numeric clamping ─────────────────────────────────────────────────────────
describe("clampResearchNumbers", () => {
  it("pulls out-of-range values back to the allowed bounds", () => {
    const cfg = { ...defaultResearchConfig(), max_rounds: 99, max_sources: 0, results_per_query: -4 };
    const out = clampResearchNumbers(cfg);
    expect(out.max_rounds).toBe(10);
    expect(out.max_sources).toBe(1);
    expect(out.results_per_query).toBe(1);
  });

  it("rounds and repairs non-finite input", () => {
    const cfg = { ...defaultResearchConfig(), max_rounds: 2.7, max_sources: NaN };
    const out = clampResearchNumbers(cfg);
    expect(out.max_rounds).toBe(3);
    expect(out.max_sources).toBe(1);
  });

  it("leaves in-range values alone", () => {
    const cfg = { ...defaultResearchConfig(), max_rounds: 3, max_sources: 8, results_per_query: 6 };
    expect(clampResearchNumbers(cfg)).toMatchObject({ max_rounds: 3, max_sources: 8, results_per_query: 6 });
  });
});

// ─── Add-engine validation ────────────────────────────────────────────────────
describe("validateEngineDraft", () => {
  const goodHtml: EngineDraft = {
    id: "my-engine",
    name: "My Engine",
    kind: "Html",
    url_template: "https://example.com/search?q={query}",
    category: "Web",
    selectors: { result: ".r", link: "a", title: "h3", snippet: ".s" },
  };

  it("accepts a complete HTML draft", () => {
    expect(validateEngineDraft(goodHtml)).toEqual([]);
  });

  it("accepts a complete JSON draft", () => {
    const draft: EngineDraft = {
      id: "myapi",
      name: "My API",
      kind: "Json",
      url_template: "https://api.example.com?q={query}",
      category: "Academic",
      json_paths: { results: "data", url: "u", title: "t", snippet: "s" },
    };
    expect(validateEngineDraft(draft)).toEqual([]);
  });

  it("requires a {query} placeholder in the URL template", () => {
    const errs = validateEngineDraft({ ...goodHtml, url_template: "https://example.com/search" });
    expect(errs.some((e) => /\{query\}/.test(e))).toBe(true);
  });

  it("rejects a non-slug id", () => {
    const errs = validateEngineDraft({ ...goodHtml, id: "My Engine!" });
    expect(errs.some((e) => /slug/i.test(e))).toBe(true);
  });

  it("rejects an id that collides with an existing engine", () => {
    const errs = validateEngineDraft(goodHtml, ["my-engine"]);
    expect(errs.some((e) => /already in use/i.test(e))).toBe(true);
  });

  it("requires the HTML selectors when kind is Html", () => {
    const errs = validateEngineDraft({ ...goodHtml, selectors: { result: "", link: "", title: "" } });
    expect(errs).toContain("HTML engines need a result (container) selector.");
    expect(errs).toContain("HTML engines need a link selector.");
    expect(errs).toContain("HTML engines need a title selector.");
  });

  it("requires the JSON paths when kind is Json", () => {
    const errs = validateEngineDraft({
      id: "j",
      name: "J",
      kind: "Json",
      url_template: "https://x?q={query}",
      json_paths: { results: "", url: "", title: "" },
    });
    expect(errs).toContain("JSON engines need a results (array) path.");
  });

  it("does not demand JSON paths for an HTML engine (conditional fields)", () => {
    // A complete HTML draft with no json_paths at all must still pass.
    expect(validateEngineDraft({ ...goodHtml, json_paths: undefined })).toEqual([]);
  });

  it("flags a missing kind", () => {
    const errs = validateEngineDraft({ id: "x", name: "X", url_template: "https://x?q={query}" });
    expect(errs.some((e) => /kind/i.test(e))).toBe(true);
  });
});

describe("engineFromDraft", () => {
  it("keeps only selectors for an HTML engine and marks it user-owned", () => {
    const eng = engineFromDraft({
      id: "e",
      name: "E",
      kind: "Html",
      url_template: "https://e?q={query}",
      category: "Web",
      selectors: { result: " .r ", link: "a", title: "h3", snippet: "" },
      json_paths: { results: "leak" },
    });
    expect(eng.builtin).toBe(false);
    expect(eng.enabled).toBe(true);
    expect(eng.json_paths).toBeNull();
    expect(eng.selectors).toEqual({ result: ".r", link: "a", title: "h3", snippet: "" });
  });

  it("keeps only json_paths for a JSON engine", () => {
    const eng = engineFromDraft({
      id: "e",
      name: "E",
      kind: "Json",
      url_template: "https://e?q={query}",
      category: "Academic",
      json_paths: { results: "data", url: "u", title: "t", snippet: "s" },
    });
    expect(eng.selectors).toBeNull();
    expect(eng.json_paths).toEqual({ results: "data", url: "u", title: "t", snippet: "s" });
  });
});
