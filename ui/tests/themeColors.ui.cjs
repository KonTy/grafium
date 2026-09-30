// Real theme switching and both editors; notes and preferences stay in the IPC fixture.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const { openEditor, focus, frames } = require("./keyboardSelection.ui.cjs");

const headings = Array.from({ length: 6 }, (_, i) => `${"#".repeat(i + 1)} Colorful heading ${i + 1}`);
const blockContents = [
  ...headings,
  "Explore #ideas #science #creative and [[Keyboard selection]]",
  `#+BEGIN_IMPORTANT\nKeep the bright ideas.\n${headings.join("\n")}\n#+END_IMPORTANT`,
  `#+BEGIN_NOTE\nA little blue makes a difference.\n${headings.join("\n")}\n#+END_NOTE`,
  "### Inline `code` stays readable",
  "Normal prose keeps the theme's original text color.",
];

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const continuous of [false, true]) {
      const { page, errors } = await openEditor(browser, {
        unifiedPage: continuous,
        blockContents,
        beforeNavigate: async (page) => {
          if (!continuous) return;
          // Exercise the retained prototype without exposing it in the shipped app.
          await page.route(/\/src\/components\/PageContent\.svelte(?:\?|$)/, async (route) => {
            if (new URL(route.request().url()).searchParams.has("type")) return route.continue();
            const response = await route.fetch();
            const source = await response.text();
            assert.match(source, /const SHOW_UNIFIED_EDITOR_PROTOTYPE = false;/);
            await route.fulfill({
              response,
              body: source.replace("const SHOW_UNIFIED_EDITOR_PROTOTYPE = false;", "const SHOW_UNIFIED_EDITOR_PROTOTYPE = true;"),
            });
          });
        },
      });
      const content = continuous ? ".unified-rendered-content" : ".rendered-content";
      const themes = await page.evaluate(async () => (await import("/src/lib/themes.ts")).themes.map(({ id }) => id));
      for (const id of themes) {
        const results = await page.evaluate(async ({ id, content, continuous }) => {
          const { themes, applyTheme } = await import("/src/lib/themes.ts");
          const { contrastRatio, parseHex } = await import("/src/lib/contrast.ts");
          const { colors } = themes.find((theme) => theme.id === id);
          applyTheme(colors);
          const expected = [colors.accentRed, colors.accentBlue, colors.accentPurple,
            colors.accentCyan, colors.accentOrange, colors.accentMagenta];
          const canvas = document.createElement("canvas");
          canvas.width = canvas.height = 1;
          const context = canvas.getContext("2d", { willReadFrequently: true });
          const rgba = (value) => {
            context.clearRect(0, 0, 1, 1);
            context.fillStyle = value;
            context.fillRect(0, 0, 1, 1);
            return [...context.getImageData(0, 0, 1, 1).data];
          };
          const rgb = (value) => {
            const [r, g, b] = rgba(value);
            return { r, g, b };
          };
          const results = expected.map((color, index) => {
            const element = document.querySelector(`${content} h${index + 1}`);
            return {
              level: index + 1,
              actual: rgb(getComputedStyle(element).color),
              expected: parseHex(color),
              contrast: contrastRatio(rgb(getComputedStyle(element).color),
                continuous ? colors.bgSecondary : colors.bgPrimary),
            };
          });
          const background = (element) => {
            const layers = [];
            for (let parent = element; parent; parent = parent.parentElement) {
              layers.unshift(rgba(getComputedStyle(parent).backgroundColor));
            }
            const [r, g, b] = layers.reduce((base, [r, g, b, a]) =>
              [r, g, b].map((channel, index) => channel * a / 255 + base[index] * (1 - a / 255)), [255, 255, 255]);
            return { r, g, b };
          };
          for (const element of document.querySelectorAll(`${content} :is(h1,h2,h3,h4,h5,h6), ${content} :is(h1,h2,h3,h4,h5,h6) code`)) {
            const actual = rgb(getComputedStyle(element).color);
            results.push({
              level: element.tagName + (element.closest(".callout") ? " in callout" : ""),
              actual, expected: actual,
              contrast: contrastRatio(actual, background(element)),
            });
          }
          return results;
        }, { id, content, continuous });
        for (const result of results) {
          assert.deepEqual(result.actual, result.expected, `${id} H${result.level} uses its live theme accent`);
          assert.ok(result.contrast >= 4.5, `${id} H${result.level} contrast = ${result.contrast}`);
        }
      }
      assert.equal(await page.locator(`${content} .callout-important`).count(), 1);
      assert.equal(await page.locator(`${content} .callout-note`).count(), 1);
      console.log(`PASS all ${themes.length} palettes update six heading colors with AA contrast (${continuous ? "continuous" : "classic"})`);

      await page.evaluate(async () => {
        const { themes, applyTheme } = await import("/src/lib/themes.ts");
        applyTheme(themes.find(({ id }) => id === "matrix").colors);
      });
      for (let level = 1; level <= 6; level++) {
        if (continuous) {
          await page.locator(`${content} h${level}`).first().click();
        } else {
          await focus(page, `b${level - 1}`);
        }
        await frames(page);
        const sourceHeading = continuous ? ".cm-heading-source" : ".cm-line";
        const actual = await page.locator(`.cm-focused .cm-content[contenteditable=true] ${sourceHeading}`)
          .filter({ hasText: headings[level - 1] }).first()
          .evaluate((element) => getComputedStyle(element).color);
        const expected = await page.evaluate((level) => {
          const probe = document.createElement("span");
          probe.style.color = `var(--heading-${level})`;
          document.body.append(probe);
          const color = getComputedStyle(probe).color;
          probe.remove();
          return color;
        }, level);
        assert.equal(actual, expected, `source H${level} matches its preview`);
        await page.getByTitle("Bionic Speedreader", { exact: true }).focus();
        await page.locator(`${content} h${level}`).first().waitFor();
      }
      assert.deepEqual(await page.evaluate(() => window.__selectionState.blocks
        .filter(({ page_id }) => page_id === "selection-page").map(({ content }) => content)), blockContents);
      console.log(`PASS heading colors survive editing without rewriting Markdown (${continuous ? "continuous" : "classic"})`);

      await page.keyboard.press("Alt+s");
      const search = page.getByRole("searchbox", { name: "Filter settings", exact: true });
      await search.fill("theme");
      const themeSection = page.locator('[data-settings-section="theme"]');
      await themeSection.locator(".palette-preview").first().waitFor();
      assert.equal(await themeSection.locator(".palette-preview").count(), themes.length);
      for (const [name, id] of [["Matrix", "matrix"], ["GitHub", "github"], ["OLED Black", "oled"]]) {
        await themeSection.getByRole("button", { name, exact: true }).click();
        await page.waitForFunction((id) => window.__selectionState.theme === id, id);
        const matches = await page.evaluate(async (id) => {
          const { themes } = await import("/src/lib/themes.ts");
          const colors = themes.find((theme) => theme.id === id).colors;
          return document.documentElement.style.getPropertyValue("--accent-red") === colors.accentRed
            && document.documentElement.style.getPropertyValue("--accent-blue") === colors.accentBlue;
        }, id);
        assert.ok(matches, `${name} selection applies its palette`);
      }
      assert.deepEqual(errors, [], "no uncaught browser errors");
      await page.close();
    }
    console.log("PASS theme picker previews and persists the chosen palette");
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
