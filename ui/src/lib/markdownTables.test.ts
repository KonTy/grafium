import { describe, expect, it } from "vitest";
import { renderBlock } from "./markdown";

describe("markdown table rendering", () => {
  it("renders loose pipe tables that are missing the markdown delimiter row", () => {
    const html = renderBlock(
      [
        "# Taurine #taurine #protocol #Bioavailability",
        "Very important for stem cells and long life",
        "| Food Source | Taurine Content (per 100g) | Bioavailability |",
        "| **Animal Organs (Heart)** | 400-600 mg | High |",
        "| **Seafood (Fish)** | 200-400 mg | High |",
        "| **Plant-Based Sources** | Negligible to Trace Amounts | Low |",
      ].join("\n")
    );

    expect(html).toContain("<table>");
    expect(html).toContain("<th>Food Source</th>");
    expect(html).toContain("<th>Taurine Content (per 100g)</th>");
    expect(html).toContain("<strong>Animal Organs (Heart)</strong>");
    expect(html).toContain("<td>Low</td>");
  });

  it("does not turn ordinary prose with pipes into a table", () => {
    const html = renderBlock("Use A | B as notation\nThen C | D later");

    expect(html).not.toContain("<table>");
    expect(html).toContain("Use A | B as notation");
  });

  it("renders pipe tables whose delimiter row has one extra cell", () => {
    const html = renderBlock(
      [
        "| Description | A1C (%) |",
        "| ---- | ---- | ---- |",
        "| Optimal | 4.0 - 5.6 |",
        "| Prediabetes | 5.7 - 6.4 |",
        "| Diabetes, controlled | 6.5 - 7.0 |",
        "| Diabetes, needs improvement | 7.1 - 8.0 |",
      ].join("\n")
    );

    expect(html).toContain("<table>");
    expect(html).toContain("<th>Description</th>");
    expect(html).toContain("<th>A1C (%)</th>");
    expect(html).toContain("<td>Prediabetes</td>");
    expect(html).not.toContain("| ---- | ---- | ---- |");
  });

  it("renders pipe tables whose delimiter row is missing trailing cells", () => {
    const html = renderBlock(
      [
        "| Description | A1C (%) | Goal |",
        "| ---- | ---- |",
        "| Optimal | 4.0 - 5.6 | Maintain |",
      ].join("\n")
    );

    expect(html).toContain("<table>");
    expect(html).toContain("<th>Goal</th>");
    expect(html).toContain("<td>Maintain</td>");
  });

  it("does not normalize table-looking text inside fenced code", () => {
    const html = renderBlock(
      [
        "```md",
        "| Description | A1C (%) |",
        "| ---- | ---- | ---- |",
        "| Optimal | 4.0 - 5.6 |",
        "```",
      ].join("\n")
    );

    expect(html).toContain("code-block-wrapper");
    expect(html).not.toContain("<table>");
    expect(html).toContain("| ---- | ---- | ---- |");
  });
});
