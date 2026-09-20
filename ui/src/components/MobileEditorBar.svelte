<script lang="ts">
  import { onMount } from "svelte";

  interface Props {
    onTodo: () => void;
    onTime: () => void;
    onTimeLocation: () => void;
    onOutdent: () => void;
    onIndent: () => void;
    onLink: () => void;
    onTag: () => void;
    onSlash: () => void;
    onHide: () => void;
  }

  let {
    onTodo,
    onTime,
    onTimeLocation,
    onOutdent,
    onIndent,
    onLink,
    onTag,
    onSlash,
    onHide,
  }: Props = $props();

  let host: HTMLDivElement | undefined;
  let keyboardInset = $state(0);
  let timePressTimer: ReturnType<typeof setTimeout> | undefined;
  let timeLongPressed = false;
  const LOCATION_LONG_PRESS_MS = 600;

  function keepFocus(event: Event) {
    event.preventDefault();
  }

  function press(action: () => void) {
    return (event: PointerEvent) => {
      event.preventDefault();
      action();
    };
  }

  function clearTimePress() {
    if (timePressTimer !== undefined) clearTimeout(timePressTimer);
    timePressTimer = undefined;
  }

  function startTimePress(event: PointerEvent) {
    event.preventDefault();
    clearTimePress();
    timeLongPressed = false;
    if (Number.isInteger(event.pointerId)) {
      try {
        (event.currentTarget as HTMLButtonElement).setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture is best-effort; pointercancel still clears the timer.
      }
    }
    timePressTimer = setTimeout(() => {
      timePressTimer = undefined;
      timeLongPressed = true;
      onTimeLocation();
    }, LOCATION_LONG_PRESS_MS);
  }

  function finishTimePress(event: PointerEvent) {
    event.preventDefault();
    const shouldInsertTime = timePressTimer !== undefined && !timeLongPressed;
    clearTimePress();
    if (shouldInsertTime) onTime();
    timeLongPressed = false;
  }

  function cancelTimePress(event: PointerEvent) {
    event.preventDefault();
    clearTimePress();
    timeLongPressed = false;
  }

  function keyboardOffset(): number {
    const viewport = window.visualViewport;
    if (!viewport) return 0;
    return Math.max(0, window.innerHeight - viewport.height - viewport.offsetTop);
  }

  onMount(() => {
    if (host) document.body.appendChild(host);
    const viewport = window.visualViewport;
    const update = () => {
      keyboardInset = keyboardOffset();
    };
    update();
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);
    window.addEventListener("resize", update);
    return () => {
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      clearTimePress();
      host?.remove();
    };
  });

  const bottomStyle = $derived(
    keyboardInset > 48
      ? `${keyboardInset}px`
      : "calc(56px + env(safe-area-inset-bottom, 0px))",
  );
</script>

<div
  bind:this={host}
  class="mobile-editor-bar"
  style={`bottom: ${bottomStyle};`}
  role="toolbar"
  tabindex="-1"
  aria-label="Editor"
  onpointerdown={keepFocus}
>
  <button type="button" title="TODO" aria-label="Turn into TODO" onpointerdown={press(onTodo)}>TODO</button>
  <button
    type="button"
    class="time"
    title="Insert current time; hold for time and location"
    aria-label="Insert current time; hold for time and location"
    onpointerdown={startTimePress}
    onpointerup={finishTimePress}
    onpointercancel={cancelTimePress}
    onlostpointercapture={cancelTimePress}
    oncontextmenu={(event) => event.preventDefault()}
  >Time</button>
  <button type="button" title="Unindent" aria-label="Unindent" onpointerdown={press(onOutdent)}>⇤</button>
  <button type="button" title="Indent" aria-label="Indent" onpointerdown={press(onIndent)}>⇥</button>
  <button type="button" title="Page link" aria-label="Insert page link" onpointerdown={press(onLink)}>[[ ]]</button>
  <button type="button" title="Tag" aria-label="Insert tag" onpointerdown={press(onTag)}>#</button>
  <button type="button" title="Slash command" aria-label="Slash command" onpointerdown={press(onSlash)}>/</button>
  <button type="button" class="hide" title="Hide keyboard" aria-label="Hide keyboard" onpointerdown={press(onHide)}>⌄</button>
</div>

<style>
  .mobile-editor-bar {
    display: none;
  }

  @media (max-width: 640px) {
    .mobile-editor-bar {
      position: fixed;
      left: 0;
      right: 0;
      z-index: 240;
      display: flex;
      align-items: stretch;
      gap: 2px;
      height: 44px;
      padding: 0 4px;
      overflow-x: auto;
      overscroll-behavior: contain;
      background: color-mix(in srgb, var(--bg-secondary, #1a1b26) 92%, transparent);
      border-top: 1px solid var(--border);
      backdrop-filter: blur(10px);
      -webkit-overflow-scrolling: touch;
    }

    .mobile-editor-bar button {
      flex: 0 0 auto;
      min-width: 44px;
      border: none;
      background: transparent;
      color: var(--text-primary);
      font: 600 0.78rem/1 system-ui, sans-serif;
      padding: 0 10px;
      touch-action: manipulation;
    }

    .mobile-editor-bar button.hide {
      margin-left: auto;
      font-size: 1.25rem;
      font-weight: 400;
    }

    .mobile-editor-bar button.time {
      touch-action: none;
      user-select: none;
      -webkit-touch-callout: none;
    }
  }
</style>
