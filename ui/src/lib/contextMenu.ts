export interface MenuPosition {
  x: number;
  y: number;
}

export interface ContextMenuPositionOptions {
  width?: number;
  height?: number;
  margin?: number;
  viewportWidth?: number;
  viewportHeight?: number;
}

export function clampContextMenuPosition(
  x: number,
  y: number,
  {
    width = 220,
    height = 240,
    margin = 8,
    viewportWidth = typeof window === "undefined" ? Number.POSITIVE_INFINITY : window.innerWidth,
    viewportHeight = typeof window === "undefined" ? Number.POSITIVE_INFINITY : window.innerHeight,
  }: ContextMenuPositionOptions = {}
): MenuPosition {
  if (!Number.isFinite(viewportWidth) || !Number.isFinite(viewportHeight)) {
    return { x, y };
  }

  const maxX = Math.max(margin, viewportWidth - width - margin);
  const maxY = Math.max(margin, viewportHeight - height - margin);

  return {
    x: Math.min(Math.max(x, margin), maxX),
    y: Math.min(Math.max(y, margin), maxY),
  };
}

export interface PlaceContextMenuInput {
  /** Where the menu opens, in viewport pixels (`event.clientX/clientY`). */
  anchor: MenuPosition;
  /**
   * When the menu has to open on the other side of the anchor (above it, or to
   * its left), the edge to flip around. A menu opened from a button flips
   * around the button's top edge rather than its bottom.
   */
  flipAnchor?: MenuPosition;
  /** The menu's rendered size in viewport pixels (`getBoundingClientRect()`). */
  size: { width: number; height: number };
  /** The viewport in viewport pixels (`innerWidth` / `innerHeight`). */
  viewport: { width: number; height: number };
  /** Rendered pixels per CSS pixel inside the menu's zoomed subtree. */
  zoom?: number;
  /** Where `left: 0; top: 0` lands in the viewport; not the origin under a transformed ancestor. */
  origin?: MenuPosition;
  margin?: number;
}

export interface PlacedContextMenu {
  /** `style.left`, in the menu's own (zoomed) CSS pixels. */
  left: number;
  /** `style.top`, in the menu's own (zoomed) CSS pixels. */
  top: number;
  /** The tallest the menu may be, in its own CSS pixels, before it scrolls. */
  maxHeight: number;
}

/**
 * Rendered pixels per CSS pixel for an element of known CSS width.
 *
 * The app zooms the whole document with CSS `zoom`. Pointer coordinates,
 * `getBoundingClientRect()` and `innerWidth` are then in viewport pixels, but a
 * `position: fixed` element's `left`/`top` are in zoomed CSS pixels, so a menu
 * placed straight at `clientX/clientY` lands `zoom` times too far from the
 * cursor. Measuring a ruler of known width, rather than reading the zoom
 * setting, stays correct whichever way an engine applies it.
 */
export function calibrateZoom(renderedWidth: number, cssWidth: number): number {
  const zoom = renderedWidth / cssWidth;
  return Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
}

function placeOnAxis(
  anchor: number,
  flipAnchor: number,
  size: number,
  viewport: number,
  margin: number
): number {
  if (anchor + size + margin <= viewport) return Math.max(anchor, margin);
  const flipped = flipAnchor - size;
  if (flipped >= margin) return flipped;
  return Math.max(margin, viewport - size - margin);
}

/**
 * Decides where a fixed-position menu goes so it opens at the anchor, flips to
 * the other side of it when it would run off the screen, and never leaves the
 * viewport. Layout happens in viewport pixels; only the final values are
 * converted into the zoomed CSS pixels the style needs.
 */
export function placeContextMenu({
  anchor,
  flipAnchor = anchor,
  size,
  viewport,
  zoom = 1,
  origin = { x: 0, y: 0 },
  margin = 8,
}: PlaceContextMenuInput): PlacedContextMenu {
  const available = Math.max(0, viewport.height - margin * 2);
  const height = Math.min(size.height, available);
  const x = placeOnAxis(anchor.x, flipAnchor.x, size.width, viewport.width, margin);
  const y = placeOnAxis(anchor.y, flipAnchor.y, height, viewport.height, margin);

  return {
    left: (x - origin.x) / zoom,
    top: (y - origin.y) / zoom,
    maxHeight: available / zoom,
  };
}
