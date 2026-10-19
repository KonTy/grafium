import { describe, expect, it } from "vitest";
import { findSpellWords } from "./spellcheckTokens";

const words = (text: string, skip: Array<[number, number]> = []) =>
  findSpellWords(text, skip).map((w) => w.word);

describe("findSpellWords", () => {
  it("reports each word with its position, shifted by the offset", () => {
    expect(findSpellWords("Hello, wrold!")).toEqual([
      { from: 0, to: 5, word: "Hello" },
      { from: 7, to: 12, word: "wrold" },
    ]);
    expect(findSpellWords("Hello, wrold!", [], 100).map((w) => w.from)).toEqual([100, 107]);
  });

  it("drops punctuation that wraps a word", () => {
    expect(words(`'quoted' *bold* _emphasis_ (paren) "double" end. Note: «guillemets» ok…fine`)).toEqual([
      "quoted", "bold", "emphasis", "paren", "double", "end", "Note", "guillemets", "ok", "fine",
    ]);
  });

  it("keeps contractions and hyphenated words whole and trims a trailing apostrophe", () => {
    expect(words("don’t well-known l'homme dogs'")).toEqual(["don’t", "well-known", "l'homme", "dogs"]);
  });

  it("leaves out what is not an ordinary word", () => {
    const skipped = [
      "a I", // one letter
      "NASA HTML", // acronyms
      "camelCase iPhone McDonald O'Brien", // inner capitals
      "abc123 3rd 2024 10:30", // digits
      "snake_case file.txt and/or e.g. U.S. C# a=b 50% x+y", // identifiers and symbols
      "foo@example.com", // e-mail
      "~/notes/todo /etc/hosts", // paths
      "\\*literal\\* >quoted",
    ];
    for (const text of skipped) expect(words(text), text).toEqual([]);
  });

  it("leaves out the app's own syntax", () => {
    const syntax = [
      "[[Kubernetes Cluster]]",
      "[[Kubernetes Cluster|clustar]]",
      "#kubernetes #proj/clustar #[[Two wrods]]",
      "((blokc-ref-id))",
      "{{video https://youtu.be/abc recieve}}",
      "[#A] :icon-starr: [^grafium-note-1]",
      "status:: in progres",
      "- id:: abcdefgh",
      "`inline recieve`",
      "https://example.com/recieve,wrold www.example.org/wrold",
      "<div class=\"wrold\"> </span> <!-- recieve -->",
      "$x^2 + recieve$ $$\nwrold\n$$",
    ];
    for (const text of syntax) expect(words(text), text).toEqual([]);
  });

  it("checks the words around that syntax", () => {
    expect(words("see [[Page]] for the #tag, ok `code` and https://x.io/a, then wrold")).toEqual([
      "see", "for", "the", "ok", "and", "then", "wrold",
    ]);
    expect(words("[link text](https://example.com/a,bcd) after")).toEqual(["link", "text", "after"]);
  });

  it("treats `$5 and $10` as prose, as the renderer does", () => {
    expect(words("I paid $5 for the boook and $10 for pens")).toEqual([
      "paid", "for", "the", "boook", "and", "for", "pens",
    ]);
  });

  it("skips the ranges the caller gives", () => {
    expect(words("alpha beta gamma", [[6, 10]])).toEqual(["alpha", "gamma"]);
    expect(words("alpha beta gamma", [[2, 12]])).toEqual(["al", "amma"]);
    expect(words("alpha beta", [[50, 60], [8, 3]])).toEqual(["alpha", "beta"]);
  });

  it("splits table cells", () => {
    expect(words("|Name|Value|\n|---|---|\n|one|two|")).toEqual(["Name", "Value", "one", "two"]);
  });

  it("handles Spanish and French punctuation", () => {
    expect(words("¿Cómo estás? ¡Muy bien!")).toEqual(["Cómo", "estás", "Muy", "bien"]);
    expect(words("L'école d'accord, peut-être")).toEqual(["L'école", "d'accord", "peut-être"]);
  });

  it("checks Cyrillic, Devanagari and Arabic words", () => {
    expect(words("Привет, мир! Ёлка")).toEqual(["Привет", "мир", "Ёлка"]);
    expect(words("यह एक वाक्य है।अगला")).toEqual(["यह", "एक", "वाक्य", "है", "अगला"]);
    expect(words("مرحبا، بالعالم؟ كتاب")).toEqual(["مرحبا", "بالعالم", "كتاب"]);
  });

  it("keeps joiners inside a word", () => {
    expect(words("می\u200cخواهم")).toEqual(["می\u200cخواهم"]);
  });

  it("ignores very long words", () => {
    expect(words("a".repeat(65))).toEqual([]);
    expect(words("a".repeat(64))).toEqual(["a".repeat(64)]);
  });
});
