import { describe, it, expect } from "vitest";
import {
  FORMATTING_SLASH_COMMANDS,
  CALLOUT_SLASH_COMMANDS,
  ANGLE_TEMPLATE_COMMANDS,
  type SlashCommand,
} from "./slashCommands";
import { CALLOUT_KINDS } from "./callouts";

function byLabel(list: SlashCommand[], label: string): SlashCommand {
  const cmd = list.find((c) => c.label === label);
  if (!cmd) throw new Error(`missing command ${label}`);
  return cmd;
}

/** The character the cursor sits on after inserting `apply` at offset. */
function charAtCursor(cmd: SlashCommand): string {
  const offset = cmd.cursorOffset ?? cmd.apply.length;
  return cmd.apply.slice(offset, offset + 1);
}

describe("formatting slash commands", () => {
  it("quote inserts '> ' with the cursor after the space", () => {
    const cmd = byLabel(FORMATTING_SLASH_COMMANDS, "/quote");
    expect(cmd.apply).toBe("> ");
    expect(cmd.cursorOffset).toBe(2);
  });

  it("headings insert the right hashes with the cursor after the space", () => {
    const h1 = byLabel(FORMATTING_SLASH_COMMANDS, "/heading 1");
    expect(h1.apply).toBe("# ");
    expect(h1.cursorOffset).toBe(2);

    const h2 = byLabel(FORMATTING_SLASH_COMMANDS, "/heading 2");
    expect(h2.apply).toBe("## ");
    expect(h2.cursorOffset).toBe(3);

    const h3 = byLabel(FORMATTING_SLASH_COMMANDS, "/heading 3");
    expect(h3.apply).toBe("### ");
    expect(h3.cursorOffset).toBe(4);
  });

  it("code inserts a fenced block with the cursor on the middle blank line", () => {
    const cmd = byLabel(FORMATTING_SLASH_COMMANDS, "/code");
    expect(cmd.apply).toBe("```\n\n```");
    expect(cmd.cursorOffset).toBe(4);
    // offset 4 is the start of the empty middle line
    expect(cmd.apply.slice(0, cmd.cursorOffset!)).toBe("```\n");
    expect(charAtCursor(cmd)).toBe("\n");
  });

  it("has one /callout entry per kind, cursor on the blank body line", () => {
    for (const kind of CALLOUT_KINDS) {
      const cmd = byLabel(FORMATTING_SLASH_COMMANDS, `/callout ${kind}`);
      const tag = kind.toUpperCase();
      expect(cmd.apply).toBe(`#+BEGIN_${tag}\n\n#+END_${tag}`);
      expect(cmd.cursorOffset).toBe(`#+BEGIN_${tag}\n`.length);
      // cursor sits at the start of the empty body line
      expect(cmd.apply.slice(0, cmd.cursorOffset!)).toBe(`#+BEGIN_${tag}\n`);
      expect(charAtCursor(cmd)).toBe("\n");
    }
  });

  it("orders formatting entries after task-style entries (callouts share list)", () => {
    expect(CALLOUT_SLASH_COMMANDS).toHaveLength(CALLOUT_KINDS.length);
    // every callout command also appears in the formatting list
    for (const c of CALLOUT_SLASH_COMMANDS) {
      expect(FORMATTING_SLASH_COMMANDS).toContainEqual(c);
    }
  });
});

describe("angle-bracket template commands", () => {
  it("has one entry per callout kind inserting the same body as slash callouts", () => {
    for (const kind of CALLOUT_KINDS) {
      const cmd = byLabel(ANGLE_TEMPLATE_COMMANDS, `< ${kind}`);
      const tag = kind.toUpperCase();
      expect(cmd.apply).toBe(`#+BEGIN_${tag}\n\n#+END_${tag}`);
      expect(cmd.cursorOffset).toBe(`#+BEGIN_${tag}\n`.length);
    }
  });
});
