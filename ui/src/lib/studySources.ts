import type { StudyItem, StudyProgress } from "./studies";

export const studyKindLabels: Record<StudyItem["kind"], string> = {
  page: "Page", book: "Book", flashcards: "Flashcards", audio: "Audio",
  video: "Video", youtube: "YouTube", website: "Website",
};

export function webStudyUrl(source: string): URL {
  if (/[\u0000-\u001f\u007f\\]/.test(source)) throw new Error("Enter a valid HTTP or HTTPS URL.");
  let url: URL;
  try { url = new URL(source.trim()); } catch { throw new Error("Enter a complete HTTP or HTTPS URL."); }
  if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password) {
    throw new Error("Only HTTP and HTTPS URLs without credentials are supported.");
  }
  return url;
}

export function youtubeVideoId(source: string): string {
  const url = webStudyUrl(source);
  const host = url.hostname.toLowerCase();
  let id: string | null = null;
  if (url.port) throw new Error("Enter a YouTube video URL.");
  if (host === "youtu.be") {
    const match = url.pathname.match(/^\/([A-Za-z0-9_-]{11})\/?$/);
    id = match?.[1] ?? null;
  } else if (["youtube.com", "www.youtube.com", "m.youtube.com", "www.youtube-nocookie.com", "youtube-nocookie.com"].includes(host)) {
    if (url.pathname === "/watch" && !host.includes("nocookie")) {
      if (url.searchParams.getAll("v").length === 1) id = url.searchParams.get("v");
    } else {
      id = url.pathname.match(/^\/(?:shorts|embed)\/([A-Za-z0-9_-]{11})\/?$/)?.[1] ?? null;
    }
  }
  if (!id || !/^[A-Za-z0-9_-]{11}$/.test(id)) throw new Error("Enter a YouTube watch, youtu.be, shorts, or embed video URL.");
  return id;
}

export function normalizeStudySource(kind: StudyItem["kind"], source: string): string {
  const value = source.trim();
  if (kind === "flashcards") return value;
  if (!value) throw new Error("Choose a source for this study.");
  if (kind === "page" || kind === "book") return value;
  if (kind === "youtube") return `https://www.youtube.com/watch?v=${youtubeVideoId(value)}`;
  if (kind === "website") return webStudyUrl(value).href;
  if (/^https?:/i.test(value)) return webStudyUrl(value).href;
  // Only graph-relative assets reach the native reader; never file URLs or traversal.
  let decoded: string;
  try { decoded = decodeURIComponent(value); } catch { throw new Error("Invalid graph asset path."); }
  if (/[:\\?#\u0000-\u001f\u007f]/.test(decoded) || decoded.startsWith("/")
    || decoded.split("/").some(part => !part || part === "." || part === "..")
    || decoded.includes("%")) {
    throw new Error("Use a graph-relative asset path (assets/lesson.mp3) or an HTTP(S) media URL.");
  }
  return decoded;
}

export function studySourceFromLink(source: string): {
  kind: StudyItem["kind"]; source: string; filenameTitle?: string;
} {
  const url = webStudyUrl(/^www\./i.test(source.trim()) ? `https://${source.trim()}` : source);
  if (["youtu.be", "youtube.com", "www.youtube.com", "m.youtube.com", "youtube-nocookie.com", "www.youtube-nocookie.com"].includes(url.hostname.toLowerCase()))
    return { kind: "youtube", source: normalizeStudySource("youtube", url.href) };
  const filename = url.pathname.split("/").at(-1) ?? "";
  const extension = filename.split(".").at(-1)?.toLowerCase() ?? "";
  const kind = ["mp3", "wav", "ogg", "m4a", "aac", "flac", "opus"].includes(extension) ? "audio"
    : ["mp4", "webm", "mov", "m4v", "ogv"].includes(extension) ? "video" : "website";
  return { kind, source: url.href,
    ...(kind !== "website" ? { filenameTitle: decodeURIComponent(filename).replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ") } : {}) };
}

export function finiteStudyProgress(position: number, total: number, label = "", anchor = ""): StudyProgress {
  const duration = Number.isFinite(total) && total > 0 ? total : 0;
  const current = Number.isFinite(position) && position > 0 ? position : 0;
  return { position: duration ? Math.min(current, duration) : current, total: duration, label, anchor };
}

export function studyPercent(progress: StudyProgress): number {
  const { position, total } = finiteStudyProgress(progress.position, progress.total);
  return total ? Math.round(position / total * 100) : 0;
}

export function studyTime(seconds: number): string {
  const value = Math.floor(Number.isFinite(seconds) && seconds > 0 ? seconds : 0);
  const hours = Math.floor(value / 3600);
  const minutes = Math.floor(value % 3600 / 60);
  return hours ? `${hours}h ${minutes}m` : minutes ? `${minutes}m ${value % 60}s` : `${value}s`;
}

export function localStudyDay(date = new Date()): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
