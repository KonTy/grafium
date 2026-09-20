import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_LOCATION_INSERT_FORMAT,
  formatLocation,
  readLocationInsertFormat,
  requestCurrentPosition,
  saveLocationInsertFormat,
  timeAndLocationSnippet,
  type LocationProvider,
} from "./locationInsert";
import { renderBlock } from "./markdown";

const position = {
  timestamp: 1,
  coords: {
    latitude: 37.7749295,
    longitude: -122.4194156,
    accuracy: 8,
    altitudeAccuracy: null,
    altitude: null,
    speed: null,
    heading: null,
  },
};

describe("location insertion", () => {
  it("defaults to a clickable OpenStreetMap link", () => {
    expect(DEFAULT_LOCATION_INSERT_FORMAT).toBe("openstreetmap");
    expect(formatLocation(position, "openstreetmap")).toBe(
      "[Location: 37.774929, -122.419416](https://www.openstreetmap.org/?mlat=37.774929&mlon=-122.419416#map=18/37.774929/-122.419416)",
    );
  });

  it("supports plain coordinates and device map links", () => {
    expect(formatLocation(position, "coordinates")).toBe("Location: 37.774929, -122.419416");
    expect(formatLocation(position, "geo")).toBe(
      "[Location: 37.774929, -122.419416](geo:37.774929,-122.419416)",
    );
    expect(renderBlock(formatLocation(position, "geo"))).toContain(
      'href="geo:37.774929,-122.419416"',
    );
  });

  it("puts the timestamp and location together on one line", () => {
    expect(timeAndLocationSnippet(position, "coordinates", new Date(2026, 8, 19, 20, 7)))
      .toBe("20:07 Location: 37.774929, -122.419416\n");
  });

  it("persists valid formats and falls back from invalid values", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    };
    saveLocationInsertFormat("geo", storage);
    expect(readLocationInsertFormat(storage)).toBe("geo");
    values.set("grafium.editor.locationFormat", "unknown");
    expect(readLocationInsertFormat(storage)).toBe("openstreetmap");
  });

  it("requests permission before reading a precise position", async () => {
    const provider: LocationProvider = {
      checkPermissions: vi.fn().mockResolvedValue({
        location: "prompt",
        coarseLocation: "prompt",
      }),
      requestPermissions: vi.fn().mockResolvedValue({
        location: "granted",
        coarseLocation: "granted",
      }),
      getCurrentPosition: vi.fn().mockResolvedValue(position),
    };
    await expect(requestCurrentPosition(provider)).resolves.toEqual(position);
    expect(provider.requestPermissions).toHaveBeenCalledWith(["location"]);
    expect(provider.getCurrentPosition).toHaveBeenCalledWith({
      enableHighAccuracy: true,
      timeout: 15_000,
      maximumAge: 60_000,
    });
  });

  it("uses approximate location when that is all Android grants", async () => {
    const provider: LocationProvider = {
      checkPermissions: vi.fn().mockResolvedValue({
        location: "denied",
        coarseLocation: "granted",
      }),
      requestPermissions: vi.fn(),
      getCurrentPosition: vi.fn().mockResolvedValue(position),
    };
    await requestCurrentPosition(provider);
    expect(provider.requestPermissions).not.toHaveBeenCalled();
    expect(provider.getCurrentPosition).toHaveBeenCalledWith(expect.objectContaining({
      enableHighAccuracy: false,
    }));
  });

  it("reports a denied permission instead of pretending insertion succeeded", async () => {
    const provider: LocationProvider = {
      checkPermissions: vi.fn().mockResolvedValue({
        location: "denied",
        coarseLocation: "denied",
      }),
      requestPermissions: vi.fn(),
      getCurrentPosition: vi.fn(),
    };
    await expect(requestCurrentPosition(provider)).rejects.toThrow("Location permission was not granted.");
    expect(provider.getCurrentPosition).not.toHaveBeenCalled();
  });
});
