const interactive = "a,button,input,textarea,select,[contenteditable],[role=button],[role=link]";
const selected = doc => !!doc.getSelection()?.toString();

export function installReaderInteractions(doc, { send, turn, canSwipe = () => true }) {
  let touch;
  let tap;
  let tapTimer;
  const zoomed = () => (window.visualViewport?.scale ?? 1) > 1.01
    || (doc.defaultView.visualViewport?.scale ?? 1) > 1.01;
  const blocked = target => target?.closest?.(interactive);
  const horizontallyScrollable = target => {
    for (let el = target; el && el !== doc.documentElement; el = el.parentElement) {
      if (el.scrollWidth > el.clientWidth + 2
        && /auto|scroll/.test(doc.defaultView.getComputedStyle(el).overflowX)) return true;
    }
    return false;
  };
  const listeners = [];
  const on = (type, listener, options = true) => {
    doc.addEventListener(type, listener, options);
    listeners.push(() => doc.removeEventListener(type, listener, options));
  };
  on("keydown", event => {
    if (!event.isTrusted || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
    const type = { F1: "help", F8: "toggle-controls", Escape: "exit-fullscreen", F11: "toggle-fullscreen" }[event.key];
    if (!type) return;
    event.preventDefault(); event.stopPropagation();
    if (!event.repeat) send(type);
  });
  on("pointerdown", event => {
    clearTimeout(tapTimer);
    tap = event.isTrusted && event.isPrimary && event.button === 0 && !blocked(event.target)
      && !selected(doc) && !zoomed()
      ? { x: event.clientX, y: event.clientY, time: event.timeStamp } : null;
  });
  on("pointermove", event => {
    if (tap && Math.hypot(event.clientX - tap.x, event.clientY - tap.y) > 10) tap = null;
  });
  on("pointercancel", () => { tap = null; });
  on("selectionchange", () => {
    if (selected(doc)) { tap = null; touch = null; clearTimeout(tapTimer); }
  });
  on("click", event => {
    const start = tap;
    tap = null;
    if (!event.isTrusted || !start || event.detail !== 1 || event.timeStamp - start.time > 400
      || blocked(event.target) || selected(doc) || zoomed()) return;
    // Let a second click/long selection win over a controls toggle.
    tapTimer = setTimeout(() => { if (!selected(doc)) send("toggle-controls"); }, 260);
  });
  const options = { capture: true, passive: false };
  on("touchstart", event => {
    // Foliate's built-in drag/snap prevents every single-finger move, including
    // vertical scroll/selection. Replace that handler, keeping its goLeft/goRight
    // navigation semantics (not guessed page indexes, and RTL stays correct).
    event.stopImmediatePropagation();
    if (!event.isTrusted || event.touches.length !== 1 || selected(doc) || zoomed()
      || blocked(event.target) || horizontallyScrollable(event.target) || !canSwipe()) { touch = null; return; }
    const t = event.touches[0];
    touch = { id: t.identifier, x: t.clientX, y: t.clientY, time: event.timeStamp };
  }, options);
  on("touchmove", event => {
    event.stopImmediatePropagation();
    if (!touch) return;
    if (event.touches.length !== 1 || zoomed() || selected(doc) || event.timeStamp - touch.time > 600) { touch = null; return; }
    const t = [...event.touches].find(t => t.identifier === touch.id);
    if (!t) { touch = null; return; }
    const dx = Math.abs(t.clientX - touch.x), dy = Math.abs(t.clientY - touch.y);
    if (dy > 12 && dy > dx) { touch = null; return; }
    if (dx > 16 && dx > dy * 1.7 && event.cancelable) event.preventDefault();
  }, options);
  on("touchend", event => {
    event.stopImmediatePropagation();
    const start = touch;
    touch = null;
    if (!event.isTrusted || !start || event.touches.length || selected(doc) || zoomed() || !canSwipe()
      || event.timeStamp - start.time > 600) return;
    const t = [...event.changedTouches].find(t => t.identifier === start.id);
    if (!t) return;
    const dx = t.clientX - start.x, dy = t.clientY - start.y;
    if (Math.abs(dx) < 48 || Math.abs(dx) < Math.abs(dy) * 1.7) return;
    tap = null; clearTimeout(tapTimer);
    if (event.cancelable) event.preventDefault();
    send("navigation");
    turn(dx < 0 ? "right" : "left");
  }, options);
  on("touchcancel", event => { event.stopImmediatePropagation(); touch = null; tap = null; }, options);
  return () => { clearTimeout(tapTimer); listeners.forEach(remove => remove()); };
}
