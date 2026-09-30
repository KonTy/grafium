export const BOOK_RENDERER_VERSION = "foliate-78914aef4466eb960965702401634c2cb348e9b1-grafium-1";
export interface BookRect { x: number; y: number; width: number; height: number }
export type BookLocation =
  | { kind: "epub"; cfi: string; rendererVersion: string }
  | { kind: "pdf"; page: number; rects?: BookRect[] };

export function isBookLocation(value: unknown): value is BookLocation {
  if (!value || typeof value !== "object") return false;
  const v = value as Record<string, unknown>;
  if (v.kind === "epub") return typeof v.cfi === "string" && v.cfi.length <= 16384
    && /^epubcfi\(.+\)$/.test(v.cfi) && typeof v.rendererVersion === "string" && v.rendererVersion.length < 200;
  if (v.kind !== "pdf" || !Number.isSafeInteger(v.page) || (v.page as number) < 1) return false;
  return v.rects === undefined || (Array.isArray(v.rects) && v.rects.length <= 1000 && v.rects.every(r =>
    r && ["x", "y", "width", "height"].every(k => typeof r[k] === "number" && Number.isFinite(r[k])
      && r[k] >= 0 && r[k] <= 1) && r.x + r.width <= 1.000001 && r.y + r.height <= 1.000001));
}
