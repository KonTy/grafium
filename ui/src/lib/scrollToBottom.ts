/**
 * Keep a streaming transcript pinned to the bottom.
 *
 * `scrollTop = scrollHeight` is clamped by the browser, so it can only ever
 * land exactly on the last pixel of content — which puts the final line flush
 * against the edge of the pane, where it reads as cut off. The scroll
 * containers therefore carry a little bottom padding, and this helper takes a
 * second pass on the next frame because streamed Markdown keeps reflowing after
 * the first paint (a fence closing, a table widening, an image resolving its
 * height), each of which grows `scrollHeight` *after* we already scrolled.
 */
export function scrollToBottom(el: HTMLElement | null | undefined): void {
  if (!el || !el.isConnected) return;
  el.scrollTop = el.scrollHeight;
  if (typeof requestAnimationFrame !== "function") return;
  requestAnimationFrame(() => {
    if (el.isConnected) el.scrollTop = el.scrollHeight;
  });
}

/**
 * Whether the view is close enough to the bottom that it should keep following
 * new content. The tolerance has to comfortably exceed the container's bottom
 * padding, or the padding alone would look like the user had scrolled away and
 * following would switch itself off.
 */
export const FOLLOW_THRESHOLD_PX = 64;

export function isNearBottom(el: HTMLElement | null | undefined): boolean {
  if (!el) return true;
  return el.scrollHeight - el.scrollTop - el.clientHeight < FOLLOW_THRESHOLD_PX;
}
