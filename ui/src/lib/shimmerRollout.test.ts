import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const read = (p: string) => readFileSync(join(process.cwd(), "src", p), "utf8");

const css = read("styles/global.css");

/** Comments discuss declarations they do not make; strip them before asserting. */
const declarationsOf = (rule: string) => rule.replace(/\/\*[\s\S]*?\*\//g, "");

describe("global shimmer", () => {
  it("expires by default instead of looping forever", () => {
    const from = css.indexOf(".shimmer {");
    const rule = css.slice(from, css.indexOf("\n}", from));
    expect(rule).toContain("var(--shimmer-cycles, 6)");
    expect(rule).not.toContain("infinite");
  });

  it("does not tile the gradient, which is what made it flash between sweeps", () => {
    const from = css.indexOf(".shimmer {");
    const rule = css.slice(from, css.indexOf("\n}", from));
    expect(rule).toContain("background-repeat: no-repeat");
  });

  it("keeps both ends of the sweep off the text so the loop restart is unseen", () => {
    const frames = css.slice(css.indexOf("@keyframes shimmer-sweep"));
    const body = frames.slice(0, frames.indexOf("\n}"));
    // With a 300%-wide image, 100%..0% is exactly the range over which the
    // image still covers the text -- and both ends park the highlight outside.
    expect(body).toContain("background-position: 100% 0");
    expect(body).toContain("background-position: 0% 0");
    expect(body).not.toMatch(/-\d+%/);
  });

  it("runs at 3.15s, twice the speed first tried", () => {
    const from = css.indexOf(".shimmer {");
    const rule = css.slice(from, css.indexOf("\n}", from));
    expect(rule).toContain("var(--shimmer-duration, 3.15s)");
  });

  it("fills the glyphs back in when the sweep expires", () => {
    // Without this the text is stranded mid-gradient and partly invisible.
    expect(css).toContain("shimmer-settle");
    const settle = css.slice(css.indexOf("@keyframes shimmer-settle"));
    expect(settle).toContain("-webkit-text-fill-color: currentColor");
  });

  it("sweeps the theme foreground over neutral grey, even when the foreground is white", () => {
    const from = css.indexOf(".shimmer {");
    const rule = declarationsOf(css.slice(from, css.indexOf("\n}", from)));
    expect(rule).toContain("#767676 37%");
    expect(rule).toContain("#767676 63%");
    expect(rule).toContain("var(--text-primary, currentColor) 50%");
    expect(rule).not.toContain("var(--text-secondary)");
    expect(rule).not.toContain("--shimmer-peak");
    expect(rule).not.toContain("color-mix");
    expect(rule).not.toContain("#ffffff");
  });

  it("never sets color:transparent, which would erase currentColor too", () => {
    const from = css.indexOf(".shimmer {");
    const rule = declarationsOf(css.slice(from, css.indexOf("\n}", from)));
    expect(rule).not.toMatch(/(^|[^-])color: transparent/m);
    expect(rule).toContain("-webkit-text-fill-color: transparent");
  });

  it("offers an endless variant only for evidence-backed indicators", () => {
    expect(css).toMatch(/\.shimmer-endless\s*{[^}]*infinite/);
  });

  it("stays readable with no motion under reduced motion", () => {
    // Other features also have reduced-motion blocks; check the shimmer's own.
    const blocks: string[] = [];
    for (let at = css.indexOf("prefers-reduced-motion"); at >= 0; at = css.indexOf("prefers-reduced-motion", at + 1)) {
      let depth = 0, end = css.indexOf("{", at);
      for (let i = end; i < css.length; i++) {
        depth += css[i] === "{" ? 1 : css[i] === "}" ? -1 : 0;
        if (depth === 0) { end = i; break; }
      }
      blocks.push(css.slice(at, end + 1));
    }
    const reduced = blocks.find((block) => block.includes(".shimmer")) ?? "";
    expect(reduced).toContain(".shimmer");
    expect(reduced).toContain("background-image: none");
    expect(reduced).toContain("animation: none !important");
  });
});

describe("busy labels shimmer", () => {
  const cases: Array<[string, string]> = [
    ["components/AISettings.svelte", "Loading configuration"],
    ["components/HelpOverlay.svelte", "Loading help"],
    ["components/GlobalSearchDialog.svelte", "Searching note titles"],
    ["components/GraphView.svelte", "Building graph"],
    ["components/JournalView.svelte", "Loading journals"],
    ["components/GoToLink.svelte", "Loading pages"],
    ["components/FolderBrowser.svelte", "Loading..."],
    ["components/Statistics.svelte", "Loading tasks"],
    ["components/UnifiedPageEditor.svelte", "Loading source"],
    ["components/ReadingNotesPanel.svelte", "Loading notes"],
    ["components/ReferencePanel.svelte", "Loading conflicts"],
    ["components/AllPages.svelte", "tree…"],
    ["components/LazyView.svelte", "Loading {name}"],
    ["components/PageContent.svelte", "Scanning for unlinked"],
    ["components/BlockEditor.svelte", "Saving pasted blocks"],
    ["components/FlashcardReview.svelte", "Loading…"],
  ];

  for (const [file, label] of cases) {
    it(`${file} shimmers "${label}"`, () => {
      const src = read(file);
      const shimmers = src.split("\n").some((l) => l.includes(label) && l.includes("shimmer"));
      expect(shimmers).toBe(true);
    });
  }

  it("uses the endless variant for job labels, which have real progress events", () => {
    const jobs = read("components/JobsView.svelte");
    expect(jobs).toContain("class:shimmer={running.length > 0}");
    expect(jobs).toContain("class:shimmer-endless={running.length > 0}");
  });

  it("keeps the chat trail endless, since it withholds shimmer on its own evidence", () => {
    const trail = read("components/ChatStatusTrail.svelte");
    expect(trail).toContain("class:shimmer-endless={row.shimmer}");
    // The sweep itself must not be redefined locally any more.
    expect(trail).not.toContain("@keyframes");
  });
});

describe("shimmer foreground per theme", () => {
  it("covers every theme, including those with identical text colours", async () => {
    const { themes, applyTheme } = await import("./themes");
    const identical = themes.filter((t) => t.colors.textPrimary === t.colors.textSecondary);
    expect(identical.length).toBeGreaterThan(0);

    for (const theme of themes) {
      applyTheme(theme.colors);
      const peak = document.documentElement.style.getPropertyValue("--text-primary");
      expect(peak, `${theme.id} must keep its foreground highlight`).toBe(theme.colors.textPrimary);
    }
  });
});
