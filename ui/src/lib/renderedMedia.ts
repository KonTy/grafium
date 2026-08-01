import { hydrateAssetMedia } from "./markdown";

export type HydrateAssetMediaFn = (
  root: HTMLElement | null | undefined
) => Promise<void> | void;

export function queueHydrateAssetMedia(
  root: HTMLElement | null | undefined,
  hydrate: HydrateAssetMediaFn = hydrateAssetMedia
): void {
  queueMicrotask(() => {
    void hydrate(root);
  });
}

export function createHydrateRenderedMediaAction(
  hydrate: HydrateAssetMediaFn = hydrateAssetMedia
) {
  return (node: HTMLElement, _content?: unknown) => {
    queueHydrateAssetMedia(node, hydrate);
    return {
      update(_nextContent?: unknown) {
        queueHydrateAssetMedia(node, hydrate);
      },
    };
  };
}

export const hydrateRenderedMedia = createHydrateRenderedMediaAction();
