export interface SidebarSearchController<T> {
  submit: (query: string) => void;
  cancel: () => void;
}

interface SidebarSearchControllerOptions<T> {
  debounceMs?: number;
  run: (query: string) => Promise<T>;
  apply: (query: string, value: T) => void;
  clear: () => void;
}

export function createSidebarSearchController<T>({
  debounceMs = 120,
  run,
  apply,
  clear,
}: SidebarSearchControllerOptions<T>): SidebarSearchController<T> {
  let version = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const clearTimer = () => {
    if (timer === null) return;
    clearTimeout(timer);
    timer = null;
  };

  const cancel = () => {
    version += 1;
    clearTimer();
  };

  return {
    submit(query: string) {
      const trimmed = query.trim();
      version += 1;
      const requestVersion = version;
      clearTimer();

      if (!trimmed) {
        clear();
        return;
      }

      timer = setTimeout(() => {
        timer = null;
        void run(trimmed).then((value) => {
          if (version !== requestVersion) return;
          apply(trimmed, value);
        });
      }, debounceMs);
    },
    cancel,
  };
}
