<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { discoverLinkCandidates, getGraphData, type GraphData } from "../lib/api";
  import { fuzzyScore } from "../lib/fuzzy";
  import { exceedsDragThreshold } from "../lib/graphColor";
  import { clusterColor } from "../lib/graphClusters";
  import {
    createCommunityLayout,
    communityLinkDistance,
    type CommunityLayout,
  } from "../lib/graphCommunityLayout";

  interface Props {
    onNavigate: (title: string, highlight?: string) => void;
    currentPageId?: string;
    currentPageTitle?: string;
  }

  let { onNavigate, currentPageId = "", currentPageTitle = "" }: Props = $props();

  // ---- Simulation node type ----
  interface SimNode {
    id: string;
    title: string;
    degree: number;
    x: number;
    y: number;
    vx: number;
    vy: number;
    anchorX: number;
    anchorY: number;
  }
  interface SimEdge {
    source: SimNode;
    target: SimNode;
    weight: number;
    suggested: boolean;
    confidence: number;
    distance: number;
    internal: boolean;
  }

  // ---- View / control state (Logseq-style) ----
  // Always default to the whole graph. Local mode is opt-in via the toggle so
  // returning to the graph (with a page open) doesn't silently shrink it.
  let mode = $state<"global" | "local">("global");
  let nodeLimit = $state(200);
  let chargeStrength = $state(60); // node repulsion
  let linkDistance = $state(70);
  let nodeScale = $state(1); // multiplier for node radius
  let showLabels = $state(true);
  let showSuggestedEdges = $state(false);
  let searchText = $state("");
  let animate = $state(false); // live jiggle; off = settle instantly, stay still

  let loading = $state(false);
  let errorMsg = $state<string | null>(null);
  let stats = $state({ nodes: 0, edges: 0, suggested: 0, communities: 0 });
  let searchMatchCount = $state(0);
  let scanningSuggestions = $state(false);
  let mobileControlsOpen = $state(false);
  let suggestionError = $state<string | null>(null);

  // ---- Canvas / camera ----
  let canvasEl: HTMLCanvasElement | null = $state(null);
  let wrapperEl: HTMLDivElement | null = $state(null);
  let ctx: CanvasRenderingContext2D | null = null;
  let dpr = 1;
  let width = 0;
  let height = 0;

  let scale = 1;
  let offsetX = 0;
  let offsetY = 0;
  let autoFit = true;

  // ---- Simulation data (non-reactive, mutated in the rAF loop) ----
  let communityLayout: CommunityLayout | null = null;
  let nodes: SimNode[] = [];
  let edges: SimEdge[] = [];
  let nodeById = new Map<string, SimNode>();
  let maxDegree = 1;
  let maxWeight = 1;
  let alpha = 0;
  let raf = 0;
  let running = false;
  let loadGeneration = 0;
  let destroyed = false;

  // Interaction
  let dragNode: SimNode | null = null;
  let hoverNode: SimNode | null = null;
  let panning = false;
  let pointerMoved = false;
  /// Where the pointer went down, so a press can be classified as click or
  /// drag by distance rather than by "did any move event arrive".
  let pointerDownX = 0;
  let pointerDownY = 0;
  let lastX = 0;
  let lastY = 0;

  const MIN_ALPHA = 0.008;

  async function loadData() {
    const generation = ++loadGeneration;
    loading = true;
    errorMsg = null;
    try {
      const focus = mode === "local" ? currentPageId || undefined : undefined;
      const data: GraphData = await getGraphData(nodeLimit, focus, showSuggestedEdges);
      if (destroyed || generation !== loadGeneration) return;
      buildSimulation(data);
      updateSearchMatchCount();
      stats = {
        nodes: data.nodes.length,
        edges: edges.length,
        suggested: edges.filter((edge) => edge.suggested).length,
        communities: communityLayout?.groups.length ?? 0,
      };
    } catch (e) {
      if (destroyed || generation !== loadGeneration) return;
      errorMsg = String(e);
      console.error("Failed to load graph data:", e);
    } finally {
      if (!destroyed && generation === loadGeneration) loading = false;
    }
  }

  function buildSimulation(data: GraphData) {
    const layout = createCommunityLayout(data.nodes, data.edges, 2);
    communityLayout = layout;
    nodeById = new Map();
    dragNode = null;
    hoverNode = null;
    nodes = data.nodes.map((n) => {
      const position = layout.positions.get(n.id);
      if (!position) throw new Error(`Missing graph layout position for ${n.id}`);
      const node: SimNode = {
        id: n.id,
        title: n.title,
        degree: n.degree,
        x: position.x * linkDistance / 70,
        y: position.y * linkDistance / 70,
        vx: 0,
        vy: 0,
        anchorX: position.x,
        anchorY: position.y,
      };
      nodeById.set(n.id, node);
      return node;
    });
    edges = [];
    for (const e of data.edges) {
      const s = nodeById.get(e.source);
      const t = nodeById.get(e.target);
      if (s && t) {
        edges.push({
          source: s,
          target: t,
          weight: e.weight ?? 1,
          suggested: e.suggested ?? false,
          confidence: e.confidence ?? 1,
          distance: communityLinkDistance(layout, e.source, e.target),
          internal: layout.clusterIndexById.has(e.source) &&
            layout.clusterIndexById.get(e.source) === layout.clusterIndexById.get(e.target),
        });
      }
    }
    maxDegree = Math.max(1, ...data.nodes.map((n) => n.degree));
    maxWeight = Math.max(1, ...edges.map((e) => e.weight));
    autoFit = true;
    fitView();
    draw();
    settleThenShow();
  }

  // Lay the graph out. When animation is off we run the physics off-screen and
  // paint the final, settled positions once so nothing flies around on screen.
  function settleThenShow() {
    if (animate) {
      reheat(1);
    } else {
      settleAsync();
    }
  }

  // Settle the layout WITHOUT blocking the main thread: run the physics in
  // short, time-boxed slices across animation frames, then paint the final
  // positions once. Painting only at the end means no on-screen "jumping"
  // (the reason animation was turned off), while the UI never freezes. The old
  // synchronous 400-iteration pre-warm locked the thread for ~1-2s on open.
  let settleToken = 0;
  function settleAsync(iterations = 300) {
    cancelAnimationFrame(raf);
    running = false;
    const token = ++settleToken;
    if (nodes.length === 0) {
      alpha = 0;
      draw();
      return;
    }
    alpha = 1;
    let done = 0;
    const step = () => {
      if (token !== settleToken) return; // superseded by a newer settle/build
      const start = performance.now();
      // Cap the work per frame so input stays smooth at any node count.
      while (done < iterations && performance.now() - start < 8) {
        simulate();
        alpha *= 0.99;
        done++;
      }
      if (done < iterations) {
        raf = requestAnimationFrame(step);
      } else {
        alpha = 0;
        running = false;
        if (autoFit) fitView();
        draw();
      }
    };
    raf = requestAnimationFrame(step);
  }

  function reheat(a = 0.6) {
    if (!animate) {
      // Static mode: re-settle off the main thread, then repaint.
      settleAsync(200);
      return;
    }
    alpha = Math.max(alpha, a);
    wake();
  }

  function wake() {
    if (!running) {
      running = true;
      raf = requestAnimationFrame(tick);
    }
  }

  // Light-touch feedback during interaction: animate mode gently reheats,
  // static mode just repaints the moved node/edges without a physics pass.
  function nudge() {
    if (animate) {
      alpha = Math.max(alpha, 0.3);
      wake();
    } else {
      draw();
    }
  }

  function radiusOf(n: SimNode): number {
    // Scale by how referenced a topic is relative to the busiest one.
    const rel = Math.sqrt(n.degree / maxDegree);
    return (3 + rel * 13) * nodeScale;
  }

  function screenRadiusOf(node: SimNode): number {
    return Math.max(2.5, radiusOf(node) * Math.sqrt(scale));
  }

  function hasActiveSearch(): boolean {
    return searchText.trim().length > 0;
  }

  function nodeMatchesSearch(node: SimNode): boolean {
    return !hasActiveSearch() || fuzzyScore(node.title, searchText) !== null;
  }

  function visibleNodeIdSet(): Set<string> | null {
    if (!hasActiveSearch()) return null;
    const visibleIds = new Set<string>();
    for (const node of nodes) {
      if (nodeMatchesSearch(node)) visibleIds.add(node.id);
    }
    return visibleIds;
  }

  function updateSearchMatchCount(): void {
    if (!hasActiveSearch()) {
      searchMatchCount = nodes.length;
      return;
    }
    let count = 0;
    for (const node of nodes) {
      if (nodeMatchesSearch(node)) count++;
    }
    searchMatchCount = count;
  }

  function simulate() {
    const n = nodes.length;
    if (n === 0) return;
    const repel = chargeStrength * chargeStrength;
    const layoutScale = linkDistance / 70;
    const radii = nodes.map(radiusOf);

    // Repulsion (O(n^2), fine for the capped node counts we render).
    for (let i = 0; i < n; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < n; j++) {
        const b = nodes[j];
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 0.01) {
          const angle = (i + j * n) * 2.399963229728653;
          dx = Math.cos(angle);
          dy = Math.sin(angle);
          d2 = 1;
        }
        const d = Math.sqrt(d2);
        const collision = Math.max(0, radii[i] + radii[j] + 5 - d) * 0.7;
        const force = repel / d2 + collision;
        const fx = (dx / d) * force;
        const fy = (dy / d) * force;
        a.vx += fx;
        a.vy += fy;
        b.vx -= fx;
        b.vy -= fy;
      }
    }

    // Bridges stay long and weak; suggestions never rearrange accepted topics.
    for (const e of edges) {
      if (e.suggested) continue;
      const dx = e.target.x - e.source.x;
      const dy = e.target.y - e.source.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
      const w = 0.6 + 0.4 * (e.weight / maxWeight);
      const force = (d - e.distance * layoutScale) * (e.internal ? 0.018 : 0.004) * w;
      const fx = (dx / d) * force;
      const fy = (dy / d) * force;
      e.source.vx += fx;
      e.source.vy += fy;
      e.target.vx -= fx;
      e.target.vy -= fy;
    }

    // Structural anchors preserve the coarse community layout instead of
    // pulling every topic toward the same screen center.
    for (const node of nodes) {
      if (node === dragNode) {
        node.vx = 0;
        node.vy = 0;
        continue;
      }
      node.vx += (node.anchorX * layoutScale - node.x) * 0.085;
      node.vy += (node.anchorY * layoutScale - node.y) * 0.085;
      node.vx *= 0.85;
      node.vy *= 0.85;
      node.x += node.vx * alpha;
      node.y += node.vy * alpha;
    }
  }

  function toScreen(x: number, y: number): [number, number] {
    return [x * scale + offsetX, y * scale + offsetY];
  }
  function toWorld(sx: number, sy: number): [number, number] {
    return [(sx - offsetX) / scale, (sy - offsetY) / scale];
  }

  function draw() {
    if (!ctx) return;
    ctx.save();
    ctx.clearRect(0, 0, width, height);
    const theme = getComputedStyle(document.documentElement);
    const themeColor = (name: string, fallback: string) =>
      theme.getPropertyValue(name).trim() || fallback;
    const backgroundColor = themeColor("--bg-primary", "#16161e");
    ctx.fillStyle = backgroundColor;
    ctx.fillRect(0, 0, width, height);

    const edgeColor = themeColor("--text-muted", "#777");
    const textColor = themeColor("--text-secondary", "#aaa");
    const primaryColor = themeColor("--text-primary", "#fff");
    const focusColor = themeColor("--accent-yellow", "#e0af68");
    const matchColor = themeColor("--accent-green", "#9ece6a");
    const visibleIds = visibleNodeIdSet();
    const palette = new Map<number, string>();
    const colorFor = (id: string): string => {
      const index = communityLayout?.clusterIndexById.get(id);
      if (index === undefined) return edgeColor;
      const cached = palette.get(index);
      if (cached) return cached;
      const resolved = clusterColor(index, theme.colorScheme === "light");
      palette.set(index, resolved);
      return resolved;
    };
    const neighbors = new Set<string>();
    if (hoverNode) {
      for (const edge of edges) {
        if (edge.source === hoverNode) neighbors.add(edge.target.id);
        if (edge.target === hoverNode) neighbors.add(edge.source.id);
      }
    }

    // Edges — thickness/opacity scale with tie magnitude (weight), and hue
    // follows the cluster. An edge *between* clusters keeps the neutral border
    // colour, which makes bridges between topics legible as the pale lines.
    for (const e of edges) {
      if (visibleIds && (!visibleIds.has(e.source.id) || !visibleIds.has(e.target.id))) {
        continue;
      }
      const [x1, y1] = toScreen(e.source.x, e.source.y);
      const [x2, y2] = toScreen(e.target.x, e.target.y);
      const rel = e.weight / maxWeight;
      ctx.strokeStyle = e.internal && !e.suggested ? colorFor(e.source.id) : edgeColor;
      ctx.setLineDash(e.suggested ? [4 * scale, 4 * scale] : []);
      ctx.lineWidth = Math.max(0.4, (e.suggested ? 0.4 + rel * 2 : 0.6 + rel * 3.5) * scale);
      const highlighted = e.source === hoverNode || e.target === hoverNode;
      ctx.globalAlpha = hoverNode && !highlighted ? 0.08 :
        highlighted ? 0.9 : e.suggested ? 0.3 : e.internal ? 0.25 + rel * 0.25 : 0.65;
      ctx.beginPath();
      ctx.moveTo(x1, y1);
      ctx.lineTo(x2, y2);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    type Label = { node: SimNode; x: number; y: number; r: number; priority: number };
    const labels: Label[] = [];
    const occupied: { left: number; right: number; top: number; bottom: number }[] = [];
    const groupBounds = new Map<number, { left: number; right: number; top: number; bottom: number }>();
    for (const node of nodes) {
      if (visibleIds && !visibleIds.has(node.id)) continue;
      const [x, y] = toScreen(node.x, node.y);
      const r = screenRadiusOf(node);
      const isMatch = hasActiveSearch();
      const isFocus = node.id === currentPageId;
      const isHover = node === hoverNode;
      const isHub = communityLayout?.hubIds.has(node.id) ?? false;

      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      // Focus and search-match keep dedicated theme accents so they stay
      // distinguishable from whatever hue their cluster happens to hold; every
      // other node wears its cluster's colour. These were hardcoded hex, which
      // was unreadable on light themes.
      if (isFocus) ctx.fillStyle = focusColor;
      else if (isMatch) ctx.fillStyle = matchColor;
      else ctx.fillStyle = colorFor(node.id);
      ctx.globalAlpha = hoverNode && !isHover && !neighbors.has(node.id) ? 0.35 : 1;
      ctx.fill();

      if (isHover || isFocus) {
        ctx.lineWidth = 2;
        ctx.strokeStyle = primaryColor;
        ctx.stroke();
      }
      ctx.globalAlpha = 1;

      occupied.push({ left: x - r - 2, right: x + r + 2, top: y - r - 2, bottom: y + r + 2 });
      const group = communityLayout?.clusterIndexById.get(node.id);
      if (group !== undefined) {
        const bounds = groupBounds.get(group);
        groupBounds.set(group, {
          left: Math.min(bounds?.left ?? x - r, x - r),
          right: Math.max(bounds?.right ?? x + r, x + r),
          top: Math.min(bounds?.top ?? y - r, y - r),
          bottom: Math.max(bounds?.bottom ?? y + r, y + r),
        });
      }
      if (isHover || (showLabels && (scale > 0.65 || r > 6 || isHub || isFocus || isMatch || neighbors.has(node.id)))) {
        labels.push({
          node, x, y, r,
          priority: isHover ? 5 : isFocus ? 4 : isHub ? 3 : isMatch || neighbors.has(node.id) ? 2 : 1,
        });
      }
    }
    // Read the hubs first; fit secondary labels only where they don't obscure
    // other labels or nodes. Zooming reveals the remaining titles.
    labels.sort((a, b) => b.priority - a.priority || b.node.degree - a.node.degree || a.node.id.localeCompare(b.node.id));
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    for (const { node, x, y, r, priority } of labels) {
      const fontSize = priority >= 3 ? 12 : Math.max(10, 11 * Math.min(scale, 1.5));
      ctx.font = `${priority >= 3 ? 600 : 400} ${fontSize}px system-ui, sans-serif`;
      const label = node.title.length > 36 ? node.title.slice(0, 35) + "…" : node.title;
      const halfWidth = ctx.measureText(label).width / 2 + 4;
      const placements = [
        [x, y + r + 5],
        [x, y - r - fontSize - 6],
        [x + r + halfWidth + 4, y - fontSize / 2],
        [x - r - halfWidth - 4, y - fontSize / 2],
      ];
      if (communityLayout?.hubIds.has(node.id)) {
        const index = communityLayout.clusterIndexById.get(node.id)!;
        const bounds = groupBounds.get(index)!;
        const centerX = (bounds.left + bounds.right) / 2;
        placements.push(
          [centerX, bounds.bottom + 6],
          [centerX, bounds.top - fontSize - 7],
          [bounds.right + halfWidth + 5, y - fontSize / 2],
          [bounds.left - halfWidth - 5, y - fontSize / 2],
        );
      }
      for (const [lx, ly] of placements) {
        const box = { left: lx - halfWidth, right: lx + halfWidth, top: ly - 2, bottom: ly + fontSize + 3 };
        if (box.left < 4 || box.right > width - 4 || box.top < 4 || box.bottom > height - 4) continue;
        if (occupied.some((other) => box.left < other.right && box.right > other.left &&
          box.top < other.bottom && box.bottom > other.top)) continue;
        ctx.fillStyle = priority >= 3 ? primaryColor : textColor;
        ctx.strokeStyle = backgroundColor;
        ctx.lineWidth = 3;
        ctx.strokeText(label, lx, ly);
        ctx.fillText(label, lx, ly);
        occupied.push(box);
        break;
      }
    }
    ctx.restore();
  }

  function tick() {
    if (destroyed) return;
    if (alpha > MIN_ALPHA) {
      simulate();
      alpha *= 0.985;
    }
    draw();
    if (alpha > MIN_ALPHA || dragNode || panning) {
      raf = requestAnimationFrame(tick);
    } else {
      running = false;
      if (autoFit) {
        fitView();
        draw();
      }
    }
  }

  function resize() {
    if (!canvasEl || !wrapperEl) return;
    dpr = window.devicePixelRatio || 1;
    width = wrapperEl.clientWidth;
    height = wrapperEl.clientHeight;
    canvasEl.width = width * dpr;
    canvasEl.height = height * dpr;
    canvasEl.style.width = width + "px";
    canvasEl.style.height = height + "px";
    ctx = canvasEl.getContext("2d");
    if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (autoFit) fitView();
    draw();
  }

  // ---- Pointer interaction ----
  function pickNode(sx: number, sy: number): SimNode | null {
    const [wx, wy] = toWorld(sx, sy);
    let best: SimNode | null = null;
    let bestD = Infinity;
    for (const node of nodes) {
      if (!nodeMatchesSearch(node)) continue;
      const dx = node.x - wx;
      const dy = node.y - wy;
      const d = dx * dx + dy * dy;
      const r = (screenRadiusOf(node) + 4) / scale;
      if (d < r * r && d < bestD) {
        best = node;
        bestD = d;
      }
    }
    return best;
  }

  function onPointerDown(e: PointerEvent) {
    if (!canvasEl) return;
    if (!animate) {
      settleToken++;
      cancelAnimationFrame(raf);
      alpha = 0;
      running = false;
    }
    canvasEl.setPointerCapture(e.pointerId);
    const rect = canvasEl.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    pointerMoved = false;
    pointerDownX = sx;
    pointerDownY = sy;
    lastX = sx;
    lastY = sy;
    const hit = pickNode(sx, sy);
    if (hit) {
      dragNode = hit;
      nudge();
    } else {
      panning = true;
      wake();
    }
  }

  function onPointerMove(e: PointerEvent) {
    if (!canvasEl) return;
    const rect = canvasEl.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    // Treating *any* pointermove as a drag made nodes effectively unclickable:
    // a real mouse emits a sub-pixel move between press and release almost
    // every time, which marked the gesture as a drag and suppressed
    // navigation. Classify by distance instead.
    if (!pointerMoved && exceedsDragThreshold(pointerDownX, pointerDownY, sx, sy)) {
      pointerMoved = true;
    }

    if (dragNode) {
      // Hold the node still until the gesture is genuinely a drag, so a click
      // can't nudge the graph out from under the pointer.
      if (pointerMoved) {
        const [wx, wy] = toWorld(sx, sy);
        dragNode.x = wx;
        dragNode.y = wy;
        dragNode.anchorX = wx * 70 / linkDistance;
        dragNode.anchorY = wy * 70 / linkDistance;
        autoFit = false;
        dragNode.vx = 0;
        dragNode.vy = 0;
        nudge();
      }
    } else if (panning) {
      if (pointerMoved) {
        autoFit = false;
        offsetX += sx - lastX;
        offsetY += sy - lastY;
        wake();
      }
      lastX = sx;
      lastY = sy;
    } else {
      const prev = hoverNode;
      hoverNode = pickNode(sx, sy);
      if (canvasEl) canvasEl.style.cursor = hoverNode ? "pointer" : "grab";
      if (prev !== hoverNode) draw();
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (!canvasEl) return;
    const rect = canvasEl.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    if (dragNode && !pointerMoved) {
      onNavigate(dragNode.title, searchText.trim());
    } else if (!dragNode && !pointerMoved) {
      const hit = pickNode(sx, sy);
      if (hit) onNavigate(hit.title, searchText.trim());
    }
    dragNode = null;
    panning = false;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    if (!canvasEl) return;
    autoFit = false;
    const rect = canvasEl.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
    const [wx, wy] = toWorld(sx, sy);
    scale = Math.min(6, Math.max(0.01, scale * factor));
    // keep the cursor point stable
    offsetX = sx - wx * scale;
    offsetY = sy - wy * scale;
    draw();
  }

  function zoomBy(factor: number) {
    autoFit = false;
    const cx = width / 2;
    const cy = height / 2;
    const [wx, wy] = toWorld(cx, cy);
    scale = Math.min(6, Math.max(0.01, scale * factor));
    offsetX = cx - wx * scale;
    offsetY = cy - wy * scale;
    draw();
  }

  function fitView() {
    if (!nodes.length || width <= 0 || height <= 0) return;
    const left = Math.min(...nodes.map((node) => node.x - radiusOf(node)));
    const right = Math.max(...nodes.map((node) => node.x + radiusOf(node)));
    const top = Math.min(...nodes.map((node) => node.y - radiusOf(node)));
    const bottom = Math.max(...nodes.map((node) => node.y + radiusOf(node)));
    const padding = Math.min(70, width * 0.12, height * 0.12);
    scale = Math.min(1.5, (width - padding * 2) / Math.max(1, right - left),
      (height - padding * 2) / Math.max(1, bottom - top));
    offsetX = width / 2 - (left + right) * scale / 2;
    offsetY = height / 2 - (top + bottom) * scale / 2;
  }

  function resetView() {
    autoFit = true;
    fitView();
    draw();
  }

  async function scanSuggestedLinks() {
    scanningSuggestions = true;
    suggestionError = null;
    try {
      const focus = mode === "local" ? currentPageId || undefined : undefined;
      await discoverLinkCandidates(focus, mode === "local" ? 200 : 500);
      showSuggestedEdges = true;
      await loadData();
    } catch (e) {
      suggestionError = String(e);
    } finally {
      scanningSuggestions = false;
    }
  }

  // ---- Lifecycle ----
  onMount(() => {
    if (!canvasEl || !wrapperEl) return;
    resize();
    const ro = new ResizeObserver(resize);
    const themeObserver = new MutationObserver(() => draw());
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["style", "class", "data-theme"] });
    ro.observe(wrapperEl);
    window.addEventListener("resize", resize);
    void loadData();
    return () => {
      ro.disconnect();
      themeObserver.disconnect();
      destroyed = true;
      loadGeneration++;
      settleToken++;
      window.removeEventListener("resize", resize);
      cancelAnimationFrame(raf);
      running = false;
    };
  });

  // Reload when mode or node limit changes.
  let lastMode: "global" | "local" = "global";
  let lastLimit = 200;
  let lastShowSuggestedEdges = false;
  let lastFocus = "";
  $effect(() => {
    const focus = mode === "local" ? currentPageId : "";
    if (
      mode !== lastMode ||
      nodeLimit !== lastLimit ||
      showSuggestedEdges !== lastShowSuggestedEdges ||
      focus !== lastFocus
    ) {
      lastMode = mode;
      lastLimit = nodeLimit;
      lastShowSuggestedEdges = showSuggestedEdges;
      lastFocus = focus;
      untrack(() => void loadData());
    }
  });

  // Redraw when purely visual controls change.
  $effect(() => {
    nodeScale;
    showLabels;
    searchText;
    updateSearchMatchCount();
    draw();
  });

  // Physics knobs: nudge the simulation so changes take effect.
  $effect(() => {
    chargeStrength;
    linkDistance;
    nodeScale;
    untrack(() => reheat(0.4));
  });

  // Start/stop the live animation when the toggle flips.
  let lastAnimate = false;
  $effect(() => {
    if (animate !== lastAnimate) {
      lastAnimate = animate;
      settleToken++;
      cancelAnimationFrame(raf);
      running = false;
      if (animate) {
        alpha = Math.max(alpha, 0.6);
        wake();
      } else {
        alpha = 0;
        draw();
      }
    }
  });
</script>

<div class="graph-view">
  <div class="graph-canvas-wrap" bind:this={wrapperEl}>
    <canvas
      bind:this={canvasEl}
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerUp}
      onpointercancel={() => { dragNode = null; panning = false; }}
      onpointerleave={() => { if (!dragNode) { hoverNode = null; draw(); } }}
      onwheel={onWheel}
    ></canvas>

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

    <!-- Zoom buttons -->
    <div class="zoom-controls">
      <button title="Zoom in" onclick={() => zoomBy(1.2)}>+</button>
      <button title="Zoom out" onclick={() => zoomBy(1 / 1.2)}>−</button>
      <button title="Reset view" onclick={resetView}>⤢</button>
    </div>
  </div>

  <!-- Controls panel (Logseq-style). On phones this docks to the bottom. -->
  <aside class="graph-controls" class:open={mobileControlsOpen}>
    <button
      type="button"
      class="graph-controls-handle"
      aria-expanded={mobileControlsOpen}
      onclick={() => (mobileControlsOpen = !mobileControlsOpen)}
    >
      <span class="handle-grip" aria-hidden="true"></span>
      <span>Graph</span>
      <span class="handle-chevron">{mobileControlsOpen ? "▾" : "▴"}</span>
    </button>
    <h2>Graph</h2>

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
      <input type="text" data-local-search placeholder="Filter nodes…" bind:value={searchText} />
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

    <label class="ctrl">
      <span>Repel force: {chargeStrength}</span>
      <input type="range" min="20" max="150" step="5" bind:value={chargeStrength} />
    </label>

    <label class="ctrl">
      <span>Link distance: {linkDistance}</span>
      <input type="range" min="20" max="200" step="5" bind:value={linkDistance} />
    </label>

    <label class="ctrl">
      <span>Node size: {nodeScale.toFixed(1)}×</span>
      <input type="range" min="0.5" max="3" step="0.1" bind:value={nodeScale} />
    </label>

    <label class="ctrl checkbox">
      <input type="checkbox" bind:checked={showLabels} />
      <span>Show labels</span>
    </label>

    <label class="ctrl checkbox">
      <input type="checkbox" bind:checked={animate} />
      <span>Animate layout</span>
    </label>

    <label class="ctrl checkbox">
      <input type="checkbox" bind:checked={showSuggestedEdges} />
      <span>Show suggested edges</span>
    </label>

    <button
      class="suggestion-scan-btn"
      type="button"
      onclick={scanSuggestedLinks}
      disabled={scanningSuggestions}
    >
      {scanningSuggestions ? "Scanning..." : "Scan for suggested links"}
    </button>

    {#if suggestionError}
      <div class="suggestion-error">{suggestionError}</div>
    {/if}

    <div class="graph-stats">
      {stats.nodes.toLocaleString()} nodes · {stats.edges.toLocaleString()} links · {stats.communities.toLocaleString()} communities
      {#if showSuggestedEdges && stats.suggested > 0}
        · {stats.suggested.toLocaleString()} suggested
      {/if}
    </div>
    <p class="hint">Colors group densely linked pages. Long gray links connect communities. Click a node to open it. Dashed lines are suggestions until you accept them on a page.</p>
  </aside>
</div>

<style>
  .graph-view {
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
  }

  canvas {
    display: block;
    cursor: grab;
    touch-action: none;
  }

  .graph-overlay {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    color: var(--text-muted);
    font-size: 14px;
    pointer-events: none;
  }
  .graph-overlay.error {
    color: var(--danger, #e74c3c);
    max-width: 60%;
    text-align: center;
  }

  .zoom-controls {
    position: absolute;
    left: 12px;
    bottom: 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .zoom-controls button {
    width: 32px;
    height: 32px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-primary);
    font-size: 16px;
    cursor: pointer;
  }
  .zoom-controls button:hover {
    background: var(--bg-hover);
  }

  .graph-controls {
    width: 240px;
    flex-shrink: 0;
    border-left: 1px solid var(--border);
    background: var(--bg-secondary);
    padding: 16px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .graph-controls h2 {
    margin: 0;
    font-size: 18px;
    color: var(--text-primary);
  }

  .mode-toggle {
    display: flex;
    gap: 4px;
  }
  .mode-toggle button {
    flex: 1;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: none;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 13px;
  }
  .mode-toggle button.active {
    background: var(--bg-active);
    color: var(--text-primary);
    border-color: var(--accent);
  }
  .mode-toggle button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .focus-label {
    font-size: 12px;
    color: var(--text-muted);
  }
  .focus-label strong {
    color: var(--text-primary);
  }

  .ctrl {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    color: var(--text-secondary);
  }
  .ctrl input[type="range"] {
    width: 100%;
  }
  .ctrl input[type="text"] {
    padding: 6px 8px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-primary);
    font-size: 13px;
    outline: none;
  }
  .ctrl.checkbox {
    flex-direction: row;
    align-items: center;
    gap: 8px;
  }

  .suggestion-scan-btn {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    cursor: pointer;
    font-size: 12px;
    padding: 7px 10px;
  }

  .suggestion-scan-btn:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .suggestion-scan-btn:disabled {
    opacity: 0.65;
    cursor: default;
  }

  .suggestion-error {
    border: 1px solid var(--danger);
    border-radius: 6px;
    color: var(--danger);
    font-size: 12px;
    padding: 8px;
  }

  .graph-stats {
    font-size: 12px;
    color: var(--text-muted);
    border-top: 1px solid var(--border);
    padding-top: 12px;
  }
  .hint {
    font-size: 11px;
    color: var(--text-muted);
    margin: 0;
    line-height: 1.5;
  }

  .graph-controls-handle {
    display: none;
  }

  @media (max-width: 640px) {
    .graph-view {
      flex-direction: column;
    }

    .graph-controls-handle {
      display: flex;
      position: relative;
      align-items: center;
      justify-content: center;
      gap: 8px;
      width: 100%;
      min-height: 44px;
      border: none;
      background: transparent;
      color: var(--text-primary);
      font-size: 14px;
      font-weight: 600;
      cursor: pointer;
      flex-shrink: 0;
    }

    .handle-grip {
      position: absolute;
      top: 8px;
      left: 50%;
      width: 36px;
      height: 4px;
      margin-left: -18px;
      border-radius: 999px;
      background: var(--border);
    }

    .handle-chevron {
      color: var(--text-muted);
      font-weight: 500;
    }

    .graph-controls {
      width: 100%;
      flex-shrink: 0;
      border-left: none;
      border-top: 1px solid var(--border);
      border-radius: 14px 14px 0 0;
      max-height: 48px;
      padding: 0 12px;
      overflow: hidden;
      gap: 10px;
    }

    .graph-controls.open {
      max-height: min(46vh, 380px);
      overflow-y: auto;
      padding-bottom: 12px;
    }

    .graph-controls h2 {
      display: none;
    }

    .zoom-controls {
      bottom: 12px;
    }
  }
</style>
