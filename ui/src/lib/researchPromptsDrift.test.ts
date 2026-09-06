import { describe, it, expect } from "vitest";
import { DEFAULT_RESEARCH_PROMPTS } from "./research";
// Import the Rust source as a raw string (typed by vite/client's `*?raw`), so
// this stays a pure Vite/Vitest import with no Node fs/path/process types —
// which svelte-check's app tsconfig doesn't provide.
import configRs from "../../../core/src/research/config.rs?raw";

// WHY: the backend parses every research step's model output with
// `serde_json::from_str` (see core/src/research/config.rs), so the bundled
// frontend defaults used by per-field "Reset to default" must reproduce the
// Rust `default_*` prompts *exactly*. A silent drift would let a reset install
// a prompt the backend can't parse, breaking research with no error at reset
// time. This test reads the Rust source directly and fails the build on any
// divergence, which is the safeguard that lets us keep an offline copy at all.

// Turn a Rust string literal's raw bytes into its runtime value: resolve the
// `\`+newline line-continuations (which also swallow the next line's indent)
// and the `\n` / `\t` / `\"` / `\\` escapes the loop-prompt strings use.
function decodeRustLiteral(raw: string): string {
  let out = "";
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    if (c !== "\\") {
      out += c;
      continue;
    }
    const next = raw[i + 1];
    if (next === "\n") {
      i++; // consume the newline…
      while (raw[i + 1] === " " || raw[i + 1] === "\t") i++; // …and the indent
    } else if (next === "n") {
      out += "\n";
      i++;
    } else if (next === "t") {
      out += "\t";
      i++;
    } else if (next === "r") {
      out += "\r";
      i++;
    } else if (next === '"' || next === "\\" || next === "'") {
      out += next;
      i++;
    } else {
      out += next; // unknown escape: keep the escaped char literally
      i++;
    }
  }
  return out;
}

// Pull the string literal that `fn <name>()` returns, walking from its opening
// quote to the first unescaped closing quote.
function extractRustDefault(src: string, fnName: string): string {
  const fnIdx = src.indexOf(`fn ${fnName}(`);
  expect(fnIdx, `fn ${fnName} present in config.rs`).toBeGreaterThanOrEqual(0);
  const open = src.indexOf('"', fnIdx);
  expect(open, `string literal in ${fnName}`).toBeGreaterThanOrEqual(0);
  let raw = "";
  let i = open + 1;
  while (i < src.length) {
    const c = src[i];
    if (c === "\\") {
      raw += c + src[i + 1];
      i += 2;
      continue;
    }
    if (c === '"') break;
    raw += c;
    i++;
  }
  return decodeRustLiteral(raw);
}

describe("DEFAULT_RESEARCH_PROMPTS mirrors the backend", () => {
  const src = configRs;

  const cases: Array<[keyof typeof DEFAULT_RESEARCH_PROMPTS, string]> = [
    ["plan_queries", "default_plan_queries"],
    ["select_sources", "default_select_sources"],
    ["assess_sufficiency", "default_assess_sufficiency"],
    ["refine_queries", "default_refine_queries"],
    ["synthesize", "default_synthesize"],
  ];

  for (const [key, fnName] of cases) {
    it(`${key} matches ${fnName}() byte-for-byte`, () => {
      expect(DEFAULT_RESEARCH_PROMPTS[key]).toBe(extractRustDefault(src, fnName));
    });
  }
});
