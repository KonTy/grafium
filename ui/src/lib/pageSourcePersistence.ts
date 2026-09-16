import { getGraphInfo, getPageSource, updatePageSource } from "./api";

export class PageSourceSession {
  private savedSource: string;
  private tail: Promise<void> = Promise.resolve();
  private invalidated = false;
  content: string;
  pending = 0;

  constructor(readonly pageId: string, readonly graphPath: string, source: string) {
    this.savedSource = source;
    this.content = source;
  }

  get expectedSource(): string { return this.savedSource; }
  get dirty(): boolean {
    return this.content.replace(/\r\n?/g, "\n") !== this.savedSource.replace(/\r\n?/g, "\n");
  }

  invalidate(): void { this.invalidated = true; }

  save(): Promise<void> {
    const content = this.content;
    this.pending += 1;
    const write = async () => {
      if (this.invalidated) throw new Error("The source editor changed. Your unsaved text has not been written.");
      if ((await getGraphInfo()).path !== this.graphPath || this.invalidated) {
        throw new Error("The graph changed. Return to the original graph before saving this source.");
      }
      await updatePageSource(this.pageId, content, {
        expectedSource: this.savedSource, graphPath: this.graphPath,
      });
      this.savedSource = content;
    };
    // A rejected save remains visible to its caller, but must not poison explicit retries.
    const result = this.tail.then(write, write).finally(() => { this.pending -= 1; });
    this.tail = result;
    return result;
  }
}

export async function loadPageSourceSession(pageId: string, expectedGraphPath?: string): Promise<PageSourceSession> {
  const graph = await getGraphInfo();
  if (!graph.path || (expectedGraphPath !== undefined && graph.path !== expectedGraphPath)) {
    throw new Error("The graph changed. Return to the original graph before reloading this source.");
  }
  const source = await getPageSource(pageId);
  if ((await getGraphInfo()).path !== graph.path) {
    throw new Error("The graph changed while loading the source. The editor was not replaced.");
  }
  return new PageSourceSession(pageId, graph.path, source);
}
