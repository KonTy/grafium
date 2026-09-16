// Real Three.js/WebGL camera and controls; IPC uses only a synthetic graph.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");

const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

(async () => {
  const browser = await chromium.launch({
    args: ["--no-sandbox", "--enable-unsafe-swiftshader", "--use-angle=swiftshader"],
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const errors = [];
    const planetRequests = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("request", (request) => {
      if (/\/planets\/.*\.png(?:\?|$)/i.test(request.url())) planetRequests.push(request.url());
    });
    await page.addInitScript(() => {
      window.__flightGraph = {
        nodes: [
          { id: "rust", title: "Rust", degree: 3 },
          { id: "wasm", title: "WebAssembly", degree: 2 },
          { id: "js", title: "JavaScript", degree: 2 },
          { id: "types", title: "Type systems", degree: 1 },
          { id: "island", title: "Unlinked island", degree: 0 },
        ],
        edges: [
          { source: "rust", target: "wasm", weight: 2 },
          { source: "rust", target: "types", weight: 1 },
          { source: "rust", target: "js", weight: 1 },
          { source: "js", target: "wasm", weight: 1 },
        ],
      };
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        plugins: {},
        transformCallback() { return 1; },
        unregisterCallback() {},
        invoke: async (cmd) => {
          switch (cmd) {
            case "get_page": throw new Error("Page not found");
            case "get_app_theme": return "tokyo-night";
            case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
            case "get_graph_info": return { name: "Flight test", path: "/synthetic/flight-test-graph" };
            case "get_graph_data": return structuredClone(window.__flightGraph);
            case "plugin:event|listen": return 1;
            default: return [];
          }
        },
      };
    });
    await page.goto(BASE_URL, { waitUntil: "networkidle" });
    const openGraph = async () => {
      await page.getByRole("button", { name: "Graph View", exact: true }).first().click();
      await page.getByRole("button", { name: "3D", exact: true }).click();
      await page.waitForFunction(() => {
        const button = document.querySelector(".flight-fab");
        return button && !button.disabled;
      });
    };
    await openGraph();
    const start = page.getByRole("button", { name: "Space flight", exact: true });
    const stop = page.getByRole("button", { name: "Stop flight", exact: true });
    const canvas = page.locator(".graph-view-3d .graph-canvas canvas");
    const hud = page.locator(".flight-hud");
    await page.getByRole("button", { name: "Show graph settings", exact: true }).click();
    await page.getByPlaceholder("Fly to nodes…").fill("WebAssembly");
    await page.getByRole("checkbox", { name: "Show labels", exact: true }).uncheck();
    await start.click();
    assert.equal(await stop.getAttribute("aria-pressed"), "true");
    assert.equal(await hud.getAttribute("data-topic"), "wasm");
    const destinationColors = [await hud.evaluate((element) => element.style.getPropertyValue("--planet-color"))];
    const firstFrame = await canvas.screenshot();
    await page.waitForTimeout(900);
    const nextFrame = await canvas.screenshot();
    assert.notDeepEqual(nextFrame, firstFrame, "flight must actually move the rendered camera");

    let previous = await hud.getAttribute("data-topic");
    for (let visit = 2; visit <= 3; visit++) {
      await page.waitForFunction((n) => Number(document.querySelector(".flight-hud")?.dataset.stop) >= n, visit, { timeout: 25000 });
      const topic = await hud.getAttribute("data-topic");
      const related = await page.evaluate(([from, to]) => window.__flightGraph.edges.some(
        (edge) => (edge.source === from && edge.target === to) || (edge.target === from && edge.source === to),
      ), [previous, topic]);
      assert.ok(related, `${previous} -> ${topic} must follow a real link`);
      destinationColors.push(await hud.evaluate((element) => element.style.getPropertyValue("--planet-color")));
      previous = topic;
    }
    assert.equal(new Set(destinationColors).size, 3, "different topics should have distinct destination colors");
    await page.locator(".flight-eyebrow").filter({ hasText: "Flying by" }).waitFor();
    await page.locator(".graph-label.destination").waitFor();
    if (process.env.GRAPH_FLIGHT_SCREENSHOT) {
      await page.locator(".graph-view-3d").screenshot({ path: process.env.GRAPH_FLIGHT_SCREENSHOT });
    }
    await stop.click();
    await hud.waitFor({ state: "detached" });
    assert.equal(await start.getAttribute("aria-pressed"), "false");
    await page.waitForTimeout(700);
    const parked = await canvas.screenshot();
    await page.waitForTimeout(1200);
    assert.deepEqual(await canvas.screenshot(), parked, "stopping must leave the camera parked");

    // Manual navigation takes over after stopping, rather than fighting a timer.
    const box = await canvas.boundingBox();
    await page.mouse.move(box.x + box.width * 0.3, box.y + box.height * 0.7);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.45, box.y + box.height * 0.7, { steps: 8 });
    await page.mouse.up();
    await page.mouse.move(1400, 950);
    await page.waitForTimeout(400);
    assert.notDeepEqual(await canvas.screenshot(), parked);
    await start.click();
    await page.keyboard.press("Escape");
    await hud.waitFor({ state: "detached" });

    for (let i = 0; i < 3; i++) {
      await start.click();
      await stop.click();
    }
    await start.click();
    await page.locator(".graph-controls .controls-close").click();
    await page.getByRole("button", { name: "Reset view", exact: true }).click();
    await hud.waitFor({ state: "detached" });
    await start.click();
    await page.getByRole("button", { name: "2D", exact: true }).click();
    await page.getByRole("button", { name: "3D", exact: true }).click();
    await start.waitFor();
    assert.equal(await hud.count(), 0);
    await page.getByRole("button", { name: "Show graph settings", exact: true }).click();

    await page.evaluate(() => { window.__flightGraph.edges = []; });
    await page.getByRole("checkbox", { name: "Hide date pages", exact: true }).check();
    await page.waitForFunction(() => document.querySelector(".flight-fab")?.disabled);
    await page.getByText("At least two linked, visible topics are needed.", { exact: false }).waitFor();

    // Hierarchy alone is enough to create a display-only planet system.
    await page.evaluate(() => {
      window.__flightGraph = {
        nodes: [
          { id: "supplements", title: "Health/Supplements", degree: 3 },
          { id: "creatine", title: "Health/Supplements/Creatine", degree: 1 },
          { id: "vitamins", title: "Health/Supplements/Vitamins", degree: 1 },
          { id: "d3", title: "Health/Supplements/Vitamins/D3", degree: 1 },
        ],
        edges: [],
      };
    });
    await page.getByRole("checkbox", { name: "Hide date pages", exact: true }).uncheck();
    await page.waitForFunction(() => !document.querySelector(".flight-fab")?.disabled);
    await page.getByPlaceholder("Fly to nodes…").fill("Health/Supplements");
    await page.getByRole("checkbox", { name: "Show labels", exact: true }).check();
    await start.click();
    assert.equal(await hud.getAttribute("data-topic"), "supplements");
    assert.equal(await page.locator(".graph-view-3d").getAttribute("data-satellites"), "3");
    await hud.getByText("2 child topics in this planet system", { exact: true }).waitFor();
    await page.locator(".flight-eyebrow").filter({ hasText: "Flying by" }).waitFor();
    await page.locator(".graph-label").filter({ hasText: /^Creatine$/ }).waitFor();
    if (process.env.GRAPH_SATELLITES_SCREENSHOT) {
      await page.locator(".graph-view-3d").screenshot({ path: process.env.GRAPH_SATELLITES_SCREENSHOT });
    }
    await page.waitForFunction(() => document.querySelector(".flight-hud")?.dataset.stop === "2", null, { timeout: 15000 });
    assert.equal(await hud.getAttribute("data-satellite"), "true");
    assert.equal(await hud.getAttribute("data-ringed"), "false");
    await hud.getByText("Satellite of Health/Supplements", { exact: true }).waitFor();
    await stop.click();
    assert.equal(await page.locator(".graph-view-3d").getAttribute("data-satellites"), "0");
    assert.deepEqual(await page.evaluate(() => window.__flightGraph.edges), []);
    await start.click();
    assert.equal(await hud.getAttribute("data-topic"), "supplements");
    await stop.click();
    assert.deepEqual(errors, []);
    assert.deepEqual(planetRequests, [], "solid spheres must not fetch planet images");
    console.log("PASS solid-sphere WebGL flight, related destinations, satellites, ringless moons, stop, manual controls and cleanup");
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
