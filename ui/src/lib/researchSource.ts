import { getGraphInfo, getPage, listBlocks, type Block } from "./api";
import { flushPageEditors } from "./editorPersistence";

export interface ResearchSource {
  graphPath: string;
  pageId: string;
  pageTitle: string;
  afterBlockId: string | null;
  snapshot: Block[];
}

export async function captureResearchSource(pageId: string, afterBlockId: string | null): Promise<ResearchSource> {
  if (!pageId) throw new Error("Open a page or select a journal day first.");
  const graph = await getGraphInfo();
  await flushPageEditors(pageId);
  const page = await getPage({ id: pageId });
  const snapshot = await listBlocks(pageId);
  if ((await getGraphInfo()).path !== graph.path) {
    throw new Error("The graph changed while reading the source. Try again.");
  }
  return {
    graphPath: graph.path,
    pageId: page.id,
    pageTitle: page.title,
    afterBlockId: snapshot.some((block) => block.id === afterBlockId) ? afterBlockId : null,
    snapshot,
  };
}

export async function assertResearchSourceGraph(source: ResearchSource): Promise<void> {
  if ((await getGraphInfo()).path !== source.graphPath) {
    throw new Error("This result belongs to a different graph. Nothing was inserted.");
  }
}
