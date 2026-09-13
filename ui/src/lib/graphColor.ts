// Colouring the graph by cluster, so hue carries meaning rather than decoration.
//
// The graph view drew every node and edge in one flat accent, which made a
// dense graph a uniform haze: you could see that things were connected but not
// *what belonged with what*. Assigning colour per structural community means a
// topic cluster reads as one colour group at a glance, and an edge crossing
// between two clusters is visibly a bridge.
//
// Hues come from the same eight semantic accents used for `#tags`
// (`tagColor.ts`), so a palette the user already recognises — and which is
// contrast-checked per theme by `themeContrast.test.ts` — is reused rather than
// a second, unchecked one being invented for the canvas.
//
// Kept DOM- and theme-free so it stays a pure, unit-testable function; callers
// resolve the returned hue name to a `--accent-<hue>` custom property.

import { TAG_HUES, type TagHue } from "./tagColor";
import { computeGraphClusters, type GraphEdgeLike } from "./graphClusters";

/** Minimal shape this module needs from a rendered edge. */
export type ClusterEdge = GraphEdgeLike;

// Identity must survive palette cycling without changing the public Map API.
const assignments = new WeakMap<Map<string, TagHue>, Map<string, number>>();

/** Shared Louvain assignment, ranked by size; isolates retain the legacy hue. */
export function assignClusterHues(
  nodeIds: readonly string[],
  edges: readonly ClusterEdge[]
): Map<string, TagHue> {
  const { clusterIndexById, isolatedIds } = computeGraphClusters(nodeIds, edges);
  const hues = new Map<string, TagHue>();
  for (const [id, cluster] of clusterIndexById) {
    hues.set(id, TAG_HUES[cluster % TAG_HUES.length]);
  }
  for (const id of isolatedIds) hues.set(id, TAG_HUES[TAG_HUES.length - 1]);
  assignments.set(hues, clusterIndexById);
  return hues;
}

/**
 * The hue an edge should take: its endpoints' colour when they share a cluster.
 *
 * An edge inside a cluster reinforces that group's colour. An edge *between*
 * clusters has no single owner, so it returns `null` and the caller draws it in
 * a neutral border colour — which is the honest rendering, and incidentally
 * makes bridges between topics stand out as the uncoloured lines.
 */
export function edgeHue(
  hues: Map<string, TagHue>,
  edge: ClusterEdge
): TagHue | null {
  if (edge.suggested) return null;
  const clusters = assignments.get(hues);
  if (clusters) {
    const source = clusters.get(edge.source);
    if (source === undefined || source !== clusters.get(edge.target)) return null;
  } else {
    // A copied/hand-built palette has no community identity. Equal colors alone
    // cannot prove membership; conservatively keep its links neutral.
    return null;
  }
  const a = hues.get(edge.source);
  const b = hues.get(edge.target);
  return a && b && a === b ? a : null;
}

/**
 * Movement, in screen pixels, past which a press counts as a drag rather than
 * a click.
 *
 * Treating any movement at all as a drag made graph nodes effectively
 * unclickable: pointers emit sub-pixel moves between press and release on
 * nearly every real click, so the gesture was classified as a drag and
 * navigation never fired. A few pixels of slop is what every drag
 * implementation needs and what users expect.
 */
export const DRAG_THRESHOLD_PX = 4;

/** Whether a press that started at (`x0`,`y0`) and is now at (`x1`,`y1`) has
 *  travelled far enough to be a drag. */
export function exceedsDragThreshold(
  x0: number,
  y0: number,
  x1: number,
  y1: number
): boolean {
  const dx = x1 - x0;
  const dy = y1 - y0;
  // Squared comparison avoids a sqrt on every pointermove.
  return dx * dx + dy * dy > DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX;
}
