import type { StudyItem, StudyProgress } from "./studies";
import { formatLocalIsoDate } from "./journalDate";

export const STUDY_IDLE_MS = 90_000;
const MEDIA_HEARTBEAT_MS = 4_000;
export type StudyClockState = "active" | "idle" | "paused" | "hidden" | "stopped";
type SaveActivity = (seconds: number, day: string, progress: StudyProgress | null, requestId: string) => Promise<void>;

/** One clock per explicitly opened study. Navigation owns its lifetime, not the reader. */
export class StudyTracker {
  private previous: number;
  private lastActivity = -Infinity;
  private heartbeat = -Infinity;
  private playing = false;
  private visible = true;
  private paused = false;
  private stopped = false;
  private pending = new Map<string, number>();
  private progress: StudyProgress | null = null;
  private writing: Promise<void> | null = null;
  private batch: { seconds: number; day: string; progress: StudyProgress | null; requestId: string } | null = null;
  private elapsed = 0;

  constructor(
    readonly item: StudyItem,
    private save: SaveActivity,
    private now = () => performance.now(),
    private day = () => formatLocalIsoDate(),
  ) {
    this.previous = now();
  }

  get state(): StudyClockState {
    if (this.stopped) return "stopped";
    if (this.paused) return "paused";
    if (!this.visible) return "hidden";
    if (this.item.kind === "website") return "idle";
    const active = this.isMedia
      ? this.playing && this.now() - this.heartbeat < MEDIA_HEARTBEAT_MS
      : this.now() - this.lastActivity < STUDY_IDLE_MS;
    return active ? "active" : "idle";
  }
  get seconds() { return this.elapsed; }
  private get isMedia() { return ["audio", "video", "youtube"].includes(this.item.kind); }

  tick() {
    const now = this.now();
    const end = this.isMedia ? this.heartbeat + MEDIA_HEARTBEAT_MS : this.lastActivity + STUDY_IDLE_MS;
    const delta = Math.max(0, Math.min(now, end) - this.previous);
    // A suspended WebView must not turn sleep or a stalled event loop into study time.
    if (!this.stopped && !this.paused && this.visible && this.item.kind !== "website"
      && (!this.isMedia || this.playing) && now - this.previous <= 2500 && delta > 0) {
      const seconds = delta / 1000;
      const day = this.day();
      this.pending.set(day, (this.pending.get(day) ?? 0) + seconds);
      this.elapsed += seconds;
    }
    this.previous = now;
  }
  activity() {
    this.tick();
    this.lastActivity = this.now();
  }
  updateProgress(progress: StudyProgress) {
    if (this.stopped) return;
    this.tick();
    this.progress = { ...progress };
    if (this.isMedia) this.heartbeat = this.now();
  }
  playback(playing: boolean) {
    this.tick();
    if (playing && !this.playing) this.heartbeat = this.now();
    this.playing = playing;
  }
  setVisible(visible: boolean) {
    this.tick();
    this.visible = visible;
  }
  setPaused(paused: boolean) {
    this.tick();
    this.paused = paused;
    if (!paused) this.lastActivity = this.now();
  }
  stop() {
    this.tick();
    this.stopped = true;
    return this.flush();
  }
  flush(): Promise<void> {
    if (this.writing) return this.writing.then(() => this.flush());
    if (!this.batch) {
      const entry = [...this.pending].find(([, seconds]) => seconds > 0);
      const day = entry?.[0] ?? this.day();
      const seconds = Math.min(120, entry?.[1] ?? 0);
      if (!seconds && !this.progress) return Promise.resolve();
      this.pending.set(day, (this.pending.get(day) ?? 0) - seconds);
      this.batch = { seconds, day, progress: this.progress, requestId: crypto.randomUUID() };
      this.progress = null;
    }
    const { seconds, day, progress, requestId } = this.batch;
    this.writing = this.save(seconds, day, progress, requestId)
      .then(() => { this.batch = null; })
      .finally(() => { this.writing = null; });
    return this.writing.then(() => this.flush());
  }
}
