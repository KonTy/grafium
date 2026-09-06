/**
 * Memoises a dynamic `import()` so repeated calls return the same promise.
 *
 * `{#await loader() then ...}` re-evaluates its expression whenever the block
 * re-renders. Handing it a fresh promise each time would drop the resolved
 * component back into its pending branch and flash the fallback, so the
 * promise is cached after the first call.
 */
export function lazyComponent<T>(
  loader: () => Promise<{ default: T }>
): () => Promise<{ default: T }> {
  let cached: Promise<{ default: T }> | undefined;
  return () => (cached ??= loader());
}
