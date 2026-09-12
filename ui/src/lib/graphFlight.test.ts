import { describe, expect, it } from "vitest";
import { Vector3 } from "three";
import { buildFlightNeighbors, createFlightLeg, nextFlightTopic, sampleFlightLeg, sampleFlightOrbit } from "./graphFlight";

describe("linked-topic flight", () => {
  const neighbors = buildFlightNeighbors(["a", "b", "c", "d", "isolated"], [
    { source: "a", target: "b" }, { source: "b", target: "c" },
    { source: "c", target: "a" }, { source: "b", target: "d" },
    { source: "a", target: "b" }, { source: "a", target: "a" },
    { source: "a", target: "missing" },
  ]);

  it("uses only visible, real relationships in either direction", () => {
    expect(neighbors.get("a")).toEqual(["b", "c"]);
    expect(neighbors.get("d")).toEqual(["b"]);
    expect(neighbors.get("isolated")).toEqual([]);
    expect(nextFlightTopic(neighbors, "isolated", null, new Map())).toBeUndefined();
  });

  it("prefers less-visited neighbors and avoids immediate backtracking", () => {
    expect(nextFlightTopic(neighbors, "b", "a", new Map([["c", 2]]), () => 0)).toBe("d");
    expect(nextFlightTopic(neighbors, "b", "a", new Map(), () => 0)).toBe("c");
    expect(nextFlightTopic(neighbors, "d", "b", new Map(), () => 0)).toBe("b");
  });

  it("keeps flying without jumping between disconnected components", () => {
    const visits = new Map<string, number>();
    let previous: string | null = null;
    let current = "a";
    for (let i = 0; i < 200; i++) {
      visits.set(current, (visits.get(current) ?? 0) + 1);
      const next = nextFlightTopic(neighbors, current, previous, visits, () => 0)!;
      expect(neighbors.get(current)).toContain(next);
      previous = current;
      current = next;
    }
    expect([...visits.keys()].sort()).toEqual(["a", "b", "c", "d"]);
  });

  it("reverses indefinitely when only two topics are linked", () => {
    const pair = buildFlightNeighbors(["a", "b"], [{ source: "a", target: "b" }]);
    expect(nextFlightTopic(pair, "a", "b", new Map())).toBe("b");
    expect(nextFlightTopic(pair, "b", "a", new Map())).toBe("a");
  });
});

describe("camera flight geometry", () => {
  const position = new Vector3(0, 0, 900);
  const lookAt = new Vector3();
  const target = new Vector3(320, 40, -80);
  const leg = createFlightLeg(position, lookAt, target, 20);

  it("starts at the current view and docks outside the planet", () => {
    expect(sampleFlightLeg(leg, 0)).toEqual({ position, lookAt });
    expect(sampleFlightLeg(leg, 1).lookAt).toEqual(target);
    expect(leg.end.distanceTo(target)).toBeCloseTo(140);
    expect(position.toArray()).toEqual([0, 0, 900]);
    expect(target.toArray()).toEqual([320, 40, -80]);
  });

  it("joins the arrival orbit and the next leg without snapping", () => {
    expect(sampleFlightOrbit(leg, 0)).toEqual(sampleFlightLeg(leg, 1));
    const orbitEnd = sampleFlightOrbit(leg, 1);
    expect(orbitEnd.position.distanceTo(target)).toBeCloseTo(140);
    const next = createFlightLeg(orbitEnd.position, orbitEnd.lookAt, new Vector3(-200, 30, 50), 12);
    expect(sampleFlightLeg(next, 0)).toEqual(orbitEnd);
  });

  it("handles coincident positions and vertical approaches without NaN", () => {
    for (const start of [target, target.clone().add(new Vector3(0, 500, 0))]) {
      const path = createFlightLeg(start, lookAt, target, 10);
      for (const fraction of [-1, 0, 0.5, 1, 2]) {
        expect(sampleFlightLeg(path, fraction).position.toArray().every(Number.isFinite)).toBe(true);
      }
    }
  });

  it("can frame a planet family and approach a satellite from outside its parent", () => {
    const outward = new Vector3(1, 0, 0);
    const path = createFlightLeg(position, lookAt, target, 8, {
      distance: 300, approach: outward,
    });
    expect(sampleFlightLeg(path, 0)).toEqual({ position, lookAt });
    expect(path.end).toEqual(target.clone().add(new Vector3(300, 0, 0)));
    expect(sampleFlightLeg(path, 1).lookAt).toEqual(target);
    expect(outward.toArray()).toEqual([1, 0, 0]);
  });
});
