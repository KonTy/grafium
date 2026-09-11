import { describe, expect, it } from "vitest";
import source from "./GraphView3D.svelte?raw";

describe("GraphView3D usability safeguards", () => {
  it("keeps node scale bounded instead of using raw degree", () => {
    expect(source).toContain("const MAX_NODE_VAL");
    expect(source).toContain("function nodeValFor(n: Node3D)");
    expect(source).toContain("degreeRatio(n) * (MAX_NODE_VAL - MIN_NODE_VAL)");
    expect(source).not.toContain("three-spritetext");
    expect(source).not.toContain("nodeThreeObject");
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
    expect(source).toContain("graph.cameraPosition({ x: 0, y: 0, z: distance }");
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

  it("spreads nodes with explicit 3D force settings", () => {
    expect(source).toContain("function configureForces()");
    expect(source).toContain(".strength?.(-520)");
    expect(source).toContain("linkForce?.distance?.(180)");
  });

  it("hides date pages by default in 3D", () => {
    expect(source).toContain("let hideDatePages = $state(true)");
    expect(source).toContain("function isDatePageTitle(title: string)");
    expect(source).toContain("bind:checked={hideDatePages}");
  });
});
