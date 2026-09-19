/**
 * Selection maths for the chat list.
 *
 * This lives outside the component because it decides what a destructive
 * button is about to destroy, and that decision deserves to be pinned by
 * tests rather than inferred from a template.
 */

/**
 * Which conversations a delete action should remove.
 *
 * An empty selection means "all of them": the one button does both jobs, so
 * the absence of a selection is what distinguishes them. That is exactly why
 * the caller must show a confirmation naming the result — see
 * {@link deletionPrompt} — because the difference between deleting two chats
 * and deleting forty is otherwise invisible state.
 *
 * Returns entries in list order, and ignores selected ids that no longer
 * exist, so a stale selection can never widen the blast radius.
 */
export function deletionTargets<T extends { id: string }>(
  threads: readonly T[],
  selected: ReadonlySet<string>,
): T[] {
  if (selected.size === 0) return [...threads];
  return threads.filter((thread) => selected.has(thread.id));
}

/**
 * Whether the delete button would take everything.
 *
 * Deliberately not `selected.size === 0`: a selection covering every chat
 * deletes everything too, and the confirmation should say so rather than
 * claiming only some are going.
 */
export function deletionTakesEverything(
  threadCount: number,
  targetCount: number,
): boolean {
  return threadCount > 0 && targetCount >= threadCount;
}

/**
 * The text shown before anything is deleted.
 *
 * Names the count and the scope, because this is the only thing standing
 * between "delete the two I picked" and "delete all forty". In-flight
 * conversations are called out separately: stopping work already running is a
 * second consequence, and it is not obvious from the chat list alone.
 */
export function deletionPrompt(
  count: number,
  everything: boolean,
  running: number,
): string {
  const scope = everything
    ? count === 1
      ? "Delete this chat?"
      : `Delete all ${count} chats?`
    : count === 1
      ? "Delete the selected chat?"
      : `Delete ${count} selected chats?`;
  const work =
    running === 0
      ? ""
      : running === 1
        ? " One is still working and will be stopped."
        : ` ${running} are still working and will be stopped.`;
  return `${scope}${work} This cannot be undone.`;
}

/**
 * The ids covered by a shift-click, inclusive of both ends.
 *
 * Without a usable anchor there is no range, so only the clicked row is
 * returned — shift-clicking as the very first action selects one chat rather
 * than silently reaching back to the top of the list.
 */
export function selectRange(
  ids: readonly string[],
  anchorId: string | null,
  targetId: string,
): string[] {
  const target = ids.indexOf(targetId);
  if (target < 0) return [];
  const anchor = anchorId === null ? -1 : ids.indexOf(anchorId);
  if (anchor < 0) return [targetId];
  const [from, to] = anchor <= target ? [anchor, target] : [target, anchor];
  return ids.slice(from, to + 1);
}

/**
 * Drop selected ids that no longer exist.
 *
 * A chat can vanish underneath the selection — deleted from its own row, or
 * the whole list swapped when the graph changed. Left alone, those ids would
 * make the button count lie about how much it is about to delete.
 */
export function pruneSelection(
  selected: ReadonlySet<string>,
  liveIds: Iterable<string>,
): Set<string> {
  const live = new Set(liveIds);
  const kept = new Set<string>();
  for (const id of selected) {
    if (live.has(id)) kept.add(id);
  }
  return kept;
}
