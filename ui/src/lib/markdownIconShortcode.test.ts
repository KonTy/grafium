import { describe, expect, it } from "vitest";
import { renderBlock } from "./markdown";

describe("markdown icon shortcodes", () => {
  it("renders known icon shortcodes as inline icons", () => {
    const html = renderBlock("Ship it :icon-rocket: :icon-star:");
    expect(html).toContain("grafium-icon");
    expect(html).toContain("aria-label=\"star\"");
  });

  it("leaves unknown icon shortcodes alone", () => {
    expect(renderBlock("Keep :icon-not-real: text")).toContain(":icon-not-real:");
  });
});
