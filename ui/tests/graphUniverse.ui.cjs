// Standalone WebGL test: a synthetic page, no application startup or note data.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");

const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

(async () => {
  const browser = await chromium.launch({
    args: ["--no-sandbox", "--enable-unsafe-swiftshader", "--use-angle=swiftshader"],
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 700 } });
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error") errors.push(message.text());
    });
    const origin = new URL(BASE_URL).origin;
    await page.route(`${origin}/universe-helper-test.html`, (route) => route.fulfill({
      contentType: "text/html",
      body: "<!doctype html><html><body style='margin:0'></body></html>",
    }));
    await page.goto(`${origin}/universe-helper-test.html`);
    const result = await page.evaluate(async () => {
      const moduleSource = await (await fetch("/src/lib/graphUniverse.ts")).text();
      const threeUrl = moduleSource.match(/from\s+["']([^"']*\/three[^"']*)["']/)?.[1];
      if (!threeUrl) throw new Error("Cannot locate Vite's Three.js module");
      const THREE = await import(threeUrl);
      const { createUniverseBackground } = await import("/src/lib/graphUniverse.ts");
      const renderer = new THREE.WebGLRenderer({ antialias: false, preserveDrawingBuffer: true });
      renderer.setSize(1000, 700);
      renderer.setPixelRatio(1);
      document.body.append(renderer.domElement);
      const scene = new THREE.Scene();
      const camera = new THREE.PerspectiveCamera(50, 1000 / 700, 0.1, 10000);
      const universe = createUniverseBackground();
      scene.add(universe.object);
      await universe.ready;
      const render = (flying = false, light = false) => {
        scene.background = new THREE.Color(light && !flying ? "#fafafa" : flying ? "#050711" : "#16161e");
        universe.setAppearance({ flying, isLightTheme: light });
        universe.update(camera, renderer.getPixelRatio());
        renderer.render(scene, camera);
        const gl = renderer.getContext();
        const data = new Uint8Array(gl.drawingBufferWidth * gl.drawingBufferHeight * 4);
        gl.readPixels(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight, gl.RGBA, gl.UNSIGNED_BYTE, data);
        return data;
      };
      const changed = (a, b) => {
        let count = 0;
        for (let i = 0; i < a.length; i += 4) {
          if (a[i] !== b[i] || a[i + 1] !== b[i + 1] || a[i + 2] !== b[i + 2]) count++;
        }
        return count;
      };
      const blankFor = (flying, light) => {
        universe.object.visible = false;
        const data = render(flying, light);
        universe.object.visible = true;
        return data;
      };
      const normal = render();
      const normalPixels = changed(normal, blankFor(false, false));
      const flight = render(true);
      const flightPixels = changed(flight, blankFor(true, false));
      const light = render(false, true);
      const lightPixels = changed(light, blankFor(false, true));
      const lightCenter = Array.from(light.slice((350 * 1000 + 500) * 4, (350 * 1000 + 500) * 4 + 3));
      const lightAlphaIntact = light.every((channel, i) => i % 4 !== 3 || channel === 255);
      const lightInkOnly = light.every((channel, i) => i % 4 === 3 || channel <= 250);
      const flightIgnoresLightTheme = changed(flight, render(true, true)) === 0;
      const stable = changed(normal, render()) === 0;

      const galaxies = universe.object.children.filter((child) => child.name.startsWith("grafium-universe-galaxy-"));
      galaxies.forEach((galaxy) => { galaxy.visible = false; });
      const starOnly = render(true);
      galaxies.forEach((galaxy) => { galaxy.visible = true; });
      const galaxyPixels = changed(starOnly, render(true));

      const target = galaxies[0].getWorldPosition(new THREE.Vector3()).normalize().multiplyScalar(200);
      const node = new THREE.Mesh(
        new THREE.SphereGeometry(18, 24, 16),
        new THREE.MeshBasicMaterial({ color: "#e03010" }),
      );
      node.position.copy(target);
      scene.add(node);
      const withSky = render(true);
      const withoutSky = blankFor(true, false);
      let coveredPixels = 0;
      let contaminatedPixels = 0;
      for (let i = 0; i < withSky.length; i += 4) {
        if (withoutSky[i] > 150 && withoutSky[i + 1] < 100 && withoutSky[i + 2] < 80) {
          coveredPixels++;
          if (withSky[i] !== withoutSky[i] || withSky[i + 1] !== withoutSky[i + 1] || withSky[i + 2] !== withoutSky[i + 2]) {
            contaminatedPixels++;
          }
        }
      }
      node.removeFromParent();
      node.geometry.dispose();
      node.material.dispose();
      camera.position.set(1e8, -2e8, 3e8);
      camera.far = 1e7;
      camera.updateProjectionMatrix();
      const traveled = render(true);
      const translationChanges = changed(flight, traveled);
      camera.position.set(0, 0, 0);
      camera.far = 10000;
      camera.updateProjectionMatrix();
      renderer.setSize(390, 700);
      renderer.setPixelRatio(2);
      camera.aspect = 390 / 700;
      camera.updateProjectionMatrix();
      const mobile = render();
      const mobilePixels = changed(mobile, blankFor(false, false));
      render();
      const geometriesBefore = renderer.info.memory.geometries;
      const texturesBefore = renderer.info.memory.textures;
      universe.dispose();
      renderer.render(scene, camera);
      const geometriesAfter = renderer.info.memory.geometries;
      const texturesAfter = renderer.info.memory.textures;
      renderer.dispose();
      return {
        normalPixels, flightPixels, lightPixels, lightCenter, lightAlphaIntact, lightInkOnly, flightIgnoresLightTheme,
        stable, galaxyPixels, coveredPixels, contaminatedPixels, translationChanges,
        mobilePixels, geometriesBefore, texturesBefore, geometriesAfter, texturesAfter,
      };
    });
    assert.deepEqual(errors, [], "WebGL must compile cleanly and local images must load");
    assert(result.normalPixels > 1000, "normal graph must visibly contain a universe background");
    assert(result.flightPixels > result.normalPixels, "flight must have larger, brighter stars and galaxies");
    assert(result.lightPixels > 1000, "light mode must retain a subtle universe background");
    assert(result.lightCenter.every((channel) => channel >= 245), "light mode must preserve its light canvas");
    assert(result.lightAlphaIntact, "background blending must not punch holes in the canvas alpha");
    assert(result.lightInkOnly, "light-mode stars and galaxies must use dark ink, not white light");
    assert(result.flightIgnoresLightTheme, "flight stays dark regardless of app theme");
    assert(result.stable, "stopping/mode switching must not jitter the deterministic sky");
    assert(result.galaxyPixels > 1000, "real galaxy textures must actually contribute visible pixels");
    assert(result.coveredPixels > 1000, "occlusion assertion must cover a foreground graph node");
    assert.equal(result.contaminatedPixels, 0, "sky must draw behind opaque graph nodes");
    assert(result.translationChanges < 30, "long flights/far-plane changes must not move the infinitely distant sky");
    assert(result.mobilePixels > 1000, "high-DPR narrow view must still render the universe");
    assert(result.geometriesBefore >= 2 && result.texturesBefore === 2, "actual GPU assets must load");
    assert.equal(result.geometriesAfter, 0, "all GPU geometry must be released");
    assert.equal(result.texturesAfter, 0, "all GPU textures must be released");
    console.log("Universe WebGL passed:", JSON.stringify(result));
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
