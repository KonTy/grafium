import { Vector3 } from "three";
import { fnv1a } from "./tagColor";

interface Topic {
  id: string;
  name: string;
}

export interface PositionedPlanet extends Topic {
  x: number;
  y: number;
  z: number;
  radius: number;
}

export interface PlanetSystem {
  parentById: Map<string, string>;
  childrenById: Map<string, string[]>;
}

function topicPath(title: string): string[] {
  return title.split("/").map((part) => part.trim().toLowerCase()).filter(Boolean);
}

/** Use only existing visible ancestors, never invent a page or infer a semantic relation. */
export function buildPlanetHierarchy(topics: readonly Topic[]): PlanetSystem {
  const byPath = new Map(topics.map((topic) => [topicPath(topic.name).join("/"), topic.id]));
  const parentById = new Map<string, string>();
  const childrenById = new Map<string, string[]>();
  for (const topic of topics) {
    const path = topicPath(topic.name);
    while (path.length > 1) {
      path.pop();
      const parent = byPath.get(path.join("/"));
      if (!parent || parent === topic.id) continue;
      parentById.set(topic.id, parent);
      const children = childrenById.get(parent) ?? [];
      children.push(topic.id);
      childrenById.set(parent, children);
      break;
    }
  }
  return { parentById, childrenById };
}

export function planetHasRings(title: string, satellite: boolean): boolean {
  return !satellite && fnv1a(topicPath(title).join("/")) % 3 === 0;
}

export interface PlanetLayout extends PlanetSystem {
  positions: Map<string, Vector3>;
  radii: Map<string, number>;
  extents: Map<string, number>;
}

/** A display-only layout; the force graph's original node objects remain untouched. */
export function layoutPlanetSystems(topics: readonly PositionedPlanet[]): PlanetLayout {
  const hierarchy = buildPlanetHierarchy(topics);
  const byId = new Map(topics.map((topic) => [topic.id, topic]));
  const radii = new Map<string, number>();
  const extents = new Map<string, number>();
  const positions = new Map<string, Vector3>();
  const orbitRadii = new Map<string, number>();
  const roots = topics.filter((topic) => !hierarchy.parentById.has(topic.id));

  function measure(topic: PositionedPlanet, parentRadius?: number): number {
    const children = hierarchy.childrenById.get(topic.id) ?? [];
    const radius = parentRadius === undefined
      ? children.length ? Math.max(28, topic.radius * 1.5) : topic.radius
      : parentRadius * 0.3;
    radii.set(topic.id, radius);
    const childExtents = children.map((id) => measure(byId.get(id)!, radius));
    const largestChild = Math.max(0, ...childExtents);
    const orbit = Math.max(radius * 3.8, largestChild * Math.max(3, Math.sqrt(children.length) * 1.7));
    orbitRadii.set(topic.id, orbit);
    const extent = children.length ? orbit + largestChild : radius;
    extents.set(topic.id, extent);
    return extent;
  }
  roots.forEach((topic) => measure(topic));
  const spread = Math.max(1, ...roots.map((topic) => extents.get(topic.id)! / 140));

  function place(topic: PositionedPlanet, position: Vector3): void {
    positions.set(topic.id, position);
    const children = [...(hierarchy.childrenById.get(topic.id) ?? [])]
      .sort((a, b) => byId.get(a)!.name.localeCompare(byId.get(b)!.name));
    const phase = (fnv1a(topic.name.toLowerCase()) % 360) * Math.PI / 180;
    children.forEach((id, index) => {
      const y = 1 - 2 * (index + 0.5) / children.length;
      const radial = Math.sqrt(1 - y * y);
      const angle = phase + index * Math.PI * (3 - Math.sqrt(5));
      const offset = new Vector3(Math.cos(angle) * radial, y, Math.sin(angle) * radial)
        .multiplyScalar(orbitRadii.get(topic.id)!);
      place(byId.get(id)!, position.clone().add(offset));
    });
  }
  roots.forEach((topic) => place(topic, new Vector3(topic.x, topic.y, topic.z).multiplyScalar(spread)));
  return { ...hierarchy, positions, radii, extents };
}
