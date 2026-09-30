// PageContent intentionally does not expose UnifiedPageEditor. App-level browser
// permutations must match that contract, not rewrite the feature gate in transit.
// Retained component selection, native undo, note navigation and revision conflicts
// run in the isolated UnifiedEditorHarness through keyboardSelection/readingNotes.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");

function applicationEditorCases(cases, { standaloneComponent = false } = {}) {
  const source = readFileSync(path.join(__dirname, "../src/components/PageContent.svelte"), "utf8");
  assert.match(source, /const SHOW_UNIFIED_EDITOR_PROTOTYPE = false;/,
    "Revisit browser coverage when the continuous editor becomes a supported app mode");
  const supported = cases.filter(([, options]) => !options.unifiedDays?.length
    && (!options.unifiedPage || standaloneComponent && !options.journal));
  const unsupported = cases.length - supported.length;
  if (unsupported) console.log(`COVERAGE: ${unsupported} disabled prototype app permutations are not advertised as passing. `
    + "The isolated continuous-editor component is covered by keyboardSelection and readingNotes; shipped app scenarios use the classic editor.");
  return supported;
}
module.exports = { applicationEditorCases };
