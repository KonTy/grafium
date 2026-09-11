import { hydrateAssetMedia } from "./markdown";

export type HydrateAssetMediaFn = (
  root: HTMLElement | null | undefined
) => (() => void) | Promise<void> | void;

export function queueHydrateAssetMedia(
  root: HTMLElement | null | undefined,
  hydrate: HydrateAssetMediaFn = hydrateAssetMedia
): void {
  queueMicrotask(() => {
    hydrate(root);
  });
}

export function createHydrateRenderedMediaAction(
  hydrate: HydrateAssetMediaFn = hydrateAssetMedia
) {
  return (node: HTMLElement, _content?: unknown) => {
    let cleanup: (() => void) | void;
    queueMicrotask(() => {
      const result = hydrate(node);
      if (typeof result === "function") cleanup = result;
    });
    return {
      update(_nextContent?: unknown) {
        cleanup?.();
        cleanup = undefined;
        queueMicrotask(() => {
          const result = hydrate(node);
          if (typeof result === "function") cleanup = result;
        });
      },
      destroy() {
        cleanup?.();
      },
    };
  };
}

export const hydrateRenderedMedia = createHydrateRenderedMediaAction();
