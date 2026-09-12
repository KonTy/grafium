import { describe, expect, it } from "vitest";
import { buildPlanetHierarchy, layoutPlanetSystems, planetHasRings } from "./planetSystems";

const topics = [
  { id: "supplements", name: "Health/Supplements", x: 100, y: 40, z: 20, radius: 18 },
  { id: "creatine", name: "Health/Supplements/Creatine", x: -200, y: 0, z: 0, radius: 20 },
  { id: "vitamins", name: "Health/Supplements/Vitamins", x: 200, y: 0, z: 0, radius: 10 },
  { id: "d3", name: "Health/Supplements/Vitamins/D3", x: 0, y: 200, z: 0, radius: 12 },
  { id: "other", name: "Supplements", x: -400, y: 0, z: 0, radius: 14 },
];

describe("topic planet systems", () => {
  it("uses the nearest visible namespace parent, not a similar title", () => {
    const { parentById } = buildPlanetHierarchy(topics);
    expect([...parentById]).toEqual([
      ["creatine", "supplements"], ["vitamins", "supplements"], ["d3", "vitamins"],
    ]);
    expect(parentById.has("other")).toBe(false);
    const skipped = buildPlanetHierarchy(topics.filter((node) => node.id !== "vitamins"));
    expect(skipped.parentById.get("d3")).toBe("supplements");
  });

  it("handles case differences without inventing invisible parent planets", () => {
    expect(buildPlanetHierarchy([
      { id: "p", name: "HEALTH" }, { id: "c", name: "health/Fitness" },
      { id: "x", name: "Other/Fitness" },
    ]).parentById).toEqual(new Map([["c", "p"]]));
  });

  it("makes descendants smaller and places them outside their parent", () => {
    const original = structuredClone(topics);
    const layout = layoutPlanetSystems(topics);
    for (const [child, parent] of layout.parentById) {
      expect(layout.radii.get(child)!).toBeLessThan(layout.radii.get(parent)! * 0.5);
      const distance = layout.positions.get(child)!.distanceTo(layout.positions.get(parent)!);
      expect(distance).toBeGreaterThan(layout.radii.get(parent)! + layout.extents.get(child)!);
      expect(layout.positions.get(child)!.toArray().every(Number.isFinite)).toBe(true);
    }
    expect(topics).toEqual(original);
  });

  it("keeps layouts stable when graph loading order changes", () => {
    const first = layoutPlanetSystems(topics);
    const reversed = layoutPlanetSystems([...topics].reverse());
    for (const node of topics) {
      expect(reversed.positions.get(node.id)).toEqual(first.positions.get(node.id));
      expect(reversed.radii.get(node.id)).toBe(first.radii.get(node.id));
    }
  });

  it("varies rings consistently and never gives satellites rings", () => {
    const names = ["Rust", "WebAssembly", "JavaScript", "Health/Supplements", "Books", "Travel"];
    expect(new Set(names.map((name) => planetHasRings(name, false))).size).toBe(2);
    for (const name of names) {
      expect(planetHasRings(name, true)).toBe(false);
      expect(planetHasRings(name.toUpperCase(), false)).toBe(planetHasRings(name, false));
    }
  });

  it("keeps unrelated standalone planets in their original positions", () => {
    const single = topics.slice(-1);
    const layout = layoutPlanetSystems(single);
    expect(layout.positions.get("other")!.toArray()).toEqual([-400, 0, 0]);
    expect(layout.radii.get("other")).toBe(14);
  });
});
