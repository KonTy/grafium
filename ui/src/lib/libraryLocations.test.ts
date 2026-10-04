import { describe, expect, it } from "vitest";
import type { ReaderBook, ReaderSnapshot } from "./privateReader";
import {
  disconnectedLocations, libraryLocationName, libraryLocationVolume, libraryLocations,
  locationForPath, unavailableSourceMessage,
} from "./libraryLocations";

const book = (extra: Partial<ReaderBook> = {}): ReaderBook => ({
  id: "dune", title: "Dune", kind: "epub", available: false, tracks: [], position: null, bookmarks: [], ...extra,
});

describe("Library location names", () => {
  it("names the drive, card or server a location is on", () => {
    for (const [path, volume, name] of [
      ["/run/media/blin/6TWDBACKUP/books", "6TWDBACKUP", "books on 6TWDBACKUP"],
      ["/run/media/blin/SDCARD", "SDCARD", "SDCARD"],
      ["/media/blin/Card/Audio", "Card", "Audio on Card"],
      ["/media/USB", "USB", "USB"],
      ["/mnt/nas/books", "nas", "books on nas"],
      ["/run/user/1000/gvfs/smb-share:server=nas,share=media", "nas", "media on nas"],
      ["/run/user/1000/gvfs/smb-share:server=nas,share=media/Audio%20books/Kids", "nas", "Kids on nas"],
      ["/Volumes/Travel/Books", "Travel", "Books on Travel"],
      ["E:\\Books", "E:", "Books on E:"],
      ["/home/blin/Books", null, "Books"],
    ] as const) {
      expect(libraryLocationVolume(path), path).toBe(volume);
      expect(libraryLocationName(path), path).toBe(name);
    }
  });
});

describe("Library locations", () => {
  const snapshot = (extra: Partial<ReaderSnapshot>): ReaderSnapshot => ({ libraryPath: null, books: [], ...extra });

  it("reads the reported locations, or the single folder of Android and older snapshots", () => {
    const locations = [{ path: "/a", connected: true, items: 1 }, { path: "/b", connected: false, reason: "Not connected", items: 2 }];
    expect(libraryLocations(snapshot({ locations }))).toBe(locations);
    expect(libraryLocations(snapshot({ libraryPath: "Books", books: [book(), book({ id: "link", kind: "youtube", sourceUrl: "https://www.youtube.com/watch?v=abcdefghijk" })] })))
      .toEqual([{ path: "Books", connected: true, items: 1 }]);
    expect(libraryLocations(snapshot({}))).toEqual([]);
    expect(disconnectedLocations(snapshot({ locations: [...locations, { path: "/c", connected: false, items: 0 }] })))
      .toEqual([locations[1]]);
  });

  it("finds which location holds a picked file, without matching a sibling or the folder itself", () => {
    const locations = [
      { path: "/run/media/blin/6TWDBACKUP/books", connected: true, items: 0 },
      { path: "/run/media/blin/6TWDBACKUP/books/audio", connected: true, items: 0 },
      { path: "/mnt/nas", connected: false, items: 0 },
    ];
    expect(locationForPath(locations, "/run/media/blin/6TWDBACKUP/books/Dune.epub"))
      .toEqual({ location: locations[0], relativePath: "Dune.epub" });
    expect(locationForPath(locations, "/run/media/blin/6TWDBACKUP/books/audio/Novel"))
      .toEqual({ location: locations[1], relativePath: "Novel" });
    expect(locationForPath(locations, "/mnt/nas/Disc 1/1.mp3")).toEqual({ location: locations[2], relativePath: "Disc 1/1.mp3" });
    expect(locationForPath(locations, "/run/media/blin/6TWDBACKUP/books2/Dune.epub")).toBeNull();
    expect(locationForPath(locations, "/run/media/blin/6TWDBACKUP/books")).toBeNull();
  });

  it("asks to plug in a disconnected item's drive and to relink a missing one", () => {
    const message = unavailableSourceMessage(book({ disconnected: true, location: "/run/media/blin/6TWDBACKUP/books" }));
    expect(message).toBe("“Dune” is on 6TWDBACKUP, which isn't connected. Plug in the drive or SD card, or connect to the file server, then try again.");
    expect(unavailableSourceMessage(book({ disconnected: true, location: "/home/blin/Books" }))).toContain("is on /home/blin/Books,");
    expect(unavailableSourceMessage(book(), "play")).toBe("This source is unavailable. Relink it before you play it.");
  });
});
