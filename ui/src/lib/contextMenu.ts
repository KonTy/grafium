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

export function contextMenuPositionFromEvent(
  event: Pick<MouseEvent, "clientX" | "clientY">,
  options?: ContextMenuPositionOptions
): MenuPosition {
  return clampContextMenuPosition(event.clientX, event.clientY, options);
}
