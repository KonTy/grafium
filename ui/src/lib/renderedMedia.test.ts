import { describe, expect, it, vi } from "vitest";
import {
  createHydrateRenderedMediaAction,
  queueHydrateAssetMedia,
} from "./renderedMedia";

describe("rendered media hydration", () => {
  it("queues hydration after rendered HTML is inserted", async () => {
    const hydrate = vi.fn(async () => {});
    const root = {} as HTMLElement;

    queueHydrateAssetMedia(root, hydrate);
    expect(hydrate).not.toHaveBeenCalled();

    await Promise.resolve();

    expect(hydrate).toHaveBeenCalledWith(root);
  });

  it("re-hydrates on action mount and update", async () => {
    const hydrate = vi.fn(async () => {});
    const root = {} as HTMLElement;
    const hydrateRenderedMedia = createHydrateRenderedMediaAction(hydrate);

    const action = hydrateRenderedMedia(root, "first");
    await Promise.resolve();
    action.update?.("second");
    await Promise.resolve();

    expect(hydrate).toHaveBeenCalledTimes(2);
    expect(hydrate).toHaveBeenNthCalledWith(1, root);
    expect(hydrate).toHaveBeenNthCalledWith(2, root);
  });
});
