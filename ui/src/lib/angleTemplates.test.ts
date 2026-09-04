import { describe, it, expect } from "vitest";
import { ANGLE_TEMPLATE_COMMANDS } from "./slashCommands";
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
