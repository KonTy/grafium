export interface PageLoadState {
  version: number;
}

export interface PageLoadRequest {
  version: number;
  pageId: string;
  pageTitle: string;
}

export function createPageLoadState(): PageLoadState {
  return { version: 0 };
}

export function beginPageLoad(state: PageLoadState, pageId: string, pageTitle: string): PageLoadRequest {
  state.version += 1;
  return { version: state.version, pageId, pageTitle };
}

export function capturePageLoad(state: PageLoadState, pageId: string, pageTitle: string): PageLoadRequest {
  return { version: state.version, pageId, pageTitle };
}

export function isCurrentPageLoad(state: PageLoadState, request: Pick<PageLoadRequest, "version">): boolean {
  return state.version === request.version;
}

export async function applyIfCurrentPageLoad<T>(
  state: PageLoadState,
  request: Pick<PageLoadRequest, "version">,
  load: () => Promise<T>,
  apply: (value: T) => void
): Promise<boolean> {
  const value = await load();
  if (!isCurrentPageLoad(state, request)) {
    return false;
  }
  apply(value);
  return true;
}
