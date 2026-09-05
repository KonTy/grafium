<script lang="ts">
  import { getGraphData, type GraphData } from "../lib/api";
  import { assignClusterHues, edgeHue } from "../lib/graphColor";
  import type { TagHue } from "../lib/tagColor";

  interface Props {
    onNavigate: (title: string) => void;
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
  }
  interface SimEdge {
    source: SimNode;
    target: SimNode;
    weight: number;
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
  let searchText = $state("");
  let animate = $state(false); // live jiggle; off = settle instantly, stay still

  let loading = $state(false);
  let errorMsg = $state<string | null>(null);
  let stats = $state({ nodes: 0, edges: 0 });

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

  // ---- Simulation data (non-reactive, mutated in the rAF loop) ----
  /// Cluster hue per node id — see `graphColor.ts`. Recomputed on load, not on
  /// every frame: it depends only on topology, which doesn't change as the
  /// layout settles.
  let clusterHues = new Map<string, TagHue>();
  /// Resolved `--accent-<hue>` values, read once per draw. `getComputedStyle`
  /// is a layout-flushing call, so doing it per node would cost a reflow for
  /// every dot on screen.
  let huePalette = new Map<string, string>();

  let nodes: SimNode[] = [];
  let edges: SimEdge[] = [];
  let nodeById = new Map<string, SimNode>();
  let maxDegree = 1;
  let maxWeight = 1;
  let alpha = 0;
  let raf = 0;
  let running = false;

  // Interaction
  let dragNode: SimNode | null = null;
  let hoverNode: SimNode | null = null;
  let panning = false;
  let pointerMoved = false;
  let lastX = 0;
  let lastY = 0;

  const MIN_ALPHA = 0.008;

  function themeColor(varName: string, fallback: string): string {
    if (typeof window === "undefined") return fallback;
    const v = getComputedStyle(document.documentElement).getPropertyValue(varName).trim();
    return v || fallback;
  }

  async function loadData() {
    loading = true;
    errorMsg = null;
    try {
      const focus = mode === "local" ? currentPageId || undefined : undefined;
      const data: GraphData = await getGraphData(nodeLimit, focus);
      buildSimulation(data);
      stats = { nodes: data.nodes.length, edges: data.edges.length };
    } catch (e) {
      errorMsg = String(e);
      console.error("Failed to load graph data:", e);
    }
    loading = false;
  }

  function buildSimulation(data: GraphData) {
    clusterHues = assignClusterHues(
      data.nodes.map((n) => n.id),
      data.edges.map((e) => ({ source: e.source, target: e.target }))
    );
    nodeById = new Map();
    const cx = width / 2;
    const cy = height / 2;
    const R = Math.min(width, height) * 0.4 || 300;
    nodes = data.nodes.map((n, i) => {
      const angle = (i / Math.max(1, data.nodes.length)) * Math.PI * 2;
      const node: SimNode = {
        id: n.id,
        title: n.title,
        degree: n.degree,
        x: cx + Math.cos(angle) * R * (0.5 + Math.random() * 0.5),
        y: cy + Math.sin(angle) * R * (0.5 + Math.random() * 0.5),
        vx: 0,
        vy: 0,
      };
      nodeById.set(n.id, node);
      return node;
    });
    edges = [];
    for (const e of data.edges) {
      const s = nodeById.get(e.source);
      const t = nodeById.get(e.target);
      if (s && t) edges.push({ source: s, target: t, weight: e.weight ?? 1 });
    }
    maxDegree = Math.max(1, ...data.nodes.map((n) => n.degree));
    maxWeight = Math.max(1, ...edges.map((e) => e.weight));
    // Reset camera to fit
    scale = 1;
    offsetX = 0;
    offsetY = 0;
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
    if (nodes.length === 0) {
      alpha = 0;
      draw();
      return;
    }
    const token = ++settleToken;
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

  function simulate() {
    const n = nodes.length;
    if (n === 0) return;
    const cx = width / 2;
    const cy = height / 2;
    const repel = chargeStrength * chargeStrength;

    // Repulsion (O(n^2), fine for the capped node counts we render).
    for (let i = 0; i < n; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < n; j++) {
        const b = nodes[j];
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 0.01) {
          dx = (Math.random() - 0.5) * 1;
          dy = (Math.random() - 0.5) * 1;
          d2 = dx * dx + dy * dy + 0.01;
        }
        const force = repel / d2;
        const d = Math.sqrt(d2);
        const fx = (dx / d) * force;
        const fy = (dy / d) * force;
        a.vx += fx;
        a.vy += fy;
        b.vx -= fx;
        b.vy -= fy;
      }
    }

    // Spring along edges (heavier ties pull a little stronger).
    for (const e of edges) {
      const dx = e.target.x - e.source.x;
      const dy = e.target.y - e.source.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
      const w = 0.6 + 0.4 * (e.weight / maxWeight);
      const force = (d - linkDistance) * 0.02 * w;
      const fx = (dx / d) * force;
      const fy = (dy / d) * force;
      e.source.vx += fx;
      e.source.vy += fy;
      e.target.vx -= fx;
      e.target.vy -= fy;
    }

    // Centering + integration.
    for (const node of nodes) {
      if (node === dragNode) {
        node.vx = 0;
        node.vy = 0;
        continue;
      }
      node.vx += (cx - node.x) * 0.0015;
      node.vy += (cy - node.y) * 0.0015;
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
    ctx.fillStyle = themeColor("--bg-primary", "#16161e");
    ctx.fillRect(0, 0, width, height);

    const edgeColor = themeColor("--border", "#333");
    const nodeColor = themeColor("--accent", "#6ea8fe");
    const textColor = themeColor("--text-secondary", "#aaa");
    const q = searchText.trim().toLowerCase();

    // One `getComputedStyle` per hue per frame instead of one per element.
    huePalette = new Map();
    const hueColor = (hue: TagHue | null): string => {
      if (!hue) return edgeColor;
      const cached = huePalette.get(hue);
      if (cached) return cached;
      const resolved = themeColor(`--accent-${hue}`, nodeColor);
      huePalette.set(hue, resolved);
      return resolved;
    };

    // Edges — thickness/opacity scale with tie magnitude (weight), and hue
    // follows the cluster. An edge *between* clusters keeps the neutral border
    // colour, which makes bridges between topics legible as the pale lines.
    for (const e of edges) {
      const [x1, y1] = toScreen(e.source.x, e.source.y);
      const [x2, y2] = toScreen(e.target.x, e.target.y);
      const rel = e.weight / maxWeight;
      ctx.strokeStyle = hueColor(
        edgeHue(clusterHues, { source: e.source.id, target: e.target.id })
      );
      ctx.lineWidth = Math.max(0.4, (0.6 + rel * 3.5) * scale);
      ctx.globalAlpha = 0.22 + rel * 0.5;
      ctx.beginPath();
      ctx.moveTo(x1, y1);
      ctx.lineTo(x2, y2);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;

    // Nodes
    for (const node of nodes) {
      const [x, y] = toScreen(node.x, node.y);
      const r = radiusOf(node) * scale;
      const isMatch = q.length > 0 && node.title.toLowerCase().includes(q);
      const isFocus = node.id === currentPageId;
      const isHover = node === hoverNode;

      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      // Focus and search-match keep dedicated theme accents so they stay
      // distinguishable from whatever hue their cluster happens to hold; every
      // other node wears its cluster's colour. These were hardcoded hex, which
      // was unreadable on light themes.
      if (isFocus) ctx.fillStyle = themeColor("--accent-yellow", "#e0af68");
      else if (isMatch) ctx.fillStyle = themeColor("--accent-green", "#9ece6a");
      else ctx.fillStyle = hueColor(clusterHues.get(node.id) ?? null);
      ctx.globalAlpha = q.length > 0 && !isMatch && !isFocus ? 0.25 : 1;
      ctx.fill();

      if (isHover || isFocus) {
        ctx.lineWidth = 2;
        ctx.strokeStyle = themeColor("--text-primary", "#fff");
        ctx.stroke();
      }
      ctx.globalAlpha = 1;

      const showThis =
        showLabels && (scale > 0.75 || r > 6 || isHover || isFocus || isMatch);
      if (showThis) {
        ctx.fillStyle = textColor;
        ctx.font = `${Math.max(10, 11 * Math.min(scale, 1.5))}px sans-serif`;
        ctx.textAlign = "center";
        ctx.textBaseline = "top";
        const label = node.title.length > 28 ? node.title.slice(0, 27) + "…" : node.title;
        ctx.fillText(label, x, y + r + 2);
      }
    }
    ctx.restore();
  }

  function tick() {
    if (alpha > MIN_ALPHA) {
      simulate();
      alpha *= 0.985;
    }
    draw();
    if (alpha > MIN_ALPHA || dragNode || panning) {
      raf = requestAnimationFrame(tick);
    } else {
      running = false;
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
    draw();
  }

  // ---- Pointer interaction ----
  function pickNode(sx: number, sy: number): SimNode | null {
    const [wx, wy] = toWorld(sx, sy);
    let best: SimNode | null = null;
    let bestD = Infinity;
    for (const node of nodes) {
      const dx = node.x - wx;
      const dy = node.y - wy;
      const d = dx * dx + dy * dy;
      const r = radiusOf(node) + 4;
      if (d < r * r && d < bestD) {
        best = node;
        bestD = d;
      }
    }
    return best;
  }

  function onPointerDown(e: PointerEvent) {
    if (!canvasEl) return;
    canvasEl.setPointerCapture(e.pointerId);
    const rect = canvasEl.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    pointerMoved = false;
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
    if (dragNode) {
      const [wx, wy] = toWorld(sx, sy);
      dragNode.x = wx;
      dragNode.y = wy;
      dragNode.vx = 0;
      dragNode.vy = 0;
      pointerMoved = true;
      nudge();
    } else if (panning) {
      offsetX += sx - lastX;
      offsetY += sy - lastY;
      lastX = sx;
      lastY = sy;
      pointerMoved = true;
      wake();
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
      onNavigate(dragNode.title);
    } else if (!dragNode && !pointerMoved) {
      const hit = pickNode(sx, sy);
      if (hit) onNavigate(hit.title);
    }
    dragNode = null;
    panning = false;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    if (!canvasEl) return;
    const rect = canvasEl.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
    const [wx, wy] = toWorld(sx, sy);
    scale = Math.min(6, Math.max(0.15, scale * factor));
    // keep the cursor point stable
    offsetX = sx - wx * scale;
    offsetY = sy - wy * scale;
    draw();
  }

  function zoomBy(factor: number) {
    const cx = width / 2;
    const cy = height / 2;
    const [wx, wy] = toWorld(cx, cy);
    scale = Math.min(6, Math.max(0.15, scale * factor));
    offsetX = cx - wx * scale;
    offsetY = cy - wy * scale;
    draw();
  }

  function resetView() {
    scale = 1;
    offsetX = 0;
    offsetY = 0;
    reheat(0.8);
  }

  // ---- Lifecycle ----
  $effect(() => {
    if (!canvasEl || !wrapperEl) return;
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(wrapperEl);
    window.addEventListener("resize", resize);
    void loadData();
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", resize);
      cancelAnimationFrame(raf);
      running = false;
    };
  });

  // Reload when mode or node limit changes.
  let lastMode = mode;
  let lastLimit = nodeLimit;
  $effect(() => {
    if (mode !== lastMode || nodeLimit !== lastLimit) {
      lastMode = mode;
      lastLimit = nodeLimit;
      void loadData();
    }
  });

  // Redraw when purely visual controls change.
  $effect(() => {
    nodeScale;
    showLabels;
    searchText;
    draw();
  });

  // Physics knobs: nudge the simulation so changes take effect.
  $effect(() => {
    chargeStrength;
    linkDistance;
    reheat(0.4);
  });

  // Start/stop the live animation when the toggle flips.
  let lastAnimate = animate;
  $effect(() => {
    if (animate !== lastAnimate) {
      lastAnimate = animate;
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
    {/if}

    <!-- Zoom buttons -->
    <div class="zoom-controls">
      <button title="Zoom in" onclick={() => zoomBy(1.2)}>+</button>
      <button title="Zoom out" onclick={() => zoomBy(1 / 1.2)}>−</button>
      <button title="Reset view" onclick={resetView}>⤢</button>
    </div>
  </div>

  <!-- Controls panel (Logseq-style) -->
  <aside class="graph-controls">
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
      <input type="text" placeholder="Highlight nodes…" bind:value={searchText} />
    </label>

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

    <div class="graph-stats">
      {stats.nodes.toLocaleString()} nodes · {stats.edges.toLocaleString()} links
    </div>
    <p class="hint">Click a node to open it. Drag to move, scroll to zoom.</p>
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
</style>
