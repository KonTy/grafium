import { fnv1a } from "./tagColor";
import { planetHasRings } from "./planetSystems";
import earthUrl from "../assets/planets/earth.jpg";
import marsUrl from "../assets/planets/mars.jpg";
import venusUrl from "../assets/planets/venus.jpg";
import mercuryUrl from "../assets/planets/mercury.jpg";
import moonUrl from "../assets/planets/moon.jpg";
import jupiterUrl from "../assets/planets/jupiter.jpg";
import saturnUrl from "../assets/planets/saturn.jpg";
import neptuneUrl from "../assets/planets/neptune.jpg";

/** Equirectangular maps from Planet Pixel Emporium (free to use). */
export const PLANET_KINDS = [
  "earth",
  "mars",
  "venus",
  "mercury",
  "moon",
  "jupiter",
  "saturn",
  "neptune",
] as const;

export type PlanetKind = (typeof PLANET_KINDS)[number];

export const PLANET_TEXTURE_URLS: Record<PlanetKind, string> = {
  earth: earthUrl,
  mars: marsUrl,
  venus: venusUrl,
  mercury: mercuryUrl,
  moon: moonUrl,
  jupiter: jupiterUrl,
  saturn: saturnUrl,
  neptune: neptuneUrl,
};

const LAND_AND_GAS: readonly PlanetKind[] = [
  "earth",
  "earth",
  "mars",
  "mars",
  "venus",
  "jupiter",
  "saturn",
  "neptune",
];

const SATELLITES: readonly PlanetKind[] = ["moon", "mercury", "mars"];

export interface OceanWorldPalette {
  ocean: readonly [number, number, number];
  land: readonly [number, number, number];
  highland: readonly [number, number, number];
  ice: readonly [number, number, number];
}

/** Alien land/ocean palettes. Continent layouts are warped away from Earth. */
export const OCEAN_WORLD_PALETTES: readonly OceanWorldPalette[] = [
  { ocean: [78, 16, 110], land: [198, 138, 52], highland: [236, 196, 118], ice: [236, 214, 255] },
  { ocean: [6, 72, 78], land: [176, 68, 42], highland: [224, 142, 86], ice: [176, 255, 228] },
  { ocean: [104, 10, 38], land: [46, 96, 72], highland: [214, 186, 118], ice: [255, 214, 226] },
  { ocean: [12, 24, 92], land: [214, 86, 138], highland: [255, 168, 188], ice: [198, 228, 255] },
  { ocean: [8, 96, 62], land: [96, 62, 132], highland: [186, 154, 224], ice: [214, 255, 210] },
];

export function planetKindFor(title: string, satellite = false): PlanetKind {
  const hash = fnv1a(title.trim().toLowerCase());
  if (satellite) return SATELLITES[hash % SATELLITES.length]!;
  if (planetHasRings(title, false) && hash % 2 === 0) return "saturn";
  return LAND_AND_GAS[hash % LAND_AND_GAS.length]!;
}

export function oceanWorldPaletteIndex(title: string): number {
  return fnv1a(`${title.trim().toLowerCase()}:ocean`) % OCEAN_WORLD_PALETTES.length;
}

export function classifyEarthPixel(r: number, g: number, b: number): "ocean" | "ice" | "land" {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  if (max > 190 && max - min < 45) return "ice";
  if (b > r + 12 && b >= g - 8) return "ocean";
  return "land";
}

export function recolorOceanWorldPixel(
  r: number,
  g: number,
  b: number,
  palette: OceanWorldPalette,
): [number, number, number] {
  const lum = (0.22 * r + 0.67 * g + 0.11 * b) / 255;
  const kind = classifyEarthPixel(r, g, b);
  const src = kind === "ocean"
    ? palette.ocean
    : kind === "ice"
      ? palette.ice
      : r > g + 8 ? palette.highland : palette.land;
  const lift = kind === "ocean" ? 0.28 + lum * 0.95 : 0.38 + lum * 0.72;
  return [
    Math.min(255, Math.round(src[0] * lift)),
    Math.min(255, Math.round(src[1] * lift)),
    Math.min(255, Math.round(src[2] * lift)),
  ];
}

export function wrap01(value: number): number {
  return value - Math.floor(value);
}

/** Scramble equirectangular land so continents are not Earth's. */
export function warpOceanUv(u: number, v: number, seed: number): { u: number; v: number } {
  const s1 = (seed & 1023) / 1023;
  const s2 = ((seed >>> 10) & 1023) / 1023;
  const s3 = ((seed >>> 20) & 1023) / 1023;
  let x = seed & 1 ? 1 - u : u;
  let y = v;
  x = wrap01(x * (1.4 + s1) + s1 + 0.32 * Math.sin((y + s2) * (2.2 + s3 * 2.4) * Math.PI * 2));
  y = wrap01(y + 0.24 * Math.sin((x + s3) * (1.7 + s1 * 2.2) * Math.PI * 2 + s2 * 6.2));
  x = wrap01(x + 0.18 * Math.sin(y * 6.8 + s1 * 9));
  if (seed & 2) {
    const swapped = x;
    x = wrap01(y * 1.55 + s2);
    y = wrap01(swapped * 0.7 + 0.16 * Math.sin(x * Math.PI * 2));
  }
  return { u: x, v: Math.min(0.97, Math.max(0.03, y)) };
}

export function remapOceanWorldImageData(
  source: Uint8ClampedArray,
  dest: Uint8ClampedArray,
  width: number,
  height: number,
  palette: OceanWorldPalette,
  seed: number,
): void {
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const warped = warpOceanUv((x + 0.5) / width, (y + 0.5) / height, seed);
      const sx = Math.min(width - 1, Math.max(0, Math.floor(warped.u * width)));
      const sy = Math.min(height - 1, Math.max(0, Math.floor(warped.v * height)));
      const si = (sy * width + sx) * 4;
      const [r, g, b] = recolorOceanWorldPixel(source[si], source[si + 1], source[si + 2], palette);
      const di = (y * width + x) * 4;
      dest[di] = r;
      dest[di + 1] = g;
      dest[di + 2] = b;
      dest[di + 3] = 255;
    }
  }
}

export function oceanWorldWarpSeed(paletteIndex: number): number {
  return fnv1a(`ocean-layout:${paletteIndex}`);
}
