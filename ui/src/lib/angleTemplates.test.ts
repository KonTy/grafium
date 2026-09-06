import { describe, it, expect } from "vitest";
import { ANGLE_TEMPLATE_COMMANDS, angleTemplateMenu } from "./slashCommands";
import { CALLOUT_KINDS, CALLOUT_META } from "./callouts";
import { renderBlock } from "./markdown";

describe("angle-bracket template menu", () => {
  it("labels every entry with a `< ` prefix, one per callout kind", () => {
    const labels = ANGLE_TEMPLATE_COMMANDS.map((c) => c.label).sort();
    expect(labels).toEqual(CALLOUT_KINDS.map((k) => `< ${k}`).sort());
  });

  it("inserts content that renders as the matching callout", () => {
    for (const kind of CALLOUT_KINDS) {
      const cmd = ANGLE_TEMPLATE_COMMANDS.find((c) => c.label === `< ${kind}`)!;
      // Simulate a typed body on the blank line, then render.
      const withBody =
        cmd.apply.slice(0, cmd.cursorOffset) +
        "hello" +
        cmd.apply.slice(cmd.cursorOffset);
      const html = renderBlock(withBody);
      expect(html).toContain(`class="callout callout-${kind}"`);
      expect(html).toContain(CALLOUT_META[kind].icon);
      expect(html).toContain("hello");
    }
  });
});

describe("angleTemplateMenu guard", () => {
  it("opens at the start of a line and shows all kinds", () => {
    const menu = angleTemplateMenu("<");
    expect(menu).not.toBeNull();
    expect(menu!.from).toBe(0);
    expect(menu!.options.length).toBe(CALLOUT_KINDS.length);
  });

  it("opens right after whitespace", () => {
    const menu = angleTemplateMenu("body <");
    expect(menu).not.toBeNull();
    expect(menu!.from).toBe(5); // position of `<`
  });

  it("filters entries by the typed prefix", () => {
    const menu = angleTemplateMenu("<ti");
    expect(menu).not.toBeNull();
    expect(menu!.options.map((o) => o.label)).toEqual(["< tip"]);
  });

  it("does not trigger immediately after a word character (a<b)", () => {
    expect(angleTemplateMenu("a<")).toBeNull();
    expect(angleTemplateMenu("if (a<b")).toBeNull();
  });

  it("does not trigger for a comparison once a space follows (2 < 3)", () => {
    // After typing `< 3`, the `<` is no longer the token adjacent to the cursor.
    expect(angleTemplateMenu("2 < 3")).toBeNull();
    // And immediately before the space is typed the token still matches, but as
    // soon as a space appears the menu closes.
    expect(angleTemplateMenu("2 < ")).toBeNull();
  });

  it("does not trigger for inline markup like <foo>", () => {
    expect(angleTemplateMenu("<foo>")).toBeNull();
    expect(angleTemplateMenu("text <foo>")).toBeNull();
  });
});
