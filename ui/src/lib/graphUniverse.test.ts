import { afterEach, describe, expect, it, vi } from "vitest";
import {
  BufferGeometry,
  CustomBlending,
  Mesh,
  OneFactor,
  OneMinusSrcAlphaFactor,
  PerspectiveCamera,
  Points,
  Raycaster,
  Scene,
  ShaderMaterial,
  SRGBColorSpace,
  Texture,
  TextureLoader,
  Vector3,
} from "three";
import { createUniverseBackground, createUniverseStarCatalog } from "./graphUniverse";

function mockTextureLoads() {
  const pending: {
    url: string;
    texture: Texture;
    resolve(): void;
    reject(): void;
  }[] = [];
  const load = vi.spyOn(TextureLoader.prototype, "load").mockImplementation((url, onLoad, _progress, onError) => {
    const texture = new Texture(document.createElement("img"));
    pending.push({
      url,
      texture,
      resolve: () => onLoad?.(texture),
      reject: () => onError?.(new Error("image unavailable")),
    });
    return texture;
  });
  return { pending, load };
}

function starPoints(universe: ReturnType<typeof createUniverseBackground>) {
  const stars = universe.object.getObjectByName("grafium-universe-stars");
  if (!(stars instanceof Points) || !(stars.material instanceof ShaderMaterial)) {
    throw new Error("Missing universe stars");
  }
  return stars as Points<BufferGeometry, ShaderMaterial>;
}

function galaxyMeshes(universe: ReturnType<typeof createUniverseBackground>) {
  return universe.object.children.filter((child): child is Mesh<BufferGeometry, ShaderMaterial> =>
    child instanceof Mesh && child.material instanceof ShaderMaterial);
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("universe star catalog", () => {
  it("is deterministic, finite and spread across a unit celestial sphere", () => {
    const first = createUniverseStarCatalog();
    expect(first).toEqual(createUniverseStarCatalog());
    expect(first.sizes.length).toBe(7200);
    for (const values of Object.values(first)) {
      expect(Array.from(values).every(Number.isFinite)).toBe(true);
    }
    let front = 0;
    let back = 0;
    for (let i = 0; i < first.positions.length; i += 3) {
      expect(Math.hypot(...first.positions.slice(i, i + 3))).toBeCloseTo(1, 6);
      if (first.positions[i + 2] < 0) front++;
      else back++;
    }
    expect(front).toBeGreaterThan(3200);
    expect(back).toBeGreaterThan(3200);
  });

  it("varies size, luminosity and warm/cool color independently", () => {
    const { sizes, brightness, colors } = createUniverseStarCatalog();
    expect(Math.min(...sizes)).toBeGreaterThanOrEqual(1.15);
    expect(Math.max(...sizes)).toBeLessThanOrEqual(7);
    expect(Array.from(sizes).filter((size) => size < 3).length).toBeGreaterThan(4800);
    const large = Array.from(sizes).filter((size) => size > 5).length;
    expect(large).toBeGreaterThan(50);
    expect(large).toBeLessThan(200);
    expect(Math.min(...brightness)).toBeLessThan(0.25);
    expect(Math.max(...brightness)).toBeGreaterThan(0.95);
    const largeBrightness = Array.from(brightness).filter((_, i) => sizes[i] > 5);
    expect(Math.min(...largeBrightness)).toBeLessThan(0.35);
    expect(Math.max(...largeBrightness)).toBeGreaterThan(0.9);
    const palette = new Set<string>();
    for (let i = 0; i < colors.length; i += 3) palette.add(colors.slice(i, i + 3).join(","));
    expect(palette.size).toBe(5);
    expect(colors.some((red, i) => i % 3 === 0 && red > colors[i + 2])).toBe(true);
    expect(colors.some((red, i) => i % 3 === 0 && red < colors[i + 2])).toBe(true);
  });
});

describe("universe background", () => {
  it("starts stars immediately, loads only local images, and resolves when all galaxies are ready", async () => {
    const { pending } = mockTextureLoads();
    const universe = createUniverseBackground();
    const galaxies = galaxyMeshes(universe);
    expect(universe.object.name).toBe("grafium-universe");
    expect(starPoints(universe).visible).toBe(true);
    expect(galaxies).toHaveLength(6);
    expect(galaxies.every((galaxy) => !galaxy.visible)).toBe(true);
    expect(pending.map((entry) => entry.url)).toEqual(["/space/whirlpool.webp", "/space/sombrero.webp"]);
    let ready = false;
    void universe.ready.then(() => { ready = true; });
    pending[0].resolve();
    await Promise.resolve();
    expect(ready).toBe(false);
    pending[1].resolve();
    await universe.ready;
    expect(galaxies.every((galaxy) => galaxy.visible)).toBe(true);
    expect(pending.every(({ texture }) => texture.colorSpace === SRGBColorSpace)).toBe(true);
    universe.dispose();
  });

  it("changes only uniforms for graph, light theme and flight without regenerating or reloading", async () => {
    const { pending, load } = mockTextureLoads();
    const universe = createUniverseBackground();
    pending.forEach((entry) => entry.resolve());
    await universe.ready;
    const stars = starPoints(universe);
    const galaxies = galaxyMeshes(universe);
    const buffers = Object.values(stars.geometry.attributes);
    const materials = galaxies.map((galaxy) => galaxy.material);
    const maps = galaxies.map((galaxy) => galaxy.material.uniforms.map.value);
    const graphOpacity = stars.material.uniforms.opacity.value;
    const galaxyOpacity = galaxies[0].material.uniforms.opacity.value;
    universe.setAppearance({ flying: true, isLightTheme: true });
    expect(stars.material.uniforms.sizeScale.value).toBe(1);
    expect(stars.material.uniforms.opacity.value).toBeGreaterThan(graphOpacity);
    expect(stars.material.uniforms.lightTheme.value).toBe(0);
    expect(galaxies[0].material.uniforms.opacity.value).toBeGreaterThan(galaxyOpacity);
    universe.setAppearance({ flying: false, isLightTheme: true });
    expect(stars.material.uniforms.lightTheme.value).toBe(1);
    expect(stars.material.uniforms.opacity.value).toBeLessThan(graphOpacity);
    expect(galaxies.every((galaxy) => galaxy.material.uniforms.lightTheme.value === 1)).toBe(true);
    expect(galaxies[0].material.uniforms.opacity.value).toBeLessThan(galaxyOpacity);
    expect(Math.max(...stars.geometry.getAttribute("starSize").array) * stars.material.uniforms.sizeScale.value)
      .toBeLessThan(3.1);
    universe.setAppearance({ flying: false, isLightTheme: false });
    expect(stars.material.uniforms.opacity.value).toBe(graphOpacity);
    expect(Object.values(stars.geometry.attributes)).toEqual(buffers);
    expect(galaxies.map((galaxy) => galaxy.material)).toEqual(materials);
    expect(galaxies.map((galaxy) => galaxy.material.uniforms.map.value)).toEqual(maps);
    expect(load).toHaveBeenCalledTimes(2);
    universe.dispose();
  });

  it("follows camera translation and far-plane changes but not rotation, with DPR-only point sizing", () => {
    mockTextureLoads();
    const universe = createUniverseBackground();
    const camera = new PerspectiveCamera(50, 1.6, 0.1, 10000);
    camera.position.set(6000, -12000, 88000);
    universe.update(camera, 2);
    expect(universe.object.position).toEqual(camera.position);
    expect(universe.object.scale.x).toBe(8000);
    expect(starPoints(universe).material.uniforms.pixelRatio.value).toBe(2);
    camera.far = 1e8;
    camera.position.set(-1e9, 3e8, 1e9);
    camera.lookAt(0, 0, 0);
    universe.update(camera, 1.5);
    expect(universe.object.position).toEqual(camera.position);
    expect(universe.object.scale.x).toBe(8e7);
    expect(universe.object.rotation.toArray()).toEqual([0, 0, 0, "XYZ"]);
    expect(starPoints(universe).material.uniforms.pixelRatio.value).toBe(1.5);
    universe.update(camera, Number.NaN);
    expect(starPoints(universe).material.uniforms.pixelRatio.value).toBe(1);
    universe.dispose();
  });

  it("places three galaxies off-center in the initial field and keeps every vertex inside the far sphere", () => {
    mockTextureLoads();
    const universe = createUniverseBackground();
    const camera = new PerspectiveCamera(50, 1.6, 0.1, 10000);
    universe.update(camera, 1);
    universe.object.updateMatrixWorld(true);
    const galaxies = galaxyMeshes(universe);
    const projected = galaxies.map((galaxy) => galaxy.getWorldPosition(new Vector3()).project(camera));
    expect(projected.filter((point) => Math.abs(point.x) < 1 && Math.abs(point.y) < 1 && point.z < 1 && point.z > -1))
      .toHaveLength(3);
    expect(projected.slice(0, 3).every((point) => Math.hypot(point.x, point.y) > 0.4)).toBe(true);
    galaxies.forEach((galaxy) => {
      const vertices = galaxy.geometry.getAttribute("position");
      for (let i = 0; i < vertices.count; i++) {
        const vertex = new Vector3().fromBufferAttribute(vertices, i).applyMatrix4(galaxy.matrixWorld);
        expect(vertex.distanceTo(camera.position)).toBeLessThan(camera.far * 0.9);
      }
    });
    universe.dispose();
  });

  it("uses the early opaque queue with alpha blending and cannot intercept picking", () => {
    mockTextureLoads();
    const universe = createUniverseBackground();
    const decorations = [starPoints(universe), ...galaxyMeshes(universe)];
    decorations.forEach((decoration) => {
      expect(decoration.renderOrder).toBeLessThan(0);
      expect(decoration.material.transparent).toBe(false);
      expect(decoration.material.blending).toBe(CustomBlending);
      expect(decoration.material.blendSrcAlpha).toBe(OneFactor);
      expect(decoration.material.blendDstAlpha).toBe(OneMinusSrcAlphaFactor);
      expect(decoration.material.depthTest).toBe(false);
      expect(decoration.material.depthWrite).toBe(false);
      expect(decoration.material.toneMapped).toBe(false);
    });
    expect(new Raycaster(new Vector3(), new Vector3(0, 0, -1)).intersectObject(universe.object, true)).toEqual([]);
    universe.dispose();
  });

  it("rejects a local image failure instead of reporting a successfully loaded universe", async () => {
    const { pending } = mockTextureLoads();
    const universe = createUniverseBackground();
    const failure = expect(universe.ready).rejects.toThrow("Could not load universe background image: /space/whirlpool.webp");
    pending[0].reject();
    pending[1].resolve();
    await failure;
    universe.dispose();
  });

  it.each([false, true])("disposes every resource exactly once, including pending textures (loaded=%s)", async (loaded) => {
    const { pending } = mockTextureLoads();
    const universe = createUniverseBackground();
    const scene = new Scene();
    scene.add(universe.object);
    const decorations = [starPoints(universe), ...galaxyMeshes(universe)];
    const resources = new Set([
      ...decorations.flatMap((decoration) => [decoration.geometry, decoration.material]),
      ...pending.map((entry) => entry.texture),
    ]);
    const disposals = Array.from(resources, (resource) => vi.spyOn(resource, "dispose"));
    if (loaded) {
      pending.forEach((entry) => entry.resolve());
      await universe.ready;
    }
    universe.dispose();
    universe.dispose();
    expect(scene.children).toEqual([]);
    expect(universe.object.children).toEqual([]);
    if (!loaded) {
      pending.forEach((entry) => entry.resolve());
      await universe.ready;
    }
    expect(universe.object.children).toEqual([]);
    disposals.forEach((dispose) => expect(dispose).toHaveBeenCalledTimes(1));
    const oldScale = universe.object.scale.clone();
    const oldOpacity = decorations[0].material.uniforms.opacity.value;
    universe.update(new PerspectiveCamera(), 3);
    universe.setAppearance({ flying: true, isLightTheme: false });
    expect(universe.object.scale).toEqual(oldScale);
    expect(decorations[0].material.uniforms.opacity.value).toBe(oldOpacity);
  });
});
