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
  ahead: Vector3;
  durationMs: number;
}

const smoothstep = (t: number) => t * t * (3 - 2 * t);
const clamp01 = (t: number) => Math.min(1, Math.max(0, t));

export function createFlightLeg(
  position: Vector3,
  lookAt: Vector3,
  target: Vector3,
  planetRadius: number,
  view?: { distance?: number; approach?: Vector3; nextTarget?: Vector3 },
): FlightLeg {
  const inbound = target.clone().sub(position);
  if (inbound.lengthSq() < 0.001) inbound.set(0, 0.15, 1);
  inbound.normalize();

  const passDistance = view?.distance ?? 40 + planetRadius * 5;
  const side = new Vector3().crossVectors(inbound, new Vector3(0, 1, 0));
  if (side.lengthSq() < 0.001) side.set(1, 0, 0);
  side.normalize();

  const bias = view?.approach?.clone() ?? view?.nextTarget?.clone().sub(target) ?? null;
  if (bias && bias.lengthSq() > 0.001 && side.dot(bias) < 0) side.negate();

  const lift = new Vector3().crossVectors(side, inbound);
  if (lift.lengthSq() < 0.001) lift.set(0, 1, 0);
  lift.normalize();

  const periapsis = target.clone()
    .addScaledVector(side, passDistance)
    .addScaledVector(lift, passDistance * 0.22);

  const outbound = view?.nextTarget ? view.nextTarget.clone().sub(target) : inbound.clone();
  if (outbound.lengthSq() < 0.001) outbound.copy(inbound);
  outbound.normalize();

  const end = target.clone()
    .addScaledVector(outbound, Math.max(passDistance * 2.2, 80))
    .addScaledVector(side, passDistance * 0.55)
    .addScaledVector(lift, passDistance * 0.12);

  // Quadratic control so t = 0.5 actually flies through the closest approach.
  const control = periapsis.clone().multiplyScalar(2)
    .sub(position.clone().add(end).multiplyScalar(0.5));

  const ahead = view?.nextTarget?.clone()
    ?? end.clone().addScaledVector(outbound, passDistance * 3);

  const travel = position.distanceTo(periapsis) + periapsis.distanceTo(end);
  return {
    start: position.clone(),
    control,
    end,
    fromLookAt: lookAt.clone(),
    target: target.clone(),
    ahead,
    durationMs: Math.min(10000, Math.max(4800, travel * 5.5)),
  };
}

/** Spend extra clock time near closest approach so planet labels can be read. */
export function lingerFlightFraction(u: number): number {
  const t = clamp01(u);
  if (t < 0.34) return (t / 0.34) * 0.42;
  if (t < 0.72) return 0.42 + ((t - 0.34) / 0.38) * 0.16;
  return 0.58 + ((t - 0.72) / 0.28) * 0.42;
}

export function sampleFlightLeg(leg: FlightLeg, fraction: number) {
  const t = smoothstep(clamp01(fraction));
  const position = leg.start.clone().multiplyScalar((1 - t) ** 2)
    .addScaledVector(leg.control, 2 * (1 - t) * t)
    .addScaledVector(leg.end, t * t);
  const lookAt = t < 0.32
    ? leg.fromLookAt.clone().lerp(leg.target, smoothstep(t / 0.32))
    : t < 0.66
      ? leg.target.clone()
      : leg.target.clone().lerp(leg.ahead, smoothstep((t - 0.66) / 0.34));
  return { position, lookAt };
}
