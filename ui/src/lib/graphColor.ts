// Colouring the graph by cluster, so hue carries meaning rather than decoration.
//
// The graph view drew every node and edge in one flat accent, which made a
// dense graph a uniform haze: you could see that things were connected but not
// *what belonged with what*. Assigning colour per connected component means a
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

/** Minimal shape this module needs from a rendered edge. */
export interface ClusterEdge {
  source: string;
  target: string;
}

/**
 * Group node ids into connected components via union-find, then assign each
 * component one of [`TAG_HUES`].
 *
 * Components are ranked by size and assigned hues in that order, so the palette
 * is spent on the clusters that dominate the view: the largest cluster always
 * gets the first hue, which keeps colouring stable between renders of the same
 * graph instead of flickering as the layout settles.
 *
 * Isolated nodes (no edges) all share the last hue rather than each consuming
 * one — they carry no grouping information, and letting them eat the palette
 * would leave real clusters sharing colours.
 */
export function assignClusterHues(
  nodeIds: readonly string[],
  edges: readonly ClusterEdge[]
): Map<string, TagHue> {
  const parent = new Map<string, string>();
  for (const id of nodeIds) parent.set(id, id);

  const find = (x: string): string => {
    let root = x;
    while (parent.get(root) !== root) root = parent.get(root) ?? root;
    // Path compression keeps this near-linear on large graphs, which matters
    // because this runs on every data load, not once.
    let cur = x;
    while (parent.get(cur) !== root) {
      const next = parent.get(cur) ?? root;
      parent.set(cur, root);
      cur = next;
    }
    return root;
  };

  for (const edge of edges) {
    if (!parent.has(edge.source) || !parent.has(edge.target)) continue;
    const a = find(edge.source);
    const b = find(edge.target);
    if (a !== b) parent.set(a, b);
  }

  const members = new Map<string, string[]>();
  for (const id of nodeIds) {
    const root = find(id);
    const list = members.get(root);
    if (list) list.push(id);
    else members.set(root, [id]);
  }

  // Largest first; ties broken by root id so the result is deterministic and
  // doesn't depend on Map iteration order for equal-sized clusters.
  const ranked = [...members.entries()].sort(
    (a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0])
  );

  const hues = new Map<string, TagHue>();
  let next = 0;
  for (const [, ids] of ranked) {
    const hue: TagHue =
      ids.length > 1
        ? TAG_HUES[next++ % TAG_HUES.length]
        : TAG_HUES[TAG_HUES.length - 1];
    for (const id of ids) hues.set(id, hue);
  }
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
  const a = hues.get(edge.source);
  const b = hues.get(edge.target);
  return a && b && a === b ? a : null;
}
