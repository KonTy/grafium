import type { Page } from "./api";

export type PageNavigationTarget = string | { id: string } | { title: string };

export type GetPageFn = (opts: { id?: string; title?: string }) => Promise<Page>;

export function resolvePageLookup(target: PageNavigationTarget): { id?: string; title?: string } {
  if (typeof target === "string") {
    return { title: target };
  }
  if ("id" in target) {
    return { id: target.id };
  }
  return { title: target.title };
}

export function loadPageForNavigation(target: PageNavigationTarget, getPageFn: GetPageFn): Promise<Page> {
  return getPageFn(resolvePageLookup(target));
}
