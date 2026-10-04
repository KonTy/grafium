import type { ReaderBook, ReaderLocation, ReaderSnapshot } from "./privateReader";

/** Configured Library locations. Snapshots without a list (Android's one
 * storage location, older fixtures) report their single folder as connected. */
export function libraryLocations(snapshot: ReaderSnapshot): ReaderLocation[] {
  if (snapshot.locations) return snapshot.locations;
  return snapshot.libraryPath ? [{
    path: snapshot.libraryPath, connected: true,
    items: snapshot.books.filter(book => !book.sourceUrl).length,
  }] : [];
}

function parts(path: string): string[] {
  return path.split(/[\\/]+/).filter(Boolean);
}

const SHARE = /\/gvfs\/[a-z0-9]+-share:server=([^,/]+),share=([^,/]+)((?:\/[^/]+)*)\/?$/i;

function decoded(value: string): string {
  try { return decodeURIComponent(value); } catch { return value; }
}

/** The drive, card or file server a location is on, when its path shows one. */
export function libraryLocationVolume(path: string): string | null {
  const share = path.match(SHARE);
  if (share) return decoded(share[1]);
  const segments = parts(path);
  if (segments[0] === "run" && segments[1] === "media" && segments.length >= 4) return segments[3];
  if (segments[0] === "media" && segments.length >= 3) return segments[2];
  if (segments[0] === "media" && segments.length === 2) return segments[1];
  if (["mnt", "Volumes"].includes(segments[0]) && segments.length >= 2) return segments[1];
  if (/^[A-Za-z]:$/.test(segments[0] ?? "")) return segments[0].toUpperCase();
  return null;
}

/** A short name for a location: its folder, and the drive or server it is on. */
export function libraryLocationName(path: string): string {
  const share = path.match(SHARE);
  const folder = share && !share[3] ? decoded(share[2]) : parts(path).at(-1) ?? path;
  const volume = libraryLocationVolume(path);
  return !volume || volume === folder ? folder : `${folder} on ${volume}`;
}

/** The location holding a file the user picked, and the file's path inside it. */
export function locationForPath(locations: ReaderLocation[], path: string): { location: ReaderLocation; relativePath: string } | null {
  const file = parts(path);
  let best: { location: ReaderLocation; relativePath: string } | null = null;
  for (const location of locations) {
    const root = parts(location.path);
    if (file.length <= root.length || root.some((segment, index) => segment !== file[index])) continue;
    if (!best || root.length > parts(best.location.path).length)
      best = { location, relativePath: file.slice(root.length).join("/") };
  }
  return best;
}

/** What to tell someone who tries to open an item that cannot be opened now. */
export function unavailableSourceMessage(book: ReaderBook, action = "open"): string {
  if (book.disconnected) {
    const place = book.location ? libraryLocationVolume(book.location) ?? book.location : "its Library location";
    return `“${book.title}” is on ${place}, which isn't connected. Plug in the drive or SD card, or connect to the file server, then try again.`;
  }
  return `This source is unavailable. Relink it before you ${action} it.`;
}

/** Disconnected locations, with the number of items each keeps. */
export function disconnectedLocations(snapshot: ReaderSnapshot): ReaderLocation[] {
  return libraryLocations(snapshot).filter(location => !location.connected && location.items > 0);
}
