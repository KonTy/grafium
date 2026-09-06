// Highlighting a search term inside already-rendered page content.
//
// Clicking a search match in the graph drops you on the page with no
// indication of *where* the match is — on a long page that means hunting for
// the thing you just searched for, which defeats the point of searching.
//
// The highlighting is applied to the DOM after render rather than to the
// markdown or the rendered HTML string. That is a deliberate safety choice:
// rewriting HTML with a regex would risk matching inside tags and attributes
// (turning `href="…term…"` into broken markup, or worse, reintroducing the
// injection class the assistant renderer was just hardened against). Walking
// text nodes can only ever touch text, so it cannot alter structure, break a
// link, or create an attribute.

/** Class applied to each wrapped match, styled per-theme by the caller. */
export const HIGHLIGHT_CLASS = "search-highlight";

/**
 * Wrap every case-insensitive occurrence of `term` inside `root` in a
 * `<mark class="search-highlight">`, returning the first wrapped element so the
 * caller can scroll to it.
 *
 * Returns `null` when there is nothing to do — an empty term, or no match —
 * so the caller can skip scrolling rather than jumping to the top of the page.
 */
export function highlightTerm(root: HTMLElement, term: string): HTMLElement | null {
  clearHighlights(root);
  const needle = term.trim().toLowerCase();
  if (!needle) return null;

  // Collect first, mutate second: replacing a text node while the walker is
  // positioned on it invalidates the traversal.
  const targets: Text[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const parent = node.parentElement;
      // Never highlight inside code, or inside an existing highlight: the
      // former changes what the user is reading as literal text, the latter
      // would nest marks on repeated calls.
      if (!parent || parent.closest(`code, pre, .${HIGHLIGHT_CLASS}`)) {
        return NodeFilter.FILTER_REJECT;
      }
      return node.nodeValue?.toLowerCase().includes(needle)
        ? NodeFilter.FILTER_ACCEPT
        : NodeFilter.FILTER_REJECT;
    },
  });
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    targets.push(node as Text);
  }

  let first: HTMLElement | null = null;
  for (const node of targets) {
    const text = node.nodeValue ?? "";
    const lower = text.toLowerCase();
    const fragment = document.createDocumentFragment();
    let cursor = 0;

    for (
      let at = lower.indexOf(needle, cursor);
      at !== -1;
      at = lower.indexOf(needle, cursor)
    ) {
      if (at > cursor) {
        fragment.appendChild(document.createTextNode(text.slice(cursor, at)));
      }
      const mark = document.createElement("mark");
      mark.className = HIGHLIGHT_CLASS;
      // Slice from the original text, not the lowercased copy, so the page
      // keeps its own capitalisation.
      mark.textContent = text.slice(at, at + needle.length);
      fragment.appendChild(mark);
      if (!first) first = mark;
      cursor = at + needle.length;
    }

    if (cursor < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(cursor)));
    }
    node.parentNode?.replaceChild(fragment, node);
  }

  return first;
}

/**
 * Remove previously applied highlights, restoring the original text.
 *
 * `normalize()` merges the text nodes left behind by unwrapping, so repeated
 * highlight/clear cycles can't shatter a paragraph into hundreds of fragments
 * — which would slowly degrade every later traversal of the same content.
 */
export function clearHighlights(root: HTMLElement): void {
  const marks = root.querySelectorAll(`mark.${HIGHLIGHT_CLASS}`);
  for (const mark of Array.from(marks)) {
    mark.replaceWith(document.createTextNode(mark.textContent ?? ""));
  }
  if (marks.length > 0) root.normalize();
}
