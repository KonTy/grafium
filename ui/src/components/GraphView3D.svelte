<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import ForceGraph3D, { type ForceGraph3DInstance } from "3d-force-graph";
  import SpriteText from "three-spritetext";
  import type { Object3D, Vector3 } from "three";
  import { getGraphData, type GraphData } from "../lib/api";
  import { clusterColor, computeGraphClusters } from "../lib/graphClusters";

  interface Props {
    onNavigate: (title: string) => void;
    currentPageId?: string;
    currentPageTitle?: string;
  }

  let { onNavigate, currentPageId = "", currentPageTitle = "" }: Props = $props();

  interface Node3D {
    id: string;
    name: string;
    degree: number;
  }
  interface Link3D {
    source: string;
    target: string;
    weight: number;
  }

  let mode = $state<"global" | "local">("global");
  let nodeLimit = $state(200);
  let showLabels = $state(true);

  let loading = $state(false);
  let errorMsg = $state<string | null>(null);
  let stats = $state({ nodes: 0, edges: 0 });

  let wrapperEl: HTMLDivElement | null = $state(null);
  // Not reactive state on purpose — this holds a live Three.js/WebGL
  // instance that mutates its own internal render loop; wrapping it in
  // Svelte's $state would just add overhead for no benefit.
  let graph: ForceGraph3DInstance<Node3D, Link3D> | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let themeObserver: MutationObserver | null = null;

  let isLightTheme = $state(false);

  // Connected components (see lib/graphClusters.ts): nodes reachable from
  // each other through links count as the same "cluster" and share a
  // color, so a densely-linked group of pages reads as one color family
  // instead of a uniform accent-colored blob. Isolated nodes (no links at
  // all) get a plain muted color instead of a palette slot.
  let clusterIndexById = new Map<string, number>();
  let isolatedIds = new Set<string>();

  function nodeColorFor(n: Node3D): string {
    if (isolatedIds.has(n.id)) return themeColor("--text-muted", isLightTheme ? "#888" : "#aaa");
    return clusterColor(clusterIndexById.get(n.id) ?? 0, isLightTheme);
  }

  // Links keep string ids only until 3d-force-graph binds the graph data,
  // at which point it mutates each link's `source`/`target` in place to
  // point at the actual node objects instead — so both shapes have to be
  // handled here.
  function linkEndpointId(endpoint: string | Node3D): string {
    return typeof endpoint === "string" ? endpoint : endpoint.id;
  }

  function linkColorFor(l: Link3D): string {
    const sourceId = linkEndpointId(l.source as unknown as string | Node3D);
    if (isolatedIds.has(sourceId)) return themeColor("--text-secondary", isLightTheme ? "#666" : "#aaa");
    return clusterColor(clusterIndexById.get(sourceId) ?? 0, isLightTheme);
  }

  function detectIsLightTheme(): boolean {
    return document.documentElement.style.colorScheme === "light";
  }

  function refreshColors(): void {
    if (!graph) return;
    // Re-invoking the accessor setters (rather than relying on the closure
    // alone) is what makes three-forcegraph actually recompute node/link
    // materials — it only redraws colors when the accessor function
    // *reference* changes, not just when the values it reads change.
    graph.nodeColor((n) => nodeColorFor(n)).linkColor((l) => linkColorFor(l));
  }

  // Persistent floating text labels above each node, rendered via
  // three-spritetext. `nodeLabel` (used elsewhere in this file) only
  // controls the hover tooltip — it never puts text in the 3D scene
  // itself, so without this every node was silently unlabeled unless
  // hovered.
  function makeNodeLabel(n: Node3D): Object3D | null {
    if (!showLabels) return null;
    const sprite = new SpriteText(n.name);
    sprite.textHeight = 3.2;
    sprite.color = themeColor("--text-primary", "#eee");
    sprite.backgroundColor = themeColor("--bg-secondary", "#1e1e2e") + "cc";
    sprite.padding = 2;
    sprite.borderRadius = 3;
    // Float above the node's sphere instead of overlapping it — spheres
    // are sized via nodeVal (Math.sqrt(degree + 1)) in onMount below, so
    // this reproduces roughly the same scale without needing to read the
    // actual rendered geometry back out.
    sprite.position.set(0, 6 + Math.sqrt(n.degree + 1) * 1.5, 0);
    return sprite as unknown as Object3D;
  }

  function themeColor(varName: string, fallback: string): string {
    if (typeof window === "undefined") return fallback;
    const v = getComputedStyle(document.documentElement).getPropertyValue(varName).trim();
    return v || fallback;
  }

  async function loadData() {
    if (!graph) return;
    loading = true;
    errorMsg = null;
    try {
      const focus = mode === "local" ? currentPageId || undefined : undefined;
      const data: GraphData = await getGraphData(nodeLimit, focus);
      const nodes: Node3D[] = data.nodes.map((n) => ({ id: n.id, name: n.title, degree: n.degree }));
      const links: Link3D[] = data.edges.map((e) => ({ source: e.source, target: e.target, weight: e.weight }));
      const clusters = computeGraphClusters(
        nodes.map((n) => n.id),
        links
      );
      clusterIndexById = clusters.clusterIndexById;
      isolatedIds = clusters.isolatedIds;
      graph.graphData({ nodes, links });
      refreshColors();
      updateLabelVisibility();
      stats = { nodes: nodes.length, edges: links.length };
    } catch (e) {
      errorMsg = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  // Ctrl+drag rolls the camera around its own view axis (Z-axis in screen
  // space) instead of orbiting around the graph — OrbitControls (used for
  // the normal drag-to-orbit behavior) has no built-in roll, so this
  // manually rotates the camera's `up` vector around the camera→target
  // axis while Ctrl is held, then hands control back to OrbitControls.
  const ROLL_RADIANS_PER_PIXEL = 0.006;
  let isRolling = false;
  let rollLastX = 0;

  function rollCamera(deltaAngle: number): void {
    if (!graph) return;
    const camera = graph.camera();
    const controls = graph.controls() as { target: Vector3 };
    const axis = camera.position.clone().sub(controls.target).normalize();
    camera.up.applyAxisAngle(axis, deltaAngle);
  }

  function setOrbitControlsEnabled(enabled: boolean): void {
    if (!graph) return;
    (graph.controls() as { enabled: boolean }).enabled = enabled;
  }

  function handlePointerDown(event: PointerEvent): void {
    if (!event.ctrlKey || event.button !== 0 || !graph || !wrapperEl) return;
    isRolling = true;
    rollLastX = event.clientX;
    setOrbitControlsEnabled(false);
    wrapperEl.setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function handlePointerMove(event: PointerEvent): void {
    if (!isRolling) return;
    const deltaX = event.clientX - rollLastX;
    rollLastX = event.clientX;
    rollCamera(deltaX * ROLL_RADIANS_PER_PIXEL);
  }

  function endRoll(event: PointerEvent): void {
    if (!isRolling) return;
    isRolling = false;
    setOrbitControlsEnabled(true);
    wrapperEl?.releasePointerCapture(event.pointerId);
  }

  onMount(() => {
    if (!wrapperEl) return;
    const bgColor = themeColor("--bg-primary", "#16161e");
    isLightTheme = detectIsLightTheme();

    // `controlType: "orbit"` is what gives us the "solar system" feel:
    // dragging orbits the camera around the graph in 3D (Three.js
    // OrbitControls under the hood), scroll zooms, right-drag/two-finger
    // pans. It's a constructor-only option (not chainable), per the
    // library's types.
    graph = new ForceGraph3D(wrapperEl, {
      controlType: "orbit",
    }) as unknown as ForceGraph3DInstance<Node3D, Link3D>;
    graph
      .backgroundColor(bgColor)
      .nodeId("id")
      .nodeLabel((n) => n.name)
      .nodeVal((n) => Math.max(1, Math.sqrt(n.degree + 1)))
      .nodeColor((n) => nodeColorFor(n))
      .nodeOpacity(0.9)
      .linkSource("source")
      .linkTarget("target")
      .linkColor((l) => linkColorFor(l))
      .linkOpacity(0.45)
      .linkWidth((l) => Math.max(0.5, Math.sqrt(l.weight)))
      .linkDirectionalParticles(0)
      .nodeThreeObjectExtend(true)
      .nodeThreeObject((n) => makeNodeLabel(n) as unknown as Object3D)
      .showNavInfo(false)
      .onNodeClick((n) => onNavigate(n.name))
      .onNodeHover((n) => {
        if (wrapperEl) wrapperEl.style.cursor = n ? "pointer" : "grab";
      });

    updateLabelVisibility();
    void loadData();

    resizeObserver = new ResizeObserver(() => {
      if (wrapperEl && graph) {
        graph.width(wrapperEl.clientWidth).height(wrapperEl.clientHeight);
      }
    });
    resizeObserver.observe(wrapperEl);

    // `applyTheme` (lib/themes.ts) sets colors via `root.style.setProperty`
    // and `root.style.colorScheme`, so watching the `style` attribute is
    // enough to catch a live theme switch without any dedicated event.
    themeObserver = new MutationObserver(() => {
      const nextIsLight = detectIsLightTheme();
      if (nextIsLight === isLightTheme) return;
      isLightTheme = nextIsLight;
      graph?.backgroundColor(themeColor("--bg-primary", "#16161e"));
      refreshColors();
      updateLabelVisibility();
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });

    wrapperEl.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", endRoll);
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    themeObserver?.disconnect();
    wrapperEl?.removeEventListener("pointerdown", handlePointerDown);
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", endRoll);
    graph?._destructor();
    graph = null;
  });

  function updateLabelVisibility() {
    if (!graph) return;
    // A fresh closure is required — three-forcegraph only rebuilds node
    // objects when the `nodeThreeObject` accessor's *reference* changes,
    // not just when the values it reads (like `showLabels`) change.
    graph.nodeThreeObject((n) => makeNodeLabel(n) as unknown as Object3D);
  }

  function resetView() {
    if (graph) graph.camera().up.set(0, 1, 0);
    graph?.cameraPosition({ x: 0, y: 0, z: 400 }, { x: 0, y: 0, z: 0 }, 600);
  }

  $effect(() => {
    // Re-run whenever mode/nodeLimit/currentPageId changes.
    mode;
    nodeLimit;
    currentPageId;
    void loadData();
  });

  $effect(() => {
    showLabels;
    updateLabelVisibility();
  });
</script>

<div class="graph-view-3d">
  <div class="graph-canvas-wrap" bind:this={wrapperEl}>
    {#if loading}
      <div class="graph-overlay">Building graph…</div>
    {:else if errorMsg}
      <div class="graph-overlay error">{errorMsg}</div>
    {:else if stats.nodes === 0}
      <div class="graph-overlay">
        No links to display{mode === "local" ? " for this page" : ""}.
      </div>
    {/if}

    <div class="zoom-controls">
      <button title="Reset view" onclick={resetView}>⤢</button>
    </div>
  </div>

  <aside class="graph-controls">
    <h2>Graph (3D)</h2>

    <div class="mode-toggle">
      <button class:active={mode === "global"} onclick={() => (mode = "global")}>Global</button>
      <button
        class:active={mode === "local"}
        disabled={!currentPageId}
        title={currentPageId ? "" : "Open a page first"}
        onclick={() => (mode = "local")}
      >Local</button>
    </div>

    {#if mode === "local" && currentPageTitle}
      <div class="focus-label">Around <strong>{currentPageTitle}</strong></div>
    {/if}

    <label class="ctrl">
      <span>Max nodes: {nodeLimit}</span>
      <input type="range" min="20" max="400" step="20" bind:value={nodeLimit} />
    </label>

    <label class="ctrl checkbox">
      <input type="checkbox" bind:checked={showLabels} />
      <span>Show labels</span>
    </label>

    <div class="graph-stats">
      {stats.nodes.toLocaleString()} nodes · {stats.edges.toLocaleString()} links
    </div>
    <p class="hint">
      Click a node to open it. Drag to orbit, scroll to zoom, right-drag to pan, Ctrl+drag to roll.
    </p>
    <p class="hint">Linked clusters share a color; unlinked pages are shown in a neutral gray.</p>
  </aside>
</div>

<style>
  .graph-view-3d {
    display: flex;
    height: 100%;
    width: 100%;
    position: absolute;
    inset: 0;
  }

  .graph-canvas-wrap {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    cursor: grab;
  }

  .graph-overlay {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    color: var(--text-muted, #aaa);
    font-size: 14px;
    pointer-events: none;
    z-index: 1;
  }

  .graph-overlay.error {
    color: var(--danger, #e74c3c);
  }

  .zoom-controls {
    position: absolute;
    bottom: 16px;
    right: 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    z-index: 2;
  }

  .zoom-controls button {
    width: 32px;
    height: 32px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #333);
    background: var(--bg-secondary, #1e1e2e);
    color: var(--text-primary, #eee);
    cursor: pointer;
    font-size: 16px;
  }

  .zoom-controls button:hover {
    background: var(--bg-hover, #2a2a3d);
  }

  .graph-controls {
    width: 240px;
    flex-shrink: 0;
    border-left: 1px solid var(--border-color, #333);
    background: var(--bg-secondary, #1e1e2e);
    padding: 16px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .graph-controls h2 {
    margin: 0 0 4px;
    font-size: 15px;
  }

  .mode-toggle {
    display: flex;
    gap: 4px;
  }

  .mode-toggle button {
    flex: 1;
    padding: 6px 8px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #333);
    background: var(--bg-primary, #16161e);
    color: var(--text-secondary, #aaa);
    cursor: pointer;
    font-size: 12px;
  }

  .mode-toggle button.active {
    background: var(--accent, #6ea8fe);
    color: #0b0b10;
    border-color: var(--accent, #6ea8fe);
  }

  .mode-toggle button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .focus-label {
    font-size: 12px;
    color: var(--text-muted, #888);
  }

  .ctrl {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-secondary, #aaa);
  }

  .ctrl.checkbox {
    flex-direction: row;
    align-items: center;
    gap: 8px;
  }

  .graph-stats {
    font-size: 12px;
    color: var(--text-muted, #888);
    margin-top: auto;
  }

  .hint {
    font-size: 11px;
    color: var(--text-muted, #666);
    margin: 0;
  }
</style>
