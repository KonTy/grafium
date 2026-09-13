import { describe, expect, it } from "vitest";
import {
  classifyEarthPixel,
  oceanWorldPaletteIndex,
  oceanWorldWarpSeed,
  OCEAN_WORLD_PALETTES,
  planetKindFor,
  PLANET_KINDS,
  recolorOceanWorldPixel,
  warpOceanUv,
} from "./planetTextures";

describe("planet skins", () => {
  it("picks a stable skin from the topic title", () => {
    expect(planetKindFor("Health/Supplements")).toBe(planetKindFor("Health/Supplements"));
    expect(PLANET_KINDS).toContain(planetKindFor("Earth notes"));
  });

  it("mixes land worlds and gas giants, and uses rocky moons for satellites", () => {
    const titles = Array.from({ length: 80 }, (_, i) => `Topic ${i}`);
    const kinds = new Set(titles.map((title) => planetKindFor(title)));
    expect(kinds.has("earth") || kinds.has("mars") || kinds.has("venus")).toBe(true);
    expect(kinds.has("jupiter") || kinds.has("saturn") || kinds.has("neptune")).toBe(true);
    expect(["moon", "mercury", "mars"]).toContain(planetKindFor("Health/Supplements/Creatine", true));
  });

  it("recolors oceans without keeping Earth blue", () => {
    const palette = OCEAN_WORLD_PALETTES[oceanWorldPaletteIndex("Alien sea")]!;
    expect(classifyEarthPixel(20, 40, 140)).toBe("ocean");
    expect(classifyEarthPixel(70, 120, 50)).toBe("land");
    const ocean = recolorOceanWorldPixel(20, 40, 140, palette);
    expect(Math.hypot(ocean[0] - 20, ocean[1] - 40, ocean[2] - 140)).toBeGreaterThan(40);
  });

  it("moves landmasses so the map is not Earth's continents", () => {
    const africa = warpOceanUv(0.52, 0.48, oceanWorldWarpSeed(0));
    expect(africa).not.toEqual({ u: 0.52, v: 0.48 });
    expect(warpOceanUv(0.52, 0.48, oceanWorldWarpSeed(0)))
      .not.toEqual(warpOceanUv(0.52, 0.48, oceanWorldWarpSeed(1)));
  });
});
