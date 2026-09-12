import { Vector3 } from "three";
import type { GraphEdgeLike } from "./graphClusters";

export function buildFlightNeighbors(
  nodeIds: readonly string[],
  links: readonly GraphEdgeLike[],
): Map<string, string[]> {
  const neighbors = new Map(nodeIds.map((id) => [id, new Set<string>()]));
  for (const { source, target } of links) {
    if (source === target || !neighbors.has(source) || !neighbors.has(target)) continue;
    neighbors.get(source)!.add(target);
    neighbors.get(target)!.add(source);
  }
  return new Map([...neighbors].map(([id, related]) => [id, [...related]]));
}

export function nextFlightTopic(
  neighbors: ReadonlyMap<string, readonly string[]>,
  current: string,
  previous: string | null,
  visits: ReadonlyMap<string, number>,
  random: () => number = Math.random,
): string | undefined {
  const related = neighbors.get(current) ?? [];
  const onward = related.filter((id) => id !== previous);
  const candidates = onward.length > 0 ? onward : related;
  if (candidates.length === 0) return undefined;
  const leastVisits = Math.min(...candidates.map((id) => visits.get(id) ?? 0));
  const choices = candidates.filter((id) => (visits.get(id) ?? 0) === leastVisits);
  return choices[Math.min(choices.length - 1, Math.max(0, Math.floor(random() * choices.length)))];
}

export interface FlightLeg {
  start: Vector3;
  control: Vector3;
  end: Vector3;
  fromLookAt: Vector3;
  target: Vector3;
  durationMs: number;
}

const smoothstep = (t: number) => t * t * (3 - 2 * t);

export function createFlightLeg(
  position: Vector3,
  lookAt: Vector3,
  target: Vector3,
  planetRadius: number,
  view?: { distance: number; approach?: Vector3 },
): FlightLeg {
  const approach = view?.approach?.clone() ?? position.clone().sub(target);
  if (approach.lengthSq() < 0.001) approach.set(0, 0.25, 1);
  approach.normalize();
  const end = target.clone().addScaledVector(approach, view?.distance ?? 40 + planetRadius * 5);
  const distance = position.distanceTo(end);
  const side = new Vector3().crossVectors(approach, new Vector3(0, 1, 0));
  if (side.lengthSq() < 0.001) side.set(1, 0, 0);
  const control = position.clone().lerp(end, 0.5)
    .addScaledVector(side.normalize(), Math.min(180, distance * 0.3));
  return {
    start: position.clone(),
    control,
    end,
    fromLookAt: lookAt.clone(),
    target: target.clone(),
    durationMs: Math.min(7500, Math.max(3600, distance * 6)),
  };
}

export function sampleFlightLeg(leg: FlightLeg, fraction: number) {
  const t = smoothstep(Math.min(1, Math.max(0, fraction)));
  return {
    position: leg.start.clone().multiplyScalar((1 - t) ** 2)
      .addScaledVector(leg.control, 2 * (1 - t) * t)
      .addScaledVector(leg.end, t * t),
    lookAt: leg.fromLookAt.clone().lerp(leg.target, smoothstep(Math.min(1, t * 1.6))),
  };
}

export function sampleFlightOrbit(leg: FlightLeg, fraction: number) {
  const t = smoothstep(Math.min(1, Math.max(0, fraction)));
  return {
    position: leg.end.clone().sub(leg.target)
      .applyAxisAngle(new Vector3(0, 1, 0), t * 0.45).add(leg.target),
    lookAt: leg.target.clone(),
  };
}
