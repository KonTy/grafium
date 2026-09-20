import {
  checkPermissions,
  getCurrentPosition,
  requestPermissions,
  type Position,
  type PositionOptions,
} from "@tauri-apps/plugin-geolocation";
import { formatLocalClockTime } from "./editorInsert";

export type LocationInsertFormat = "openstreetmap" | "coordinates" | "geo";

export const LOCATION_INSERT_FORMAT_KEY = "grafium.editor.locationFormat";
export const DEFAULT_LOCATION_INSERT_FORMAT: LocationInsertFormat = "openstreetmap";

type PermissionStatus = Awaited<ReturnType<typeof checkPermissions>>;

export interface LocationProvider {
  checkPermissions: () => Promise<PermissionStatus>;
  requestPermissions: typeof requestPermissions;
  getCurrentPosition: (options?: PositionOptions) => Promise<Position>;
}

const pluginProvider: LocationProvider = {
  checkPermissions,
  requestPermissions,
  getCurrentPosition,
};

export function isLocationInsertFormat(value: string | null): value is LocationInsertFormat {
  return value === "openstreetmap" || value === "coordinates" || value === "geo";
}

export function readLocationInsertFormat(storage?: Pick<Storage, "getItem"> | null): LocationInsertFormat {
  const source = storage ?? (typeof localStorage === "undefined" ? null : localStorage);
  if (!source) return DEFAULT_LOCATION_INSERT_FORMAT;
  try {
    const saved = source.getItem(LOCATION_INSERT_FORMAT_KEY);
    return isLocationInsertFormat(saved) ? saved : DEFAULT_LOCATION_INSERT_FORMAT;
  } catch {
    return DEFAULT_LOCATION_INSERT_FORMAT;
  }
}

export function saveLocationInsertFormat(
  format: LocationInsertFormat,
  storage?: Pick<Storage, "setItem"> | null,
): void {
  const target = storage ?? (typeof localStorage === "undefined" ? null : localStorage);
  if (!target) return;
  try {
    target.setItem(LOCATION_INSERT_FORMAT_KEY, format);
  } catch {
    // Keep the selected in-memory setting usable when storage is unavailable.
  }
}

function fixedCoordinates(position: Position): { latitude: string; longitude: string } {
  return {
    latitude: position.coords.latitude.toFixed(6),
    longitude: position.coords.longitude.toFixed(6),
  };
}

export function formatLocation(position: Position, format: LocationInsertFormat): string {
  const { latitude, longitude } = fixedCoordinates(position);
  const label = `${latitude}, ${longitude}`;
  if (format === "coordinates") return `Location: ${label}`;
  if (format === "geo") return `[Location: ${label}](geo:${latitude},${longitude})`;
  const url = `https://www.openstreetmap.org/?mlat=${latitude}&mlon=${longitude}#map=18/${latitude}/${longitude}`;
  return `[Location: ${label}](${url})`;
}

export function timeAndLocationSnippet(
  position: Position,
  format = readLocationInsertFormat(),
  date = new Date(),
): string {
  return `${formatLocalClockTime(date)} ${formatLocation(position, format)}\n`;
}

function permissionGranted(status: PermissionStatus): boolean {
  return status.location === "granted" || status.coarseLocation === "granted";
}

function permissionCanPrompt(status: PermissionStatus): boolean {
  return [status.location, status.coarseLocation].some(
    (state) => state === "prompt" || state === "prompt-with-rationale",
  );
}

export async function requestCurrentPosition(
  provider: LocationProvider = pluginProvider,
): Promise<Position> {
  let permissions = await provider.checkPermissions();
  if (!permissionGranted(permissions) && permissionCanPrompt(permissions)) {
    permissions = await provider.requestPermissions(["location"]);
  }
  if (!permissionGranted(permissions)) {
    throw new Error("Location permission was not granted.");
  }
  return provider.getCurrentPosition({
    enableHighAccuracy: permissions.location === "granted",
    timeout: 15_000,
    maximumAge: 60_000,
  });
}
