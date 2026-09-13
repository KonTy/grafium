import {
  BufferGeometry,
  Camera,
  Color,
  CustomBlending,
  Float32BufferAttribute,
  Group,
  Mesh,
  OneFactor,
  OneMinusSrcAlphaFactor,
  PlaneGeometry,
  Points,
  ShaderMaterial,
  SrcAlphaFactor,
  SRGBColorSpace,
  Texture,
  TextureLoader,
  Vector3,
} from "three";

export interface UniverseBackground {
  object: Group;
  ready: Promise<void>;
  update(camera: Camera, pixelRatio: number): void;
  setAppearance(appearance: { flying: boolean; isLightTheme: boolean }): void;
  dispose(): void;
}

export function createUniverseStarCatalog(count = 7200) {
  let seed = 0x6a09e667;
  const random = () => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed / 0x100000000;
  };
  const positions = new Float32Array(count * 3);
  const colors = new Float32Array(count * 3);
  const sizes = new Float32Array(count);
  const brightness = new Float32Array(count);
  const palette = [
    new Color("#d8e7ff"),
    new Color("#f2f4ff"),
    new Color("#fff0d5"),
    new Color("#bacef5"),
    new Color("#f2d1bb"),
  ];
  for (let i = 0; i < count; i++) {
    const z = random() * 2 - 1;
    const angle = random() * Math.PI * 2;
    const radius = Math.sqrt(1 - z * z);
    positions.set([radius * Math.cos(angle), radius * Math.sin(angle), z], i * 3);
    const size = random();
    sizes[i] = size > 0.982 ? 5.2 + random() * 1.8 : 1.15 + 3.5 * size ** 2;
    brightness[i] = 0.24 + 0.76 * random() ** 1.35;
    palette[Math.floor(random() * palette.length)].toArray(colors, i * 3);
  }
  return { positions, colors, sizes, brightness };
}

const starVertexShader = `
  attribute float starSize;
  attribute float brightness;
  uniform float pixelRatio;
  uniform float sizeScale;
  varying vec3 starColor;
  varying float starBrightness;
  void main() {
    starColor = color;
    starBrightness = brightness;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    gl_PointSize = starSize * sizeScale * pixelRatio;
  }
`;

const starFragmentShader = `
  uniform float opacity;
  uniform float lightTheme;
  varying vec3 starColor;
  varying float starBrightness;
  void main() {
    float radius = length(gl_PointCoord - vec2(0.5)) * 2.0;
    if (radius >= 1.0) discard;
    float coverage = 1.0 - smoothstep(0.25, 1.0, radius);
    vec3 ink = mix(starColor, vec3(0.10, 0.15, 0.24), lightTheme);
    gl_FragColor = vec4(ink, coverage * starBrightness * opacity);
    #include <colorspace_fragment>
  }
`;

const galaxyVertexShader = `
  varying vec2 galaxyUv;
  void main() {
    galaxyUv = uv;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

const galaxyFragmentShader = `
  uniform sampler2D map;
  uniform float opacity;
  uniform float lightTheme;
  varying vec2 galaxyUv;
  void main() {
    vec4 galaxy = texture2D(map, galaxyUv);
    if (galaxy.a < 0.002) discard;
    vec3 ink = mix(galaxy.rgb, vec3(0.17, 0.22, 0.31), lightTheme);
    gl_FragColor = vec4(ink, galaxy.a * opacity);
    #include <colorspace_fragment>
  }
`;

// Alpha blending in the opaque queue: transparent objects are always rendered
// after opaque note spheres, regardless of a negative renderOrder.
const backgroundMaterial = {
  transparent: false,
  blending: CustomBlending,
  blendSrc: SrcAlphaFactor,
  blendDst: OneMinusSrcAlphaFactor,
  blendSrcAlpha: OneFactor,
  blendDstAlpha: OneMinusSrcAlphaFactor,
  depthTest: false,
  depthWrite: false,
  toneMapped: false,
};

const galaxyAssets = [
  { file: "whirlpool.webp", aspect: 512 / 356 },
  { file: "sombrero.webp", aspect: 512 / 284 },
] as const;

const galaxyCatalog = [
  { asset: 0, direction: [-0.34, 0.22, -1], width: 0.17, roll: -0.42, strength: 1 },
  { asset: 1, direction: [0.42, -0.22, -1], width: 0.21, roll: 0.38, strength: 0.95 },
  { asset: 0, direction: [0.13, 0.37, -1], width: 0.095, roll: 1.7, strength: 0.75 },
  { asset: 1, direction: [1, 0.16, 0.28], width: 0.17, roll: -0.9, strength: 0.8 },
  { asset: 0, direction: [-0.94, -0.37, 0.42], width: 0.15, roll: 2.1, strength: 0.85 },
  { asset: 1, direction: [0.18, -0.28, 1], width: 0.22, roll: 1.25, strength: 1 },
] as const;

export function createUniverseBackground(): UniverseBackground {
  const object = new Group();
  object.name = "grafium-universe";
  object.renderOrder = -10000;
  const starCatalog = createUniverseStarCatalog();
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new Float32BufferAttribute(starCatalog.positions, 3));
  geometry.setAttribute("color", new Float32BufferAttribute(starCatalog.colors, 3));
  geometry.setAttribute("starSize", new Float32BufferAttribute(starCatalog.sizes, 1));
  geometry.setAttribute("brightness", new Float32BufferAttribute(starCatalog.brightness, 1));
  const starMaterial = new ShaderMaterial({
    ...backgroundMaterial,
    vertexColors: true,
    uniforms: {
      pixelRatio: { value: 1 },
      sizeScale: { value: 0.44 },
      opacity: { value: 0.53 },
      lightTheme: { value: 0 },
    },
    vertexShader: starVertexShader,
    fragmentShader: starFragmentShader,
  });
  const stars = new Points(geometry, starMaterial);
  stars.name = "grafium-universe-stars";
  stars.renderOrder = -9999;
  stars.frustumCulled = false;
  stars.raycast = () => {};
  object.add(stars);

  const galaxyGeometry = new PlaneGeometry(1, 1);
  const galaxies = galaxyCatalog.map((entry, i) => {
    const material = new ShaderMaterial({
      ...backgroundMaterial,
      uniforms: {
        map: { value: null },
        opacity: { value: 0.22 * entry.strength },
        lightTheme: { value: 0 },
      },
      vertexShader: galaxyVertexShader,
      fragmentShader: galaxyFragmentShader,
    });
    const mesh = new Mesh(galaxyGeometry, material);
    mesh.name = `grafium-universe-galaxy-${i}`;
    mesh.position.set(entry.direction[0], entry.direction[1], entry.direction[2]).normalize().multiplyScalar(0.94);
    mesh.lookAt(0, 0, 0);
    mesh.rotateZ(entry.roll);
    mesh.scale.set(entry.width, entry.width / galaxyAssets[entry.asset].aspect, 1);
    mesh.renderOrder = -10000;
    mesh.frustumCulled = false;
    mesh.raycast = () => {};
    mesh.visible = false;
    object.add(mesh);
    return mesh;
  });

  let disposed = false;
  const textures = new Set<Texture>();
  const retiredTextures = new WeakSet<Texture>();
  const retireTexture = (texture: Texture) => {
    if (retiredTextures.has(texture)) return;
    retiredTextures.add(texture);
    texture.dispose();
  };
  const loader = new TextureLoader();
  const ready = Promise.all(galaxyAssets.map((asset, assetIndex) =>
    new Promise<void>((resolve, reject) => {
      const url = `${import.meta.env.BASE_URL}space/${asset.file}`;
      const texture = loader.load(url, (loaded) => {
        if (disposed) {
          retireTexture(loaded);
          resolve();
          return;
        }
        loaded.colorSpace = SRGBColorSpace;
        galaxies.forEach((galaxy, i) => {
          if (galaxyCatalog[i].asset !== assetIndex) return;
          galaxy.material.uniforms.map.value = loaded;
          galaxy.visible = true;
        });
        resolve();
      }, undefined, () => {
        retireTexture(texture);
        reject(new Error(`Could not load universe background image: ${url}`));
      });
      textures.add(texture);
    }),
  )).then(() => {});

  const cameraPosition = new Vector3();
  return {
    object,
    ready,
    update(camera, pixelRatio) {
      if (disposed) return;
      camera.getWorldPosition(cameraPosition);
      object.position.copy(cameraPosition);
      // Tangent galaxy corners remain comfortably inside the far plane, even
      // when zooming changes it. Camera translation never brings the sky closer.
      const far = "far" in camera && typeof camera.far === "number" && Number.isFinite(camera.far)
        ? camera.far : 10000;
      object.scale.setScalar(Math.max(1, far * 0.8));
      starMaterial.uniforms.pixelRatio.value = Number.isFinite(pixelRatio) && pixelRatio > 0 ? pixelRatio : 1;
    },
    setAppearance({ flying, isLightTheme }) {
      if (disposed) return;
      const light = isLightTheme && !flying;
      starMaterial.uniforms.sizeScale.value = flying ? 1 : 0.44;
      starMaterial.uniforms.opacity.value = flying ? 0.96 : light ? 0.25 : 0.53;
      starMaterial.uniforms.lightTheme.value = light ? 1 : 0;
      galaxies.forEach((galaxy, i) => {
        galaxy.material.uniforms.opacity.value = (flying ? 0.62 : light ? 0.10 : 0.22) * galaxyCatalog[i].strength;
        galaxy.material.uniforms.lightTheme.value = light ? 1 : 0;
      });
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      object.removeFromParent();
      object.clear();
      geometry.dispose();
      starMaterial.dispose();
      galaxyGeometry.dispose();
      galaxies.forEach((galaxy) => galaxy.material.dispose());
      textures.forEach(retireTexture);
      textures.clear();
    },
  };
}
