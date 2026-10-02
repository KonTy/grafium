<script lang="ts">
  import LibrarySourcePlayer from "./LibrarySourcePlayer.svelte";
  import type { StudyItem, StudyProgress } from "../lib/studies";
  let { graphPath, item, onProgress, onPlayback, onActivity }: {
    graphPath: string; item: StudyItem; onProgress: (progress: StudyProgress) => void;
    onPlayback: (playing: boolean) => void; onActivity?: () => void;
  } = $props();
</script>

<LibrarySourcePlayer {graphPath} {item} onProgress={progress => onProgress(progress)} {onPlayback} {onActivity} />
