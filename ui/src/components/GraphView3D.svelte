<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import ForceGraph3D, { type ForceGraph3DInstance } from "3d-force-graph";
  import { Vector3 } from "three";
  import { getGraphData, type GraphData } from "../lib/api";
  import { fuzzyScore } from "../lib/fuzzy";
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
    x?: number;
    y?: number;
    z?: number;
  }
  interface Link3D {
    source: string;
    target: string;
    weight: number;
  }
  type PositionedNode3D = Node3D & Required<Pick<Node3D, "x" | "y" | "z">>;
  interface ScreenLabel {
    id: string;
    text: string;
    x: number;
    y: number;
    hovered: boolean;
  }
  interface SearchGlow {
    id: string;
    x: number;
    y: number;
    size: number;
  }
  interface LabelCandidate extends ScreenLabel {
    score: number;
    width: number;
  }
  interface LabelRect {
    left: number;
    right: number;
    top: number;
    bottom: number;
  }

  let mode = $state<"global" | "local">("global");
  let nodeLimit = $state(200);
  let hideDatePages = $state(true);
  let showSmartLabels = $state(true);
  let searchText = $state("");

  let loading = $state(false);
  let errorMsg = $state<string | null>(null);
  let stats = $state({ nodes: 0, edges: 0 });
  let visibleLabels = $state<ScreenLabel[]>([]);
  let visibleSearchGlows = $state<SearchGlow[]>([]);
  let searchMatchCount = $state(0);

  let wrapperEl: HTMLDivElement | null = $state(null);
  let graphMountEl: HTMLDivElement | null = $state(null);
  // Not reactive state on purpose — this holds a live Three.js/WebGL
  // instance that mutates its own internal render loop; wrapping it in
  // Svelte's $state would just add overhead for no benefit.
  let graph: ForceGraph3DInstance<Node3D, Link3D> | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let themeObserver: MutationObserver | null = null;
  let isLightTheme = $state(false);
  let hoverNode: Node3D | null = $state(null);
  let maxDegree = 1;
  let fitTimer: number | undefined;
  let labelFrame: number | undefined;
  let searchFlyTimer: number | undefined;
  let lastLabelUpdateMs = 0;
  let pendingFit = false;
  let latestNodes: Node3D[] = [];
  let searchMatchIds = new Set<string>();
  let rankedSearchMatches: Node3D[] = [];

  const MIN_NODE_VAL = 8;
  const MAX_NODE_VAL = 64;
  const MIN_CAMERA_DISTANCE = 620;
  const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));
  const LABEL_UPDATE_INTERVAL_MS = 50;
  const MAX_LABEL_CANDIDATES = 240;
  const LABEL_COLLISION_PADDING = 4;
  const MAX_LABEL_WIDTH = 220;
  const LABEL_HEIGHT = 20;
  const labelCameraDirection = new Vector3();
  const labelNodeVector = new Vector3();

  // Connected components (see lib/graphClusters.ts): nodes reachable from
  // each other through links count as the same "cluster" and share a
  // color, so a densely-linked group of pages reads as one color family
  // instead of a uniform accent-colored blob. Isolated nodes (no links at
  // all) get a plain muted color instead of a palette slot.
  let clusterIndexById = new Map<string, number>();
  let isolatedIds = new Set<string>();

  function nodeColorFor(n: Node3D): string {
    if (hasActiveSearch()) {
      if (searchMatchIds.has(n.id)) return themeColor("--accent-green", "#9ece6a");
      return themeColor("--text-muted", isLightTheme ? "#b8b8b8" : "#4d5360");
    }
    if (isolatedIds.has(n.id)) return themeColor("--text-muted", isLightTheme ? "#888" : "#aaa");
    return clusterColor(clusterIndexById.get(n.id) ?? 0, isLightTheme);
  }

  function degreeRatio(n: Node3D): number {
    return Math.sqrt(Math.max(0, n.degree) / Math.max(1, maxDegree));
  }

  function nodeValFor(n: Node3D): number {
    const base = MIN_NODE_VAL + degreeRatio(n) * (MAX_NODE_VAL - MIN_NODE_VAL);
    return searchMatchIds.has(n.id) ? base * 2.2 + 24 : base;
  }

  function linkWidthFor(l: Link3D): number {
    return Math.min(1.4, Math.max(0.2, 0.25 + Math.sqrt(Math.max(1, l.weight)) * 0.18));
  }

  function labelTextFor(n: Node3D): string {
    return n.name.length > 34 ? `${n.name.slice(0, 33)}…` : n.name;
  }

  function hasActiveSearch(): boolean {
    return searchText.trim().length > 0;
  }

  function updateSearchMatches(): void {
    const query = searchText.trim();
    if (!query) {
      searchMatchIds = new Set();
      rankedSearchMatches = [];
      searchMatchCount = 0;
      visibleSearchGlows = [];
      return;
    }

    const scored: Array<{ node: Node3D; score: number }> = [];
    for (const node of latestNodes) {
      const match = fuzzyScore(node.name, query);
      if (match) {
        scored.push({
          node,
          score: match.score + Math.log1p(Math.max(0, node.degree)) * 6,
        });
      }
    }

    scored.sort((a, b) => b.score - a.score || a.node.name.localeCompare(b.node.name));
    rankedSearchMatches = scored.map((entry) => entry.node);
    searchMatchIds = new Set(rankedSearchMatches.map((node) => node.id));
    searchMatchCount = rankedSearchMatches.length;
  }

  function scheduleFlyToSearchMatches(): void {
    if (searchFlyTimer !== undefined) {
      window.clearTimeout(searchFlyTimer);
      searchFlyTimer = undefined;
    }
    if (!hasActiveSearch() || rankedSearchMatches.length === 0) return;
    searchFlyTimer = window.setTimeout(() => {
      searchFlyTimer = undefined;
      flyToSearchMatches();
    }, 220);
  }

  function flyToSearchMatches(): void {
    if (!graph || rankedSearchMatches.length === 0) return;

    const targets = rankedSearchMatches.filter(hasGraphPosition).slice(0, 8);
    if (targets.length === 0) return;

    const center = targets.reduce(
      (acc, node) => acc.add(new Vector3(node.x, node.y, node.z)),
      new Vector3()
    ).divideScalar(targets.length);
    const radius = Math.max(
      1,
      ...targets.map((node) => center.distanceTo(new Vector3(node.x, node.y, node.z)))
    );
    const controls = graph.controls() as { target: Vector3 };
    const direction = graph.camera().position.clone().sub(controls.target);
    if (direction.lengthSq() < 0.001) direction.set(0, 0, 1);

    const distance = Math.max(180, Math.min(900, 220 + radius * 3.2));
    const nextPosition = center.clone().add(direction.normalize().multiplyScalar(distance));
    graph.cameraPosition(
      { x: nextPosition.x, y: nextPosition.y, z: nextPosition.z },
      { x: center.x, y: center.y, z: center.z },
      1100
    );
  }

  function hasGraphPosition(n: Node3D): n is PositionedNode3D {
    return Number.isFinite(n.x) && Number.isFinite(n.y) && Number.isFinite(n.z);
  }

  function nodeIsInFrontOfCamera(n: PositionedNode3D): boolean {
    if (!graph) return false;
    const camera = graph.camera();
    camera.getWorldDirection(labelCameraDirection);
    labelNodeVector.set(n.x, n.y, n.z).sub(camera.position);
    return labelCameraDirection.dot(labelNodeVector) > 0;
  }

  function estimateLabelWidth(text: string): number {
    return Math.min(MAX_LABEL_WIDTH, Math.max(56, text.length * 7 + 18));
  }

  function labelRect(label: LabelCandidate): LabelRect {
    const halfWidth = label.width / 2;
    return {
      left: label.x - halfWidth - LABEL_COLLISION_PADDING,
      right: label.x + halfWidth + LABEL_COLLISION_PADDING,
      top: label.y - LABEL_HEIGHT - LABEL_COLLISION_PADDING - 8,
      bottom: label.y - 8 + LABEL_COLLISION_PADDING,
    };
  }

  function rectsOverlap(a: LabelRect, b: LabelRect): boolean {
    return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
  }

  function currentLabelBudget(): number {
    if (!graph) return 0;
    const controls = graph.controls() as { target: Vector3 };
    const distance = graph.camera().position.distanceTo(controls.target);
    if (mode === "local") return 120;
    if (distance < 520) return 96;
    if (distance < 900) return 72;
    return 48;
  }

  function updateScreenLabels(timestamp = performance.now(), force = false): void {
    if (!force && timestamp - lastLabelUpdateMs < LABEL_UPDATE_INTERVAL_MS) return;
    lastLabelUpdateMs = timestamp;

    if (!graph || !wrapperEl || latestNodes.length === 0) {
      visibleLabels = [];
      visibleSearchGlows = [];
      return;
    }

    const width = wrapperEl.clientWidth;
    const height = wrapperEl.clientHeight;
    if (width <= 0 || height <= 0) {
      visibleLabels = [];
      visibleSearchGlows = [];
      return;
    }

    const centerX = width / 2;
    const centerY = height / 2;
    const maxCenterDistance = Math.hypot(centerX, centerY);
    const hoveredId = hoverNode?.id ?? "";
    const candidates: LabelCandidate[] = [];
    const glows: SearchGlow[] = [];
    const firstMatchId = rankedSearchMatches[0]?.id ?? "";

    for (const node of latestNodes) {
      if (!hasGraphPosition(node) || !nodeIsInFrontOfCamera(node)) continue;
      const screen = graph.graph2ScreenCoords(node.x, node.y, node.z);
      if (!Number.isFinite(screen.x) || !Number.isFinite(screen.y)) continue;
      if (screen.x < -MAX_LABEL_WIDTH || screen.x > width + MAX_LABEL_WIDTH) continue;
      if (screen.y < -40 || screen.y > height + 40) continue;

      const text = labelTextFor(node);
      const searchMatch = searchMatchIds.has(node.id);
      if (searchMatch) {
        glows.push({
          id: node.id,
          x: screen.x,
          y: screen.y,
          size: 26 + degreeRatio(node) * 30 + (node.id === firstMatchId ? 14 : 0),
        });
      }

      const centerDistance = Math.hypot(screen.x - centerX, screen.y - centerY);
      const centerScore = 1 - Math.min(1, centerDistance / Math.max(1, maxCenterDistance));
      const hovered = node.id === hoveredId;
      const score =
        (hovered ? 1000 : 0) +
        (searchMatch ? 650 : 0) +
        degreeRatio(node) * 110 +
        centerScore * 45 +
        Math.min(28, Math.log1p(Math.max(0, node.degree)) * 8);

      candidates.push({
        id: node.id,
        text,
        x: screen.x,
        y: screen.y,
        hovered,
        score,
        width: estimateLabelWidth(text),
      });
    }

    visibleSearchGlows = hasActiveSearch() ? glows.slice(0, 120) : [];

    if (!showSmartLabels) {
      visibleLabels = [];
      return;
    }

    candidates.sort((a, b) => b.score - a.score);
    const budget = currentLabelBudget();
    const acceptedRects: LabelRect[] = [];
    const labels: ScreenLabel[] = [];

    for (const candidate of candidates.slice(0, MAX_LABEL_CANDIDATES)) {
      if (labels.length >= budget && !candidate.hovered) break;
      const rect = labelRect(candidate);
      if (!candidate.hovered && acceptedRects.some((accepted) => rectsOverlap(rect, accepted))) {
        continue;
      }
      acceptedRects.push(rect);
      labels.push({
        id: candidate.id,
        text: candidate.text,
        x: candidate.x,
        y: candidate.y,
        hovered: candidate.hovered,
      });
      if (labels.length >= budget && labels.some((label) => label.hovered || !hoveredId)) break;
    }

    visibleLabels = labels;
  }

  function startLabelLoop(): void {
    const tick = (timestamp: number) => {
      updateScreenLabels(timestamp);
      labelFrame = window.requestAnimationFrame(tick);
    };
    labelFrame = window.requestAnimationFrame(tick);
  }

  function isDatePageTitle(title: string): boolean {
    return /^\d{4}[-_]\d{2}[-_]\d{2}$/.test(title.trim());
  }

  function filteredGraphData(data: GraphData): { nodes: Node3D[]; links: Link3D[] } {
    const hiddenNodeIds = new Set(
      hideDatePages
        ? data.nodes
            .filter((node) => isDatePageTitle(node.title))
            .map((node) => node.id)
        : []
    );
    const nodes: Node3D[] = data.nodes
      .filter((node) => !hiddenNodeIds.has(node.id))
      .map((node) => ({ id: node.id, name: node.title, degree: node.degree }));
    const visibleNodeIds = new Set(nodes.map((node) => node.id));
    const links: Link3D[] = data.edges
      .filter((edge) => visibleNodeIds.has(edge.source) && visibleNodeIds.has(edge.target))
      .map((edge) => ({ source: edge.source, target: edge.target, weight: edge.weight }));
    seedNodePositions(nodes);
    return { nodes, links };
  }

  function seedNodePositions(nodes: Node3D[]): void {
    const count = Math.max(1, nodes.length);
    const radius = 180 + Math.sqrt(count) * 18;
    nodes.forEach((node, index) => {
      const y = 1 - (2 * (index + 0.5)) / count;
      const r = Math.sqrt(Math.max(0, 1 - y * y));
      const theta = index * GOLDEN_ANGLE;
      node.x = Math.cos(theta) * r * radius;
      node.y = y * radius;
      node.z = Math.sin(theta) * r * radius;
    });
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
      const { nodes, links } = filteredGraphData(data);
      latestNodes = nodes;
      updateSearchMatches();
      maxDegree = Math.max(1, ...nodes.map((n) => n.degree));
      const clusters = computeGraphClusters(
        nodes.map((n) => n.id),
        links
      );
      clusterIndexById = clusters.clusterIndexById;
      isolatedIds = clusters.isolatedIds;
      stats = { nodes: nodes.length, edges: links.length };
      graph.nodeVal((n) => nodeValFor(n)).linkWidth((l) => linkWidthFor(l));
      graph.graphData({ nodes, links });
      configureForces();
      refreshColors();
      updateScreenLabels(performance.now(), true);
      if (hasActiveSearch() && rankedSearchMatches.length > 0) {
        scheduleFlyToSearchMatches();
      } else {
        scheduleFitToGraph();
      }
    } catch (e) {
      latestNodes = [];
      searchMatchIds = new Set();
      rankedSearchMatches = [];
      searchMatchCount = 0;
      visibleLabels = [];
      visibleSearchGlows = [];
      errorMsg = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function fitToGraph(durationMs = 650): void {
    if (!graph || stats.nodes === 0) return;
    graph.camera().up.set(0, 1, 0);
    const distance = Math.max(MIN_CAMERA_DISTANCE, 440 + Math.sqrt(stats.nodes) * 34);
    graph.cameraPosition({ x: 0, y: 0, z: distance }, { x: 0, y: 0, z: 0 }, durationMs);
    updateScreenLabels(performance.now(), true);
  }

  function scheduleFitToGraph(): void {
    pendingFit = true;
    if (fitTimer !== undefined) {
      window.clearTimeout(fitTimer);
    }
    // Fallback in case the force engine is still cooling for a long time.
    fitTimer = window.setTimeout(() => {
      if (!pendingFit) return;
      pendingFit = false;
      fitTimer = undefined;
      fitToGraph();
    }, 900);
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

  function configureForces(): void {
    if (!graph) return;
    graph.d3VelocityDecay(0.55).warmupTicks(120).cooldownTicks(300);
    (graph.d3Force("charge") as { strength?: (value: number) => unknown } | undefined)
      ?.strength?.(-520);
    const linkForce = graph.d3Force("link") as
      | { distance?: (value: number) => unknown; strength?: (value: number) => unknown }
      | undefined;
    linkForce?.distance?.(180);
    linkForce?.strength?.(0.08);
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
    if (!wrapperEl || !graphMountEl) return;
    const bgColor = themeColor("--bg-primary", "#16161e");
    isLightTheme = detectIsLightTheme();

    // `controlType: "orbit"` is what gives us the "solar system" feel:
    // dragging orbits the camera around the graph in 3D (Three.js
    // OrbitControls under the hood), scroll zooms, right-drag/two-finger
    // pans. It's a constructor-only option (not chainable), per the
    // library's types.
    graph = new ForceGraph3D(graphMountEl, {
      controlType: "orbit",
    }) as unknown as ForceGraph3DInstance<Node3D, Link3D>;
    graph
      .backgroundColor(bgColor)
      .nodeId("id")
      .nodeLabel((n) => n.name)
      .nodeRelSize(2)
      .nodeVal((n) => nodeValFor(n))
      .nodeColor((n) => nodeColorFor(n))
      .nodeOpacity(0.98)
      .nodeResolution(14)
      .linkSource("source")
      .linkTarget("target")
      .linkColor((l) => linkColorFor(l))
      .linkOpacity(0.25)
      .linkWidth((l) => linkWidthFor(l))
      .linkDirectionalParticles(0)
      .showNavInfo(false)
      .onNodeClick((n) => onNavigate(n.name))
      .onNodeHover((n) => {
        hoverNode = n ?? null;
        if (wrapperEl) wrapperEl.style.cursor = n ? "pointer" : "grab";
        updateScreenLabels(performance.now(), true);
      })
      .onEngineStop(() => {
        if (!pendingFit) return;
        pendingFit = false;
        if (fitTimer !== undefined) {
          window.clearTimeout(fitTimer);
          fitTimer = undefined;
        }
        fitToGraph();
      });
    configureForces();
    graph.width(wrapperEl.clientWidth).height(wrapperEl.clientHeight);

    void loadData();
    startLabelLoop();

    resizeObserver = new ResizeObserver(() => {
      if (wrapperEl && graph) {
        graph.width(wrapperEl.clientWidth).height(wrapperEl.clientHeight);
        updateScreenLabels(performance.now(), true);
        if (pendingFit) fitToGraph(0);
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
    if (fitTimer !== undefined) {
      window.clearTimeout(fitTimer);
      fitTimer = undefined;
    }
    if (labelFrame !== undefined) {
      window.cancelAnimationFrame(labelFrame);
      labelFrame = undefined;
    }
    if (searchFlyTimer !== undefined) {
      window.clearTimeout(searchFlyTimer);
      searchFlyTimer = undefined;
    }
    graph?._destructor();
    graph = null;
  });

  function resetView() {
    fitToGraph(600);
  }

  $effect(() => {
    // Re-run whenever mode/nodeLimit/currentPageId changes.
    mode;
    nodeLimit;
    currentPageId;
    hideDatePages;
    void loadData();
  });

  $effect(() => {
    searchText;
    updateSearchMatches();
    graph?.nodeVal((n) => nodeValFor(n));
    refreshColors();
    updateScreenLabels(performance.now(), true);
    scheduleFlyToSearchMatches();
  });

  $effect(() => {
    showSmartLabels;
    updateScreenLabels(performance.now(), true);
  });
</script>

<div class="graph-view-3d">
  <div class="graph-canvas-wrap" bind:this={wrapperEl}>
    <div class="graph-canvas" bind:this={graphMountEl}></div>

    {#if loading}
      <div class="graph-overlay">Building graph…</div>
    {:else if errorMsg}
      <div class="graph-overlay error">{errorMsg}</div>
    {:else if stats.nodes === 0}
      <div class="graph-overlay">
        No links to display{mode === "local" ? " for this page" : ""}.
      </div>
    {:else if searchText.trim() && searchMatchCount === 0}
      <div class="graph-overlay">
        No graph nodes match “{searchText.trim()}”.
      </div>
    {/if}

    {#if hoverNode}
      <div class="hover-card">
        <strong>{labelTextFor(hoverNode)}</strong>
        <span>{hoverNode.degree.toLocaleString()} links</span>
      </div>
    {/if}

    {#if visibleSearchGlows.length > 0}
      <div class="graph-search-glow-layer" aria-hidden="true">
        {#each visibleSearchGlows as glow (glow.id)}
          <span
            class="graph-search-glow"
            style={`left: ${glow.x}px; top: ${glow.y}px; width: ${glow.size}px; height: ${glow.size}px;`}
          ></span>
        {/each}
      </div>
    {/if}

    {#if visibleLabels.length > 0}
      <div class="graph-label-layer" aria-hidden="true">
        {#each visibleLabels as label (label.id)}
          <span
            class="graph-label"
            class:hovered={label.hovered}
            style={`left: ${label.x}px; top: ${label.y}px;`}
          >{label.text}</span>
        {/each}
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
      <span>Search</span>
      <input type="text" placeholder="Fly to nodes…" bind:value={searchText} />
    </label>

    {#if searchText.trim()}
      <div class="focus-label">
        {searchMatchCount.toLocaleString()} matching node{searchMatchCount === 1 ? "" : "s"}
      </div>
    {/if}

    <label class="ctrl">
      <span>Max nodes: {nodeLimit}</span>
      <input type="range" min="20" max="400" step="20" bind:value={nodeLimit} />
    </label>

    <label class="ctrl checkbox">
      <input type="checkbox" bind:checked={hideDatePages} />
      <span>Hide date pages</span>
    </label>

    <label class="ctrl checkbox">
      <input type="checkbox" bind:checked={showSmartLabels} />
      <span>Show labels</span>
    </label>

    <div class="graph-stats">
      {stats.nodes.toLocaleString()} nodes · {stats.edges.toLocaleString()} links
    </div>
    <p class="hint">
      Click a node to open it. Labels stay in screen space; hover any node for the exact title.
      Drag to orbit, scroll to zoom, right-drag to pan, Ctrl+drag to roll.
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
    z-index: 0;
  }

  .graph-canvas {
    position: absolute;
    inset: 0;
    z-index: 0;
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

  .hover-card {
    position: absolute;
    left: 16px;
    bottom: 16px;
    z-index: 4;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: min(420px, calc(100% - 32px));
    padding: 8px 10px;
    border: 1px solid var(--border-color, #333);
    border-radius: 8px;
    background: color-mix(in srgb, var(--bg-secondary, #1e1e2e) 92%, transparent);
    color: var(--text-primary, #eee);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    pointer-events: none;
  }

  .hover-card strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }

  .hover-card span {
    color: var(--text-muted, #888);
    font-size: 11px;
  }

  .graph-search-glow-layer {
    position: absolute;
    inset: 0;
    z-index: 1;
    overflow: hidden;
    pointer-events: none;
  }

  .graph-search-glow {
    position: absolute;
    transform: translate(-50%, -50%);
    border: 1px solid color-mix(in srgb, var(--accent-green, #9ece6a) 88%, white);
    border-radius: 999px;
    background: radial-gradient(
      circle,
      color-mix(in srgb, var(--accent-green, #9ece6a) 62%, transparent) 0%,
      color-mix(in srgb, var(--accent-green, #9ece6a) 28%, transparent) 38%,
      transparent 72%
    );
    box-shadow:
      0 0 16px color-mix(in srgb, var(--accent-green, #9ece6a) 70%, transparent),
      0 0 34px color-mix(in srgb, var(--accent-green, #9ece6a) 45%, transparent);
  }

  .graph-label-layer {
    position: absolute;
    inset: 0;
    z-index: 2;
    overflow: hidden;
    pointer-events: none;
  }

  .graph-label {
    position: absolute;
    max-width: 220px;
    transform: translate(-50%, calc(-100% - 8px));
    padding: 2px 7px;
    border: 1px solid color-mix(in srgb, var(--border-color, #333) 70%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--bg-primary, #050508) 72%, transparent);
    color: var(--text-primary, #eee);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.32);
    font-size: 12px;
    font-weight: 600;
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.95);
    white-space: nowrap;
  }

  .graph-label.hovered {
    z-index: 2;
    border-color: var(--accent, #6ea8fe);
    background: color-mix(in srgb, var(--accent, #6ea8fe) 28%, var(--bg-primary, #050508));
  }

  .zoom-controls {
    position: absolute;
    bottom: 16px;
    right: 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    z-index: 3;
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
    position: relative;
    z-index: 3;
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

  .ctrl input[type="text"] {
    width: 100%;
    box-sizing: border-box;
    padding: 6px 8px;
    border: 1px solid var(--border-color, #333);
    border-radius: 4px;
    background: var(--bg-primary, #16161e);
    color: var(--text-primary, #eee);
    font-size: 12px;
    outline: none;
  }

  .ctrl input[type="text"]:focus {
    border-color: var(--accent, #6ea8fe);
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
