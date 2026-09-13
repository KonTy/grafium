const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

function makeFixture() {
  const nodes = [];
  const edges = [];
  for (const topic of ["Astronomy", "Gardening", "Music"]) {
    nodes.push({ id: topic, title: topic, degree: 0 });
    for (let i = 1; i <= 12; i++) {
      nodes.push({ id: `${topic}-${i}`, title: `${topic} / Note ${i}`, degree: 0 });
      edges.push({ source: topic, target: `${topic}-${i}`, weight: 4 });
      if (i > 1) edges.push({ source: `${topic}-${i - 1}`, target: `${topic}-${i}`, weight: 1 });
    }
  }
  edges.push({ source: "Astronomy", target: "Gardening", weight: 1 });
  edges.push({ source: "Gardening", target: "Music", weight: 1 });
  nodes.push({ id: "unlinked", title: "Unlinked page", degree: 0 });
  for (const edge of edges) {
    nodes.find((node) => node.id === edge.source).degree += edge.weight;
    nodes.find((node) => node.id === edge.target).degree += edge.weight;
  }
  return { nodes, edges };
}

async function openGraph(browser, fixture, theme, mobile) {
  const page = await browser.newPage({
    viewport: mobile ? { width: 390, height: 844 } : { width: 1440, height: 960 },
    isMobile: mobile,
    hasTouch: mobile,
    ...(mobile ? { userAgent: "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36" } : {}),
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(({ fixture, theme }) => {
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "graph", scrollTop: 0 }));
    localStorage.setItem("grafium.graphView.mode", "2d");
    window.__graphFixture = fixture;
    window.__graphCalls = 0;
    window.__openedGraphPages = [];
    let sequence = 0;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        switch (cmd) {
          case "get_graph_info": return { name: "Community fixture", path: "/tmp/community-fixture" };
          case "get_app_theme": return theme;
          case "get_smplos_theme": return null;
          case "get_graph_data":
            window.__graphCalls++;
            return structuredClone(fixture);
          case "get_page":
            window.__openedGraphPages.push(args.title);
            return {
              id: args.title, title: args.title, properties: {}, is_journal: false,
              file_path: `pages/${args.title}.md`, created_at: 0, updated_at: 0,
            };
          case "list_blocks": return [{
            id: "fixture-block", page_id: args.pageId, parent_id: null, order_index: 0,
            content: "Fixture note", block_type: "markdown", properties: {}, created_at: 0, updated_at: 0,
          }];
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
    // Observe actual canvas output without adding production debug globals.
    const proto = CanvasRenderingContext2D.prototype;
    for (const method of ["clearRect", "beginPath", "arc", "moveTo", "lineTo", "stroke", "fill", "fillText"]) {
      const original = proto[method];
      proto[method] = function (...args) {
        if (this.canvas.closest(".graph-canvas-wrap")) {
          if (method === "clearRect") window.__graphFrame = { nodes: [], edges: [], labels: [] };
          if (method === "beginPath") { this.__graphArc = null; this.__graphLine = null; }
          if (method === "arc") this.__graphArc = { x: args[0], y: args[1], r: args[2] };
          if (method === "moveTo") this.__graphLine = { source: args.slice(0, 2) };
          if (method === "lineTo" && this.__graphLine) this.__graphLine.target = args.slice(0, 2);
          if (method === "stroke" && this.__graphLine?.target) {
            window.__graphFrame.edges.push({ ...this.__graphLine, color: this.strokeStyle });
          }
          if (method === "fill" && this.__graphArc) {
            window.__graphFrame.nodes.push({ ...this.__graphArc, color: this.fillStyle });
          }
          if (method === "fillText") {
            window.__graphFrame.labels.push({
              text: args[0], x: args[1], y: args[2],
              width: this.measureText(args[0]).width, height: Number.parseFloat(this.font.match(/([\d.]+)px/)[1]),
            });
          }
        }
        return original.apply(this, args);
      };
    }
  }, { fixture, theme });
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.locator(".graph-canvas-wrap canvas").waitFor();
  await page.waitForFunction((count) => {
    if (window.__graphFrame?.nodes.length !== count) return false;
    const current = JSON.stringify(window.__graphFrame.nodes);
    if (current !== window.__graphLastPositions) {
      window.__graphLastPositions = current;
      window.__graphLastChange = performance.now();
    }
    return performance.now() - window.__graphLastChange > 250;
  }, fixture.nodes.length);
  return { page, errors };
}

async function snapshot(page) {
  return page.evaluate(() => ({
    ...window.__graphFrame,
    width: document.querySelector(".graph-canvas-wrap").clientWidth,
    height: document.querySelector(".graph-canvas-wrap").clientHeight,
    calls: window.__graphCalls,
  }));
}

(async () => {
  const fixture = process.env.GRAPH_FIXTURE
    ? JSON.parse(fs.readFileSync(process.env.GRAPH_FIXTURE, "utf8")) : makeFixture();
  const visibleIds = new Set(fixture.nodes.map((node) => node.id));
  const visibleEdges = fixture.edges.filter((edge) => visibleIds.has(edge.source) && visibleIds.has(edge.target));
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const [theme, mobile] of [["oled", false], ["github", false], ["oled", true]]) {
      const { page, errors } = await openGraph(browser, fixture, theme, mobile);
      const frame = await snapshot(page);
      if (process.env.GRAPH_SCREENSHOT_DIR) {
        await page.screenshot({
          path: path.join(process.env.GRAPH_SCREENSHOT_DIR,
            `graph-2d-${process.env.GRAPH_FIXTURE ? "knowledge-" : ""}${theme}-${mobile ? "mobile" : "desktop"}.png`),
        });
      }
      assert.equal(frame.nodes.length, fixture.nodes.length);
      assert.equal(frame.edges.length, visibleEdges.length, "every accepted link between visible nodes must still be drawn");
      assert.equal(frame.calls, 1, "drawing and theme initialization must not reload graph data");
      for (const node of frame.nodes) {
        assert.ok(Number.isFinite(node.x) && Number.isFinite(node.y));
        assert.ok(node.x - node.r >= 0 && node.x + node.r <= frame.width, "fit includes horizontal node bounds");
        assert.ok(node.y - node.r >= 0 && node.y + node.r <= frame.height, "fit includes vertical node bounds");
      }
      for (let i = 0; i < frame.labels.length; i++) {
        const a = frame.labels[i];
        for (const b of frame.labels.slice(i + 1)) {
          assert.ok(Math.abs(a.x - b.x) >= (a.width + b.width) / 2 ||
            a.y + a.height <= b.y || b.y + b.height <= a.y, "drawn labels must not overlap");
        }
      }
      if (!process.env.GRAPH_FIXTURE) {
        assert.match(await page.locator(".graph-stats").textContent(), /3 communities/);
        const byId = new Map(fixture.nodes.map((node, i) => [node.id, frame.nodes[i]]));
        const groups = ["Astronomy", "Gardening", "Music"].map((id) => {
          const center = byId.get(id);
          assert.ok(frame.labels.some((label) => label.text === id), `hub ${id} should remain labeled`);
          const members = fixture.nodes.filter((node) => node.id.startsWith(`${id}-`));
          assert.ok(members.every((node) => byId.get(node.id).color === center.color));
          return {
            id, center,
            radius: Math.max(...members.map((node) => Math.hypot(
              byId.get(node.id).x - center.x, byId.get(node.id).y - center.y,
            ))),
          };
        });
        assert.equal(new Set(groups.map((group) => group.center.color)).size, 3);
        assert.ok(groups.every((group) => group.center.color !== byId.get("unlinked").color),
          "unlinked pages must not borrow a community color");
        for (let i = 0; i < groups.length; i++) {
          for (const b of groups.slice(i + 1)) {
            const a = groups[i];
            const distance = Math.hypot(a.center.x - b.center.x, a.center.y - b.center.y);
            assert.ok(distance > a.radius + b.radius + 5, "settled communities need visible space between them");
          }
        }
      }
      if (!mobile && !process.env.GRAPH_FIXTURE) {
        await page.getByPlaceholder("Filter nodes…").fill("Music");
        await page.waitForFunction(() => window.__graphFrame.nodes.length === 13);
        assert.equal((await snapshot(page)).calls, frame.calls, "search filters without rebuilding the layout");
        await page.getByPlaceholder("Filter nodes…").fill("");
        await page.waitForFunction(() => window.__graphFrame.nodes.length === 40);
        const restored = await snapshot(page);
        assert.deepEqual(restored.nodes, frame.nodes, "clearing search preserves positions and colors");
        const canvas = page.locator(".graph-canvas-wrap canvas");
        const bounds = await canvas.boundingBox();
        const hub = restored.nodes[0];
        await page.mouse.move(bounds.x + hub.x, bounds.y + hub.y);
        await page.mouse.down();
        await page.mouse.move(bounds.x + hub.x + 30, bounds.y + hub.y + 10, { steps: 4 });
        await page.mouse.up();
        const dragged = (await snapshot(page)).nodes[0];
        assert.ok(Math.abs(dragged.x - hub.x - 30) < 1);
        assert.deepEqual(await page.evaluate(() => window.__openedGraphPages), [], "drag must not navigate");
        await page.mouse.click(bounds.x + dragged.x, bounds.y + dragged.y);
        await page.waitForFunction(() => window.__openedGraphPages.includes("Astronomy"));
      }
      assert.deepEqual(errors, []);
      console.log(`PASS 2D communities, true bridges, fit and readable labels: ${theme} ${mobile ? "mobile" : "desktop"}`);
      await page.close();
    }
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
