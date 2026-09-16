// Inspect the real WebGL graph through a test-only module wrapper, never a
// production debug hook. All IPC returns synthetic pages; no notes are opened.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");

const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";
const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y, (a.z ?? 0) - (b.z ?? 0));
const mean = (values) => values.reduce((sum, value) => sum + value, 0) / values.length;

function fixture() {
  const nodes = [];
  const edges = [];
  for (const [group, title] of ["Biology", "Music", "Compilers"].entries()) {
    nodes.push({ id: `hub-${group}`, title, degree: 0 });
    for (let leaf = 0; leaf < 12; leaf++) {
      nodes.push({
        id: `leaf-${group}-${leaf}`,
        title: group === 1 && leaf === 0 ? "2026-09-13" : `${title}/Topic ${leaf + 1}`,
        degree: 0,
      });
      edges.push({ source: `hub-${group}`, target: `leaf-${group}-${leaf}`, weight: 3 });
      edges.push({ source: `leaf-${group}-${leaf}`, target: `leaf-${group}-${(leaf + 1) % 12}`, weight: 1 });
    }
  }
  edges.push({ source: "hub-0", target: "hub-1", weight: 1 });
  edges.push({ source: "hub-1", target: "hub-2", weight: 1 });
  nodes.push({ id: "isolate-a", title: "2026_09_12", degree: 0 });
  nodes.push({ id: "isolate-b", title: "Another unlinked thought", degree: 0 });
  for (let i = 0; i < 30; i++) nodes.push({ id: `isolate-${i}`, title: `Unlinked thought ${i + 1}`, degree: 0 });
  for (const node of nodes) node.degree = edges.filter((edge) => edge.source === node.id || edge.target === node.id).length;
  return { nodes, edges };
}

(async () => {
  const data = fixture();
  const browser = await chromium.launch({
    args: ["--no-sandbox", "--enable-unsafe-swiftshader", "--use-angle=swiftshader"],
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const errors = [];
    const shaderErrors = [];
    const planetRequests = [];
    const galaxyRequests = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error" && /THREE\.WebGLProgram|Shader Error|VALIDATE_STATUS/.test(message.text())) {
        shaderErrors.push(message.text());
      }
    });
    page.on("request", (request) => {
      if (/\/planets\/.*\.png(?:\?|$)/i.test(request.url())) planetRequests.push(request.url());
      if (/\/space\/.*\.(?:webp|png|jpe?g)(?:\?|$)/i.test(request.url())) galaxyRequests.push(request.url());
    });
    await page.route("**/node_modules/.vite/deps/3d-force-graph.js*", async (route) => {
      const response = await route.fetch();
      const source = await response.text();
      const instrumented = source.replace(/export\s*\{\s*(\w+)\s+as\s+default\s*\};/, (_, constructor) => `
        const CapturedForceGraph = new Proxy(${constructor}, {
          construct(target, args) {
            const graph = Reflect.construct(target, args);
            window.__communityGraph = graph;
            window.__communityTicks = 0;
            graph.onEngineTick(() => { window.__communityTicks++; });
            return graph;
          }
        });
        export { CapturedForceGraph as default };
      `);
      assert.notEqual(instrumented, source, "instrument the actual 3d-force-graph constructor");
      await route.fulfill({ response, body: instrumented });
    });
    await page.addInitScript((data) => {
      window.__syntheticCommunityData = data;
      window.__communityRequests = [];
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        plugins: {},
        transformCallback() { return 1; },
        unregisterCallback() {},
        invoke: async (cmd, args = {}) => {
          switch (cmd) {
            case "get_page": throw new Error("Page not found");
            case "get_app_theme": return "tokyo-night";
            case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
            case "get_graph_info": return { name: "Community test", path: "/synthetic/community-test-graph" };
            case "get_graph_data":
              window.__communityRequests.push(structuredClone(args));
              return structuredClone(window.__syntheticCommunityData);
            case "plugin:event|listen": return 1;
            default: return [];
          }
        },
      };
    }, data);
    await page.goto(BASE_URL, { waitUntil: "networkidle" });
    await page.getByRole("button", { name: "Graph View", exact: true }).first().click();
    await page.locator(".graph-view .graph-stats").filter({ hasText: `${data.nodes.length} nodes` }).waitFor();
    assert.match(await page.locator(".graph-view .graph-stats").textContent(),
      new RegExp(`${data.nodes.length} nodes.*${data.edges.length} links`, "s"));
    const twoDimensionalRequest = await page.evaluate(() => window.__communityRequests.at(-1));
    await page.getByRole("button", { name: "3D", exact: true }).click();
    await page.waitForFunction((count) => window.__communityGraph?.graphData().nodes.length === count, data.nodes.length);
    assert.deepEqual(await page.evaluate(() => window.__communityRequests.at(-1)), twoDimensionalRequest,
      "both views must request the same graph scope, node limit and accepted links");
    await page.waitForFunction(() => window.__communityTicks >= 240, null, { timeout: 30000 });
    await page.waitForTimeout(800);
    await page.waitForFunction(() => {
      const background = window.__communityGraph.scene().getObjectByName("grafium-universe");
      if (!background) return false;
      const galaxies = [];
      background.traverse((object) => {
        if (object.name.startsWith("grafium-universe-galaxy-") && object.material) galaxies.push(object);
      });
      return galaxies.length >= 3 && galaxies.every((galaxy) => {
        const material = galaxy.material;
        const textures = [material.map, ...Object.values(material.uniforms ?? {}).map((uniform) => uniform.value)];
        return textures.some((texture) => texture?.isTexture && texture.image?.width > 0);
      });
    });

    const checkBackground = async () => {
      const result = await page.evaluate(() => {
        const graph = window.__communityGraph;
        const renderer = graph.renderer();
        const scene = graph.scene();
        const camera = graph.camera();
        const background = scene.getObjectByName("grafium-universe");
        const gl = renderer.getContext();
        const width = gl.drawingBufferWidth;
        const height = gl.drawingBufferHeight;
        const read = () => {
          renderer.render(scene, camera);
          const pixels = new Uint8Array(width * height * 4);
          gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
          return pixels;
        };
        background.visible = false;
        const plain = read();
        background.visible = true;
        const decorated = read();
        let changed = 0;
        for (let i = 0; i < plain.length; i += 4) {
          if (plain[i] !== decorated[i] || plain[i + 1] !== decorated[i + 1] || plain[i + 2] !== decorated[i + 2]) changed++;
        }
        const hubs = graph.graphData().nodes.filter((node) => node.id.startsWith("hub-"));
        const covered = hubs.filter((node) => {
          const point = graph.graph2ScreenCoords(node.x, node.y, node.z);
          const x = Math.floor(point.x * renderer.getPixelRatio());
          const y = height - 1 - Math.floor(point.y * renderer.getPixelRatio());
          if (x < 0 || x >= width || y < 0 || y >= height) return false;
          const offset = (y * width + x) * 4;
          return [0, 1, 2].some((channel) => Math.abs(plain[offset + channel] - decorated[offset + channel]) > 1);
        });
        return {
          changed,
          covered: covered.map((node) => node.id),
          stars: scene.getObjectByName("grafium-universe-stars").geometry.getAttribute("position").count,
        };
      });
      assert.ok(result.changed > 500, "the decorative background must actually render visible pixels");
      assert.ok(result.stars >= 1000, "the starfield is a batched, populated background");
      assert.deepEqual(result.covered, [], "background stars and galaxies must render behind opaque graph nodes");
    };

    const snapshot = () => page.evaluate(() => {
      const graph = window.__communityGraph;
      const nodes = graph.graphData().nodes.map((node) => ({
        id: node.id, x: node.x, y: node.y, z: node.z,
        screen: graph.graph2ScreenCoords(node.x, node.y, node.z),
        color: node.__threeObj.material.color.getHexString(),
        map: node.__threeObj.material.map !== null,
        opacity: node.__threeObj.material.opacity,
        wireframe: node.__threeObj.material.wireframe,
        geometry: node.__threeObj.geometry.type,
        radius: node.__threeObj.geometry.parameters.radius,
        screenRadius: Math.abs(graph.graph2ScreenCoords(
          node.x + node.__threeObj.geometry.parameters.radius, node.y, node.z,
        ).x - graph.graph2ScreenCoords(node.x, node.y, node.z).x),
      }));
      const links = graph.graphData().links.map((link) => ({
        source: typeof link.source === "string" ? link.source : link.source.id,
        target: typeof link.target === "string" ? link.target : link.target.id,
        weight: link.weight,
      }));
      return { nodes, links, width: graph.width(), height: graph.height(), ticks: window.__communityTicks };
    });
    const overview = await snapshot();
    await checkBackground();
    assert.ok(new Set(galaxyRequests).size >= 2, "use multiple bundled galaxy images");
    assert.ok(galaxyRequests.every((url) => new URL(url).origin === new URL(BASE_URL).origin),
      "galaxy images must not require an external service");
    assert.deepEqual(overview.nodes.map((node) => node.id).sort(), data.nodes.map((node) => node.id).sort(),
      "3D must include the same nodes as 2D, including linked and isolated date pages");
    assert.deepEqual(overview.links, data.edges, "3D must retain every actual connection, including journal links");
    const suggestionStrength = await page.evaluate(() =>
      window.__communityGraph.d3Force("link").strength()({
        source: "hub-0", target: "hub-1", suggested: true, weight: 100,
      }));
    assert.equal(suggestionStrength, 0, "suggested links must exert no spring force");
    assert.equal(await page.locator(".graph-view-3d").getAttribute("data-communities"), "3");
    const byId = new Map(overview.nodes.map((node) => [node.id, node]));
    assert.ok(overview.ticks >= 240, "check the actual cooled d3 simulation, not only initial anchors");
    for (const node of overview.nodes) {
      assert.ok([node.x, node.y, node.z].every(Number.isFinite), `${node.id} must have finite coordinates`);
      assert.equal(node.map, false, "spheres must not have image maps");
      assert.equal(node.wireframe, false, "spheres must be filled");
      assert.equal(node.opacity, 1, "overview spheres must be solid");
      assert.equal(node.geometry, "SphereGeometry");
      assert.ok(node.screenRadius >= 2, `${node.id} must remain a visible filled sphere in the fitted overview`);
      assert.ok(node.screen.x > 0 && node.screen.x < overview.width, `${node.id} fits horizontally`);
      assert.ok(node.screen.y > 0 && node.screen.y < overview.height, `${node.id} fits vertically`);
    }
    const groups = [0, 1, 2].map((group) => {
      const members = overview.nodes.filter((node) => node.id === `hub-${group}` || node.id.startsWith(`leaf-${group}-`));
      const center = {
        x: mean(members.map((node) => node.x)),
        y: mean(members.map((node) => node.y)),
        z: mean(members.map((node) => node.z)),
      };
      const screen = { x: mean(members.map((node) => node.screen.x)), y: mean(members.map((node) => node.screen.y)) };
      assert.equal(new Set(members.map((node) => node.color)).size, 1, "each real topic community shares one color");
      return {
        center, screen,
        radius: Math.max(...members.map((node) => distance(node, center))),
        screenRadius: Math.max(...members.map((node) => distance({ x: node.screen.x, y: node.screen.y }, screen))),
        color: members[0].color,
      };
    });
    assert.equal(new Set(groups.map((group) => group.color)).size, 3, "sparse bridges must not merge all topics into one color");
    assert.equal(byId.get("isolate-a").color, byId.get("isolate-b").color, "isolates use the same neutral color");
    assert.ok(groups.every((group) => group.color !== byId.get("isolate-a").color));
    const isolated = overview.nodes.filter((node) => node.id.startsWith("isolate-"));
    const sphereRadius = mean(isolated.map((node) => Math.hypot(node.x, node.y, node.z)));
    for (const node of isolated) {
      assert.ok(Math.abs(Math.hypot(node.x, node.y, node.z) - sphereRadius) < sphereRadius * 0.02,
        "disconnected nodes stay on the spherical surface after simulation");
    }
    for (const axis of ["x", "y", "z"]) {
      const values = isolated.map((node) => node[axis] / sphereRadius);
      assert.ok(Math.max(...values) - Math.min(...values) > 1.7, `the sphere must span the ${axis} axis`);
      assert.ok(Math.abs(mean(values.map((value) => value * value)) - 1 / 3) < 0.06,
        "isolates must form a full sphere, not a rotated plane");
    }
    for (let a = 0; a < groups.length; a++) {
      for (let b = a + 1; b < groups.length; b++) {
        assert.ok(distance(groups[a].center, groups[b].center) > (groups[a].radius + groups[b].radius) * 1.35,
          "community balls remain separate after force settling");
        assert.ok(distance(groups[a].screen, groups[b].screen) > (groups[a].screenRadius + groups[b].screenRadius) * 1.1,
          "community balls must be visibly separate in the overview projection");
      }
    }
    const lengths = fixture().edges.map((edge) => ({
      bridge: edge.source.startsWith("hub-") && edge.target.startsWith("hub-"),
      length: distance(byId.get(edge.source), byId.get(edge.target)),
    }));
    assert.ok(mean(lengths.filter((edge) => edge.bridge).map((edge) => edge.length))
      > mean(lengths.filter((edge) => !edge.bridge).map((edge) => edge.length)) * 3,
    "bridge links should be visibly longer than links inside communities");
    for (let a = 0; a < overview.nodes.length; a++) {
      for (let b = a + 1; b < overview.nodes.length; b++) {
        assert.ok(distance(overview.nodes[a], overview.nodes[b]) > overview.nodes[a].radius + overview.nodes[b].radius,
          "solid node spheres must not overlap");
      }
    }
    if (process.env.GRAPH_COMMUNITIES_3D_SCREENSHOT) {
      await page.locator(".graph-view-3d").screenshot({ path: process.env.GRAPH_COMMUNITIES_3D_SCREENSHOT });
    }

    // A theme mutation must change actual Three materials, not only an unused accessor.
    await page.evaluate(async () => {
      const { applyTheme, getThemeById } = await import("/src/lib/themes.ts");
      applyTheme(getThemeById("github").colors);
    });
    await page.waitForFunction((color) => window.__communityGraph.graphData().nodes[0].__threeObj.material.color.getHexString() !== color,
      overview.nodes[0].color);
    const light = await snapshot();
    assert.notEqual(light.nodes[0].color, overview.nodes[0].color);
    await checkBackground();
    if (process.env.GRAPH_UNIVERSE_LIGHT_SCREENSHOT) {
      await page.locator(".graph-view-3d").screenshot({ path: process.env.GRAPH_UNIVERSE_LIGHT_SCREENSHOT });
    }
    await page.evaluate(async () => {
      const { applyTheme, getThemeById } = await import("/src/lib/themes.ts");
      applyTheme(getThemeById("tokyo-night").colors);
    });
    await page.waitForFunction((color) => window.__communityGraph.graphData().nodes[0].__threeObj.material.color.getHexString() === color,
      overview.nodes[0].color);

    await page.getByRole("button", { name: "Show graph settings", exact: true }).click();
    assert.equal(await page.getByRole("checkbox", { name: "Hide date pages", exact: true }).isChecked(), false);
    await page.locator(".graph-stats").filter({ hasText: "3 communities" }).waitFor();
    await page.getByPlaceholder("Fly to nodes…").fill("Biology");
    await page.waitForFunction(() => {
      const nodes = window.__communityGraph.graphData().nodes;
      const color = (id) => nodes.find((node) => node.id === id).__threeObj.material.color.getHexString();
      return color("hub-0") !== color("hub-1") && color("hub-1") === color("hub-2");
    });
    await page.locator(".graph-search-glow").first().waitFor();
    const beforeFlight = await snapshot();
    await page.getByRole("button", { name: "Space flight", exact: true }).click();
    await page.locator(".flight-hud").waitFor();
    assert.equal(await page.locator(".flight-hud").getAttribute("data-topic"), "hub-0");
    const flight = await snapshot();
    assert.ok(flight.nodes.every((node) => !node.map && !node.wireframe && node.opacity === 1));
    assert.ok(flight.nodes.some((node) => distance(node, byId.get(node.id)) > 1), "flight uses its separate hierarchy layout");
    await page.waitForTimeout(900);
    await checkBackground();
    if (process.env.GRAPH_UNIVERSE_FLIGHT_SCREENSHOT) {
      await page.locator(".graph-view-3d").screenshot({ path: process.env.GRAPH_UNIVERSE_FLIGHT_SCREENSHOT });
    }
    await page.getByRole("button", { name: "Stop flight", exact: true }).click();
    await page.locator(".flight-hud").waitFor({ state: "detached" });
    await page.waitForTimeout(400);
    const restored = await snapshot();
    const original = new Map(beforeFlight.nodes.map((node) => [node.id, node]));
    for (const node of restored.nodes) {
      assert.ok(distance(node, original.get(node.id)) < 0.001, "stopping flight restores the exact community layout");
      assert.equal(node.color, original.get(node.id).color, "stopping flight restores overview search colors");
      assert.equal(node.map, false);
    }
    const controlsRestored = await page.evaluate(() => ({
      controls: window.__communityGraph.controls().enabled,
      drag: window.__communityGraph.enableNodeDrag(),
      pointer: window.__communityGraph.enablePointerInteraction(),
    }));
    assert.deepEqual(controlsRestored, { controls: true, drag: true, pointer: true });
    await page.getByPlaceholder("Fly to nodes…").fill("");
    await page.locator(".graph-controls .controls-close").click();
    await page.getByRole("button", { name: "Reset view", exact: true }).click();
    await page.waitForTimeout(800);
    await checkBackground();
    assert.deepEqual(planetRequests, [], "overview and flight must not load planet PNGs");
    assert.deepEqual(errors, []);
    assert.deepEqual(shaderErrors, []);
    await page.evaluate(() => {
      const background = window.__communityGraph.scene().getObjectByName("grafium-universe");
      window.__universeBeforeLeave = background;
      window.__universeDisposals = 0;
      const resources = new Set();
      background.traverse((object) => {
        if (object.geometry) resources.add(object.geometry);
        if (object.material) {
          resources.add(object.material);
          if (object.material.map) resources.add(object.material.map);
          for (const uniform of Object.values(object.material.uniforms ?? {})) {
            if (uniform.value?.isTexture) resources.add(uniform.value);
          }
        }
      });
      for (const resource of resources) resource.addEventListener("dispose", () => window.__universeDisposals++);
      window.__universeResourceCount = resources.size;
    });
    await page.getByRole("button", { name: "2D", exact: true }).click();
    await page.locator(".graph-view").waitFor();
    assert.deepEqual(await page.evaluate(() => ({
      removed: window.__universeBeforeLeave.parent === null,
      disposed: window.__universeDisposals === window.__universeResourceCount,
    })), { removed: true, disposed: true }, "leaving 3D must release the entire background");
    console.log("PASS matching 2D/3D dates and topology, spherical isolates, communities, solid spheres, offline universe in both modes, foreground occlusion, theme/search, flight restoration and GPU cleanup");
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
