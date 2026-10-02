import { mount, unmount, flushSync } from "svelte";
import Player from "../../src/components/LibrarySourcePlayer.svelte";
import * as preferences from "../../src/lib/readerPlaybackPreferences";

export { preferences };
export const progress: unknown[] = [];
let component: ReturnType<typeof mount> | undefined;

export async function mountSource({ kind, local, origin }: { kind: "audio" | "video"; local: boolean; origin: string }) {
  if (component) await unmount(component);
  const source = `${origin}/synthetic-${kind === "audio" ? "audio.wav" : "video.webm"}`;
  component = mount(Player, { target: document.body, props: {
    item: { id: `${kind}-${local}`, kind, title: "Synthetic playback speed",
      source: local ? "Synthetic/local-media" : source,
      progress: { position: 3, total: 30, anchor: "", label: "" } },
    ...(local ? { resolveMedia: async () => source } : {}),
    autoplay: false, onProgress: value => progress.push(value), onPlayback: () => {},
  } });
  flushSync();
}
