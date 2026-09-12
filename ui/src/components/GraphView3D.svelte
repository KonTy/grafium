<script lang="ts">
  import { onMount, onDestroy, untrack } from "svelte";
  import ForceGraph3D, { type ForceGraph3DInstance } from "3d-force-graph";
  import { BufferGeometry, Float32BufferAttribute, Mesh, MeshBasicMaterial, Points, PointsMaterial, RingGeometry, DoubleSide, Vector3 } from "three";
  import type { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
  import { getGraphData, type GraphData } from "../lib/api";
  import { fuzzyScore } from "../lib/fuzzy";
  import { clusterColor, computeGraphClusters, planetColor } from "../lib/graphClusters";
  import { buildFlightNeighbors, createFlightLeg, nextFlightTopic, sampleFlightLeg, sampleFlightOrbit, type FlightLeg } from "../lib/graphFlight";
  import { buildPlanetHierarchy, layoutPlanetSystems, planetHasRings, type PlanetLayout } from "../lib/planetSystems";

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
    fx?: number;
    fy?: number;
    fz?: number;
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
    destination: boolean;
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
  let loadVersion = 0;
  let flying = $state(false);
  let flightAvailable = $state(false);
  let flightTopic = $state<Node3D | null>(null);
  let flightFromTitle = $state("");
  let flightPhase = $state<"travel" | "orbit">("travel");
  let flightStops = $state(0);
  let flightNeighbors = new Map<string, string[]>();
  let flightVisits = new Map<string, number>();
  let flightPrevious: string | null = null;
  let flightLeg: FlightLeg | null = null;
  let flightElapsed = 0;
  let flightLastFrame: number | null = null;
  let starfield: Points<BufferGeometry, PointsMaterial> | null = null;
  let planetRing: Mesh<RingGeometry, MeshBasicMaterial> | null = null;
  let savedCooldownTicks = 300;
  let savedWarmupTicks = 120;
  let restoreSimulationPending = false;
  let savedDamping = true;
  let normalGraphData: { nodes: Node3D[]; links: Link3D[] } | null = null;
  let flightLayout = $state.raw<PlanetLayout | null>(null);
  let flightLinks: Link3D[] = [];
  let flightFamilyLabel = $state("");
  let flightRinged = $state(false);
  const FLIGHT_ORBIT_MS = 2600;

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
    if (flying) return planetColor(n.name);
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

  function baseNodeValFor(n: Node3D): number {
    return MIN_NODE_VAL + degreeRatio(n) * (MAX_NODE_VAL - MIN_NODE_VAL);
  }

  function nodeValFor(n: Node3D): number {
    const radius = flying ? flightLayout?.radii.get(n.id) : undefined;
    if (radius !== undefined) return (radius / 5) ** 3;
    const base = baseNodeValFor(n);
    return !flying && searchMatchIds.has(n.id) ? base * 2.2 + 24 : base;
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
    if (flying || !hasActiveSearch() || rankedSearchMatches.length === 0) return;
    searchFlyTimer = window.setTimeout(() => {
      searchFlyTimer = undefined;
      flyToSearchMatches();
    }, 220);
  }

  function flyToSearchMatches(): void {
    if (!graph || flying || rankedSearchMatches.length === 0) return;

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
    if (flying) return 14;
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

      const destination = flying && node.id === flightTopic?.id;
      const satellite = flying && flightLayout?.parentById.has(node.id);
      const text = destination ? node.name : satellite ? node.name.split("/").at(-1)! : labelTextFor(node);
      const searchMatch = !flying && searchMatchIds.has(node.id);
      if (destination) {
        const abovePlanet = new Vector3(node.x, node.y, node.z)
          .addScaledVector(graph.camera().up, planetRadius(node) * 1.3);
        screen.y = graph.graph2ScreenCoords(abovePlanet.x, abovePlanet.y, abovePlanet.z).y;
      }
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
        (destination ? 3000 : 0) +
        (flying && flightLayout?.parentById.get(node.id) === flightTopic?.id ? 800 : 0) +
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
        destination,
        score,
        width: estimateLabelWidth(text),
      });
    }

    visibleSearchGlows = hasActiveSearch() ? glows.slice(0, 120) : [];

    if (!showSmartLabels && !flying) {
      visibleLabels = [];
      return;
    }

    candidates.sort((a, b) => b.score - a.score);
    const budget = currentLabelBudget();
    const acceptedRects: LabelRect[] = [];
    const labels: ScreenLabel[] = [];

    for (const candidate of candidates.slice(0, MAX_LABEL_CANDIDATES)) {
      if (!showSmartLabels && !candidate.destination) continue;
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
        destination: candidate.destination,
      });
      if (labels.length >= budget && labels.some((label) => label.hovered || !hoveredId)) break;
    }

    visibleLabels = labels;
  }

  function startLabelLoop(): void {
    const tick = (timestamp: number) => {
      updateFlight(timestamp);
      updateScreenLabels(timestamp);
      labelFrame = window.requestAnimationFrame(tick);
    };
    labelFrame = window.requestAnimationFrame(tick);
  }

  function planetRadius(node: Node3D): number {
    return Math.cbrt(nodeValFor(node)) * 5;
  }

  function clearCameraTimers(): void {
    pendingFit = false;
    if (fitTimer !== undefined) window.clearTimeout(fitTimer);
    if (searchFlyTimer !== undefined) window.clearTimeout(searchFlyTimer);
    fitTimer = undefined;
    searchFlyTimer = undefined;
  }

  function addFlightStars(): void {
    if (!graph) return;
    const radius = Math.max(1800, ...latestNodes.filter(hasGraphPosition)
      .map((node) => Math.hypot(node.x, node.y, node.z) * 2 + 600));
    const points: number[] = [];
    for (let i = 0; i < 1800; i++) {
      const direction = new Vector3().randomDirection().multiplyScalar(radius * (1 + Math.random()));
      points.push(direction.x, direction.y, direction.z);
    }
    const geometry = new BufferGeometry();
    geometry.setAttribute("position", new Float32BufferAttribute(points, 3));
    starfield = new Points(geometry, new PointsMaterial({
      color: "#cbd5ff", size: 5, transparent: true, opacity: 0.85, depthWrite: false,
    }));
    graph.scene().add(starfield);
  }

  function clearPlanetRing(): void {
    if (!planetRing) return;
    planetRing.removeFromParent();
    planetRing.geometry.dispose();
    planetRing.material.dispose();
    planetRing = null;
  }

  function restoreSimulationSettings(): void {
    if (!restoreSimulationPending || !graph) return;
    restoreSimulationPending = false;
    graph.warmupTicks(savedWarmupTicks).cooldownTicks(savedCooldownTicks);
  }

  function beginFlightTo(node: PositionedNode3D): void {
    if (!graph) return;
    const controls = graph.controls() as OrbitControls;
    flightFromTitle = flightTopic?.name ?? "";
    flightPrevious = flightTopic?.id ?? null;
    flightTopic = node;
    flightVisits.set(node.id, (flightVisits.get(node.id) ?? 0) + 1);
    flightStops++;
    flightPhase = "travel";
    flightElapsed = 0;
    const parentId = flightLayout?.parentById.get(node.id);
    const parent = latestNodes.find((candidate) => candidate.id === parentId);
    const childCount = flightLayout?.childrenById.get(node.id)?.length ?? 0;
    flightFamilyLabel = parent ? `Satellite of ${parent.name}`
      : childCount ? `${childCount} child topic${childCount === 1 ? "" : "s"} in this planet system`
      : "Topic planet";
    const position = new Vector3(node.x, node.y, node.z);
    const extent = flightLayout?.extents.get(node.id) ?? planetRadius(node);
    flightLeg = createFlightLeg(
      graph.camera().position, controls.target,
      position, planetRadius(node),
      {
        distance: Math.max(40 + planetRadius(node) * 5, extent * 2.6),
        approach: parent && hasGraphPosition(parent)
          ? position.clone().sub(new Vector3(parent.x, parent.y, parent.z))
          : undefined,
      },
    );
    clearPlanetRing();
    flightRinged = planetHasRings(node.name, !!parentId);
    if (flightRinged) {
      planetRing = new Mesh(
        new RingGeometry(planetRadius(node) * 1.35, planetRadius(node) * 1.65, 64),
        new MeshBasicMaterial({ color: planetColor(node.name), side: DoubleSide, transparent: true, opacity: 0.45, depthWrite: false }),
      );
      planetRing.position.copy(position);
      planetRing.lookAt(graph.camera().position);
      planetRing.rotateX(1.1);
      planetRing.rotateZ(0.3);
      graph.scene().add(planetRing);
    }
    refreshColors();
  }

  function toggleFlight(): void {
    if (flying) {
      stopFlight();
      return;
    }
    if (!graph || loading) return;
    const connected = latestNodes.filter((node): node is PositionedNode3D =>
      hasGraphPosition(node) && (flightNeighbors.get(node.id)?.length ?? 0) > 0);
    const matchedStart = rankedSearchMatches.find((match) => connected.some((node) => node.id === match.id));
    const first = hasActiveSearch()
      ? connected.find((node) => node.id === matchedStart?.id)
      : connected.find((node) => node.id === currentPageId)
        ?? connected.sort((a, b) => b.degree - a.degree)[0];
    if (!first) {
      errorMsg = hasActiveSearch()
        ? "No matching topic has a visible link. Clear the search or choose another starting topic."
        : "Space flight needs at least two linked topics in the visible graph.";
      return;
    }
    errorMsg = null;
    clearCameraTimers();
    restoreSimulationSettings();
    const controls = graph.controls() as OrbitControls;
    // Finish any library-owned search/reset tween at the current view, then
    // let our frame loop own both camera position and its orbit-control target.
    const position = graph.camera().position.clone();
    const target = controls.target.clone();
    graph.cameraPosition(position, target, 0);
    savedDamping = controls.enableDamping;
    controls.enableDamping = false;
    controls.update();
    controls.enabled = false;
    savedCooldownTicks = graph.cooldownTicks();
    savedWarmupTicks = graph.warmupTicks();
    normalGraphData = { nodes: latestNodes, links: graph.graphData().links };
    flightLayout = layoutPlanetSystems(latestNodes.filter(hasGraphPosition).map((node) => ({
      id: node.id, name: node.name, x: node.x, y: node.y, z: node.z, radius: Math.cbrt(baseNodeValFor(node)) * 5,
    })));
    const flightNodes = latestNodes.filter(hasGraphPosition).map((node) => {
      const position = flightLayout!.positions.get(node.id)!;
      return {
        ...node, x: position.x, y: position.y, z: position.z,
        fx: position.x, fy: position.y, fz: position.z,
      };
    });
    latestNodes = flightNodes;
    graph.warmupTicks(0).cooldownTicks(0).enableNodeDrag(false).enablePointerInteraction(false);
    flying = true;
    hoverNode = null;
    flightStops = 0;
    flightVisits = new Map();
    flightLastFrame = null;
    graph.camera().up.set(0, 1, 0);
    graph.backgroundColor("#050711").nodeRelSize(5).nodeResolution(32)
      .nodeVal((node) => nodeValFor(node)).linkOpacity(0.15)
      .graphData({ nodes: flightNodes, links: flightLinks.map((link) => ({ ...link })) });
    addFlightStars();
    beginFlightTo(flightNodes.find((node) => node.id === first.id)!);
  }

  function updateFlight(timestamp: number): void {
    if (!flying || !graph || !flightLeg) return;
    // Do not skip whole destinations after a suspended/background frame.
    flightElapsed += flightLastFrame === null ? 0 : Math.min(100, timestamp - flightLastFrame);
    flightLastFrame = timestamp;
    const duration = flightPhase === "travel" ? flightLeg.durationMs : FLIGHT_ORBIT_MS;
    const fraction = Math.min(1, flightElapsed / duration);
    const pose = flightPhase === "travel"
      ? sampleFlightLeg(flightLeg, fraction)
      : sampleFlightOrbit(flightLeg, fraction);
    (graph.controls() as OrbitControls).target.copy(pose.lookAt);
    graph.cameraPosition(pose.position, pose.lookAt, 0);
    if (fraction < 1) return;
    flightElapsed = 0;
    if (flightPhase === "travel") {
      flightPhase = "orbit";
      return;
    }
    const nextId = nextFlightTopic(flightNeighbors, flightTopic!.id, flightPrevious, flightVisits);
    const next = latestNodes.find((node) => node.id === nextId);
    if (!next || !hasGraphPosition(next)) {
      stopFlight();
      errorMsg = "No positioned, related topic is available to continue this flight.";
      return;
    }
    beginFlightTo(next);
  }

  function stopFlight(restoreGraph = true): void {
    if (!flying) return;
    if (restoreGraph && graph && normalGraphData) {
      const originalTopic = normalGraphData.nodes.find((node) => node.id === flightTopic?.id);
      if (originalTopic && flightTopic && hasGraphPosition(originalTopic) && hasGraphPosition(flightTopic)) {
        // Keep the camera beside the same topic while restoring the normal layout.
        const offset = new Vector3(originalTopic.x - flightTopic.x, originalTopic.y - flightTopic.y, originalTopic.z - flightTopic.z);
        graph.camera().position.add(offset);
        (graph.controls() as OrbitControls).target.add(offset);
      }
      latestNodes = normalGraphData.nodes;
      graph.warmupTicks(0).cooldownTicks(0).graphData(normalGraphData);
      restoreSimulationPending = true;
    }
    normalGraphData = null;
    flying = false;
    flightLayout = null;
    flightLeg = null;
    flightTopic = null;
    flightLastFrame = null;
    clearPlanetRing();
    if (starfield) {
      starfield.removeFromParent();
      starfield.geometry.dispose();
      starfield.material.dispose();
      starfield = null;
    }
    if (restoreGraph && graph) {
      const controls = graph.controls() as OrbitControls;
      controls.enabled = true;
      controls.update();
      controls.enableDamping = savedDamping;
      graph.enableNodeDrag(true).enablePointerInteraction(true)
        .backgroundColor(themeColor("--bg-primary", "#16161e"))
        .nodeRelSize(2).nodeResolution(14).nodeVal((node) => nodeValFor(node)).linkOpacity(0.25);
      refreshColors();
      updateScreenLabels(performance.now(), true);
    }
  }

  function onFlightKeydown(event: KeyboardEvent): void {
    if (flying && event.key === "Escape") {
      event.preventDefault();
      stopFlight();
    }
  }

  function onVisibilityChange(): void {
    if (document.hidden) stopFlight();
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
    const sourceId = linkEndpointId(l.source);
    const targetId = linkEndpointId(l.target);
    if (flying && (
      (sourceId === flightPrevious && targetId === flightTopic?.id) ||
      (targetId === flightPrevious && sourceId === flightTopic?.id)
    )) return "#e0f2fe";
    if (isolatedIds.has(sourceId)) return themeColor("--text-secondary", isLightTheme ? "#666" : "#aaa");
    return clusterColor(clusterIndexById.get(sourceId) ?? 0, flying ? false : isLightTheme);
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
    const version = ++loadVersion;
    stopFlight();
    clearCameraTimers();
    loading = true;
    errorMsg = null;
    try {
      const focus = mode === "local" ? currentPageId || undefined : undefined;
      const data: GraphData = await getGraphData(nodeLimit, focus);
      if (version !== loadVersion || !graph) return;
      const { nodes, links } = filteredGraphData(data);
      latestNodes = nodes;
      const hierarchy = buildPlanetHierarchy(nodes);
      flightLinks = links.map((link) => ({ ...link }));
      for (const [child, parent] of hierarchy.parentById) {
        if (!flightLinks.some((link) =>
          (link.source === child && link.target === parent) || (link.source === parent && link.target === child)
        )) flightLinks.push({ source: parent, target: child, weight: 1 });
      }
      flightNeighbors = buildFlightNeighbors(nodes.map((node) => node.id), flightLinks);
      flightAvailable = [...flightNeighbors.values()].some((related) => related.length > 0);
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
      restoreSimulationPending = false;
      configureForces();
      refreshColors();
      updateScreenLabels(performance.now(), true);
      if (hasActiveSearch() && rankedSearchMatches.length > 0) {
        scheduleFlyToSearchMatches();
      } else {
        scheduleFitToGraph();
      }
    } catch (e) {
      if (version !== loadVersion || !graph) return;
      flightAvailable = false;
      latestNodes = [];
      searchMatchIds = new Set();
      rankedSearchMatches = [];
      searchMatchCount = 0;
      visibleLabels = [];
      visibleSearchGlows = [];
      errorMsg = e instanceof Error ? e.message : String(e);
    } finally {
      if (version === loadVersion) loading = false;
    }
  }

  function fitToGraph(durationMs = 650): void {
    if (!graph || flying || stats.nodes === 0) return;
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
    if (flying || !event.ctrlKey || event.button !== 0 || !graph || !wrapperEl) return;
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
      .onNodeClick((n) => { if (!flying) onNavigate(n.name); })
      .onNodeHover((n) => {
        hoverNode = n ?? null;
        if (wrapperEl) wrapperEl.style.cursor = n ? "pointer" : "grab";
        updateScreenLabels(performance.now(), true);
      })
      .onEngineStop(() => {
        if (!flying) restoreSimulationSettings();
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
      if (!flying) graph?.backgroundColor(themeColor("--bg-primary", "#16161e"));
      refreshColors();
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });

    wrapperEl.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", endRoll);
    window.addEventListener("keydown", onFlightKeydown);
    document.addEventListener("visibilitychange", onVisibilityChange);
  });

  onDestroy(() => {
    loadVersion++;
    stopFlight(false);
    resizeObserver?.disconnect();
    themeObserver?.disconnect();
    wrapperEl?.removeEventListener("pointerdown", handlePointerDown);
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", endRoll);
    window.removeEventListener("keydown", onFlightKeydown);
    document.removeEventListener("visibilitychange", onVisibilityChange);
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
    stopFlight();
    clearCameraTimers();
    fitToGraph(600);
  }

  $effect(() => {
    // Re-run whenever mode/nodeLimit/currentPageId changes.
    mode;
    nodeLimit;
    currentPageId;
    hideDatePages;
    untrack(() => void loadData());
  });

  $effect(() => {
    searchText;
    untrack(() => {
      updateSearchMatches();
      graph?.nodeVal((n) => nodeValFor(n));
      refreshColors();
      updateScreenLabels(performance.now(), true);
      scheduleFlyToSearchMatches();
    });
  });

  $effect(() => {
    showSmartLabels;
    untrack(() => updateScreenLabels(performance.now(), true));
  });
</script>

<div class="graph-view-3d" class:flying data-satellites={flying ? flightLayout?.parentById.size ?? 0 : 0}>
  <div class="graph-canvas-wrap" bind:this={wrapperEl}>
    <div class="graph-canvas" bind:this={graphMountEl}></div>
    {#if flying && flightTopic}
      <div class="flight-hud" role="status" data-stop={flightStops} data-topic={flightTopic.id}
        data-ringed={flightRinged} data-satellite={flightLayout?.parentById.has(flightTopic.id) ?? false}
        style={`--planet-color: ${planetColor(flightTopic.name)};`}>
        <span class="flight-eyebrow">SPACE FLIGHT · {flightPhase === "travel" ? "Approaching" : "Orbiting"}</span>
        <strong>{flightTopic.name}</strong>
        <span>{flightFamilyLabel}</span>
        <span>{flightFromTitle ? `From ${flightFromTitle}` : "Beginning your journey"}</span>
      </div>
    {/if}

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
            class:destination={label.destination}
            style={`left: ${label.x}px; top: ${label.y}px; --planet-color: ${planetColor(label.text)};`}
          >{label.text}</span>
        {/each}
      </div>
    {/if}

    <div class="zoom-controls">
      <button title="Reset view" aria-label="Reset view" onclick={resetView}>⤢</button>
    </div>
  </div>

  <aside class="graph-controls">
    <h2>Graph (3D)</h2>
    <button
      class="flight-toggle"
      class:active={flying}
      aria-pressed={flying}
      disabled={!flying && (loading || !flightAvailable)}
      onclick={toggleFlight}
    >{flying ? "Stop flight" : "Space flight"}</button>
    <p class="hint">
      {flying
        ? "Autopilot follows links and child topics. Press Stop flight or Escape to take control."
        : "Fly between topic-planets along their links. Search first to choose a starting topic."}
      {#if !loading && !flightAvailable}At least two linked, visible topics are needed.{/if}
    </p>

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
      <input type="text" data-local-search placeholder="Fly to nodes…" bind:value={searchText} disabled={flying} />
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
    <p class="hint">{flying
      ? "Child pages are smaller, ringless satellites of their nearest visible parent. Some main planets have rings. Namespace families share colors."
      : "Linked clusters share a color; unlinked pages are shown in a neutral gray."}</p>
  </aside>
</div>

<style>
  .flight-toggle {
    padding: 10px 12px;
    border: 1px solid var(--accent, #6ea8fe);
    border-radius: 6px;
    color: var(--text-primary);
    background: color-mix(in srgb, var(--accent, #6ea8fe) 15%, var(--bg-primary));
    font: inherit;
    cursor: pointer;
  }

  .flight-toggle.active {
    color: #e0f2fe;
    background: #15344e;
    border-color: #7dd3fc;
  }

  .flight-toggle:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .flight-toggle:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
  }

  .flight-hud {
    position: absolute;
    top: 64px;
    left: 24px;
    max-width: min(460px, calc(100% - 48px));
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px 18px;
    border-left: 2px solid var(--planet-color, #7dd3fc);
    border-radius: 0 8px 8px 0;
    color: #cbd5e1;
    background: #050711cc;
    pointer-events: none;
    z-index: 4;
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .flight-hud strong {
    font-size: 22px;
    font-weight: 500;
    color: #f0f9ff;
  }

  .flight-eyebrow {
    color: var(--planet-color, #7dd3fc);
    font-size: 10px;
    letter-spacing: 0.14em;
  }

  .flying .graph-label {
    background: #050711cc;
    color: #cbd5e1;
    border-color: #334155;
  }

  .graph-label.destination {
    max-width: 320px;
    border-color: var(--planet-color, #7dd3fc);
    color: #f0f9ff;
    font-size: 15px;
    white-space: normal;
    text-align: center;
    overflow-wrap: anywhere;
    border-radius: 6px;
  }

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
