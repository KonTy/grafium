import { describe, expect, it } from "vitest";
import source from "./GraphView3D.svelte?raw";

describe("GraphView3D usability safeguards", () => {
  it("keeps node scale bounded instead of using raw degree", () => {
    expect(source).toContain("const MAX_NODE_VAL");
    expect(source).toContain("function nodeValFor(n: Node3D)");
    expect(source).toContain("degreeRatio(n) * (MAX_NODE_VAL - MIN_NODE_VAL)");
    expect(source).not.toContain("three-spritetext");
    expect(source).not.toContain(".nodeThreeObject(");
  });

  it("keeps labels out of the 3D scene and fits the camera after load", () => {
    expect(source).toContain('{#if hoverNode}');
    expect(source).toContain('class="hover-card"');
    expect(source).toContain('bind:this={graphMountEl}');
    expect(source).toContain('class="graph-label-layer"');
    expect(source).toContain("function updateScreenLabels");
    expect(source).toContain("graph.graph2ScreenCoords");
    expect(source).toContain("new ForceGraph3D(graphMountEl");
    expect(source).toContain("function scheduleFitToGraph()");
    expect(source).toContain("const MIN_CAMERA_DISTANCE");
    expect(source).toContain("latestNodes.filter(hasGraphPosition)");
    expect(source).toContain("halfSize.x / (tanHalfFov * aspect)");
    expect(source).toContain("z: center.z + distance");
  });

  it("uses fuzzy search to glow matches and fly the camera to them", () => {
    expect(source).toContain('import { fuzzyScore } from "../lib/fuzzy"');
    expect(source).toContain('placeholder="Fly to nodes…"');
    expect(source).toContain("function updateSearchMatches()");
    expect(source).toContain("function scheduleFlyToSearchMatches()");
    expect(source).toContain("function flyToSearchMatches()");
    expect(source).toContain("searchMatchIds.has(n.id)");
    expect(source).toContain('class="graph-search-glow-layer"');
    expect(source).toContain("visibleSearchGlows");
  });

  it("keeps detected communities anchored through actual force ticks", () => {
    expect(source).toContain("function configureForces()");
    expect(source).toContain('from "../lib/graphCommunityLayout"');
    expect(source).toContain("communityLayout = createCommunityLayout(");
    expect(source).toContain("communityLinkDistance(");
    expect(source).toContain('graph.d3Force("community", createCommunityForce())');
    expect(source).toContain('graph.d3Force("center", null)');
    expect(source).toContain("isCommunityBridge(link) ? 0.008 : 0.14");
    expect(source).toContain("force.initialize");
    expect(source).not.toContain("computeGraphClusters");
    expect(source).not.toContain("seedNodePositions");
  });

  it("uses renderer-managed, theme-aware solid spheres without image textures", () => {
    expect(source).toContain(".nodeOpacity(1)");
    expect(source).toContain(".nodeResolution(24)");
    expect(source).toContain("clusterColor(clusterIndexById.get(n.id) ?? 0, isLightTheme)");
    expect(source).toContain("graph.nodeColor((n) => nodeColorFor(n))");
    expect(source).toContain('themeObserver.observe(document.documentElement');
    expect(source).toContain("graph?._destructor()");
    expect(source).not.toMatch(/TextureLoader|CanvasTexture|planetTextures|loadPlanetTextures|createPlanetObject/);
  });

  it("does not let suggested links pull communities together", () => {
    expect(source).toContain("linkForce?.strength((link) => link.suggested ? 0 :");
  });

  it("keeps fitted spheres and bridges visible at large community extents", () => {
    expect(source).toContain("const worldPerPixel = 2 * (distance + halfSize.z) * tanHalfFov");
    expect(source).toContain("graph.nodeRelSize(overviewNodeScale)");
    expect(source).toContain("width * overviewLinkScale");
  });

  it("includes date pages by default, matching the 2D graph", () => {
    expect(source).toContain("let hideDatePages = $state(false)");
    expect(source).toContain("function isDatePageTitle(title: string)");
    expect(source).toContain("bind:checked={hideDatePages}");
  });

  it("keeps one decorative universe background in both overview and flight", () => {
    expect(source).toContain('from "../lib/graphUniverse"');
    expect(source).toContain("universeBackground?.setAppearance({ flying, isLightTheme })");
    expect(source).toContain("universeBackground.update(graph.camera(), graph.renderer().getPixelRatio())");
    expect(source).toContain("universeBackground?.dispose()");
    expect(source).toContain("void background.ready.catch");
    expect(source).toContain('role="status">{backgroundError}');
    expect(source).not.toContain("addFlightStars");
  });
});
