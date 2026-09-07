/**
 * Development-only diagnostic logging.
 *
 * These call sites record note content (block text, page titles, backlink
 * excerpts), so they must never run in a release build. The payload is passed
 * as a thunk so that neither the object nor its `JSON.stringify` is evaluated
 * when logging is off — some call sites map over every backlink on a page.
 *
 * `import.meta.env.DEV` is statically replaced by Vite, so the body is dropped
 * from the production bundle entirely.
 */
export function telemetry(event: string, data: () => unknown): void {
  if (!import.meta.env.DEV) return;
  console.log(`[telemetry] ${event}`, JSON.stringify(data()));
}
