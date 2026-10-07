/// Startup timings stop at the Rust/webview boundary, which on a phone is
/// under a third of the wait. These marks cover the rest: how long the boot
/// bundle takes to evaluate, how long Svelte takes to mount, and how long the
/// first page takes to arrive. They are reported once, with the native phases.
export type BootPhase = { name: string; ms: number };

type BootWindow = Window & { __grafiumBoot?: BootPhase[] };

function store(): BootPhase[] {
  const w = window as BootWindow;
  if (!w.__grafiumBoot) w.__grafiumBoot = [];
  return w.__grafiumBoot;
}

/// `performance.now()` counts from navigation start, so every mark shares one
/// origin without having to agree with the Rust clock.
export function markBoot(name: string): void {
  store().push({ name, ms: performance.now() });
}

export function bootPhases(): BootPhase[] {
  return store().slice();
}

export function resetBootMarksForTests(): void {
  (window as BootWindow).__grafiumBoot = [];
}
