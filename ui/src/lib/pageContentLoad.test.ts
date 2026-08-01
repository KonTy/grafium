import { describe, expect, it } from "vitest";
import {
  applyIfCurrentPageLoad,
  beginPageLoad,
  createPageLoadState,
} from "./pageContentLoad";

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

describe("page content load guards", () => {
  it("keeps only the latest navigation result when overlapping loads resolve out of order", async () => {
    const state = createPageLoadState();
    const renderedPages: string[] = [];

    const pageA = beginPageLoad(state, "page-a", "Page A");
    const slowA = applyIfCurrentPageLoad(
      state,
      pageA,
      async () => {
        await wait(20);
        return "Page A blocks";
      },
      (value) => {
        renderedPages.push(value);
      }
    );

    const pageB = beginPageLoad(state, "page-b", "Page B");
    const fastB = applyIfCurrentPageLoad(
      state,
      pageB,
      async () => {
        await wait(1);
        return "Page B blocks";
      },
      (value) => {
        renderedPages.push(value);
      }
    );

    await Promise.all([slowA, fastB]);

    expect(renderedPages).toEqual(["Page B blocks"]);
  });
});
