export interface MutableBlockLike {
  id: string;
  content: string;
}

export interface SaveContext {
  blockId: string;
  pageId: string;
  changed: boolean;
  before: string;
  after: string;
}

export type UpdateBlockFn = (id: string, content: string) => Promise<void>;

export function buildSaveContext(blockId: string, pageId: string, before: string, after: string): SaveContext {
  return {
    blockId,
    pageId,
    changed: after !== before,
    before: before.slice(0, 80),
    after: after.slice(0, 80),
  };
}

export async function persistBlockContentIfChanged(
  block: MutableBlockLike,
  nextContent: string,
  updateBlockFn: UpdateBlockFn
): Promise<boolean> {
  if (nextContent === block.content) {
    return false;
  }

  await updateBlockFn(block.id, nextContent);
  block.content = nextContent;
  return true;
}

export async function persistThen(
  persist: () => Promise<void>,
  structuralOp: () => Promise<void>
): Promise<void> {
  await persist();
  await structuralOp();
}
