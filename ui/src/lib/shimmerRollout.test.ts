import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const read = (p: string) => readFileSync(join(process.cwd(), "src", p), "utf8");

const css = read("styles/global.css");

describe("global shimmer", () => {
  it("expires by default instead of looping forever", () => {
    const from = css.indexOf(".shimmer {");
    const rule = css.slice(from, css.indexOf("\n}", from));
    expect(rule).toContain("var(--shimmer-cycles, 10)");
    expect(rule).not.toContain("infinite");
  });

  it("fills the glyphs back in when the sweep expires", () => {
    // Without this the text is stranded mid-gradient and partly invisible.
    expect(css).toContain("shimmer-settle");
    const settle = css.slice(css.indexOf("@keyframes shimmer-settle"));
    expect(settle).toContain("-webkit-text-fill-color: var(--text-primary)");
  });

  it("offers an endless variant only for evidence-backed indicators", () => {
    expect(css).toMatch(/\.shimmer-endless\s*{[^}]*infinite/);
  });

  it("stays readable with no motion under reduced motion", () => {
    const reduced = css.slice(css.lastIndexOf("prefers-reduced-motion"));
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
    ["components/Statistics.svelte", "Loading statistics"],
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
    expect(read("components/JobActivity.svelte")).toContain('class="shimmer shimmer-endless"');
    expect(read("components/JobsView.svelte")).toContain("shimmer shimmer-endless");
  });

  it("keeps the chat trail endless, since it withholds shimmer on its own evidence", () => {
    const trail = read("components/ChatStatusTrail.svelte");
    expect(trail).toContain("class:shimmer-endless={row.shimmer}");
    // The sweep itself must not be redefined locally any more.
    expect(trail).not.toContain("@keyframes");
  });
});
