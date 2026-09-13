import { describe, expect, it } from "vitest";
import { Vector3 } from "three";
import { buildFlightNeighbors, createFlightLeg, lingerFlightFraction, nextFlightTopic, sampleFlightLeg } from "./graphFlight";

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

  it("starts at the current view and flies past the planet instead of docking", () => {
    expect(sampleFlightLeg(leg, 0)).toEqual({ position, lookAt });
    const closest = sampleFlightLeg(leg, 0.5).position.distanceTo(target);
    expect(closest).toBeGreaterThan(100);
    expect(closest).toBeLessThan(180);
    expect(leg.end.distanceTo(target)).toBeGreaterThan(closest);
    expect(sampleFlightLeg(leg, 0.5).lookAt).toEqual(target);
    expect(sampleFlightLeg(leg, 1).lookAt).not.toEqual(target);
    expect(position.toArray()).toEqual([0, 0, 900]);
    expect(target.toArray()).toEqual([320, 40, -80]);
  });

  it("spends extra clock time looking at the planet near closest approach", () => {
    expect(lingerFlightFraction(0)).toBe(0);
    expect(lingerFlightFraction(1)).toBe(1);
    expect(lingerFlightFraction(0.53)).toBeGreaterThan(0.45);
    expect(lingerFlightFraction(0.53)).toBeLessThan(0.55);
    expect(lingerFlightFraction(0.72) - lingerFlightFraction(0.34))
      .toBeLessThan(lingerFlightFraction(0.34) - lingerFlightFraction(0));
  });

  it("joins one flyby to the next without snapping", () => {
    const flybyEnd = sampleFlightLeg(leg, 1);
    const nextPlanet = new Vector3(-200, 30, 50);
    const next = createFlightLeg(flybyEnd.position, flybyEnd.lookAt, nextPlanet, 12, {
      nextTarget: new Vector3(80, -10, 40),
    });
    expect(sampleFlightLeg(next, 0)).toEqual(flybyEnd);
  });

  it("handles coincident positions and vertical approaches without NaN", () => {
    for (const start of [target, target.clone().add(new Vector3(0, 500, 0))]) {
      const path = createFlightLeg(start, lookAt, target, 10);
      for (const fraction of [-1, 0, 0.5, 1, 2]) {
        expect(sampleFlightLeg(path, fraction).position.toArray().every(Number.isFinite)).toBe(true);
      }
    }
  });

  it("passes on the outward side and looks toward the next planet by the end", () => {
    const outward = new Vector3(1, 0, 0);
    const nextPlanet = new Vector3(-200, 30, 50);
    const path = createFlightLeg(position, lookAt, target, 8, {
      distance: 300, approach: outward, nextTarget: nextPlanet,
    });
    expect(sampleFlightLeg(path, 0)).toEqual({ position, lookAt });
    const closest = sampleFlightLeg(path, 0.5).position;
    expect(closest.distanceTo(target)).toBeGreaterThan(250);
    expect(closest.distanceTo(target)).toBeLessThan(400);
    expect(closest.clone().sub(target).dot(outward)).toBeGreaterThan(0);
    expect(sampleFlightLeg(path, 1).lookAt).toEqual(nextPlanet);
    expect(outward.toArray()).toEqual([1, 0, 0]);
  });
});
