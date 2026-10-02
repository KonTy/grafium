import { readerNavigationKey } from "../readerNavigation";
import { readerShortcut } from "../readerHotkeys";

const interactive = "a,button,input,textarea,select,[contenteditable],[role=button],[role=link]";
const selected = doc => !!doc.getSelection()?.toString();

export function installReaderInteractions(doc, { send, turn, canSwipe = () => true, canPage = () => true,
  scrolled = () => false, scrollAtBoundary = () => false, atScrollBoundary = () => false, scrollContainer = null }) {
  let touch;
  let scrollTouch;
  let tap;
  let tapTimer;
  let wheelAmount = 0;
  let wheelUntil = 0;
  let lastWheel = 0;
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
  const verticallyScrollable = target => {
    for (let el = target; el && el !== doc.body && el !== doc.documentElement; el = el.parentElement) {
      if (el === scrollContainer) break;
      if (el.scrollHeight > el.clientHeight + 2
        && /auto|scroll/.test(doc.defaultView.getComputedStyle(el).overflowY)) return true;
    }
    return false;
  };
  const listeners = [];
  const on = (type, listener, options = true) => {
    doc.addEventListener(type, listener, options);
    listeners.push(() => doc.removeEventListener(type, listener, options));
  };
  on("keydown", event => {
    if (!event.isTrusted) return;
    const shortcut = readerShortcut(event);
    if (shortcut) {
      event.preventDefault(); event.stopImmediatePropagation(); send(shortcut); return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
    const direction = readerNavigationKey(event, doc);
    if (direction) {
      event.preventDefault(); event.stopImmediatePropagation();
      send("navigation"); turn(direction); return;
    }
    if (event.isComposing || event.defaultPrevented) return;
    const type = { F1: "help", F8: "toggle-controls", Escape: "exit-fullscreen", F11: "toggle-fullscreen" }[event.key];
    if (!type) return;
    event.preventDefault(); event.stopPropagation();
    if (!event.repeat) send(type);
  });
  on("wheel", event => {
    if (!event.isTrusted || event.defaultPrevented || !canPage() || !event.deltaY || event.ctrlKey || event.metaKey
      || event.altKey || event.shiftKey || event.buttons || blocked(event.target) || zoomed()
      || Math.abs(event.deltaX) > Math.abs(event.deltaY) || verticallyScrollable(event.target)) return;
    if (scrolled()) {
      send("navigation");
      if (scrollAtBoundary(event.deltaY > 0 ? "next" : "prev")) event.preventDefault();
      return;
    }
    event.preventDefault();
    if (event.timeStamp < wheelUntil) return;
    if (event.timeStamp - lastWheel > 250 || Math.sign(wheelAmount) !== Math.sign(event.deltaY)) wheelAmount = 0;
    lastWheel = event.timeStamp;
    wheelAmount += event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? doc.defaultView.innerHeight : 1);
    if (Math.abs(wheelAmount) < 40) return;
    send("navigation"); turn(wheelAmount > 0 ? "next" : "prev");
    wheelAmount = 0; wheelUntil = event.timeStamp + 180;
  }, { capture: true, passive: false });
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
    scrollTouch = event.isTrusted && scrolled() && event.touches.length === 1
      && !selected(doc) && !zoomed() && !blocked(event.target) && !verticallyScrollable(event.target)
      ? { x: event.touches[0].clientX, y: event.touches[0].clientY,
        next: atScrollBoundary("next"), prev: atScrollBoundary("prev") } : null;
    if (!event.isTrusted || event.touches.length !== 1 || selected(doc) || zoomed()
      || blocked(event.target) || horizontallyScrollable(event.target) || !canSwipe()) { touch = null; return; }
    const t = event.touches[0];
    touch = { id: t.identifier, x: t.clientX, y: t.clientY, time: event.timeStamp };
  }, options);
  on("touchmove", event => {
    event.stopImmediatePropagation();
    if (scrollTouch) {
      if (event.touches.length !== 1 || selected(doc) || zoomed()) scrollTouch = null;
      else if (Math.abs(event.touches[0].clientY - scrollTouch.y) > 12) send("navigation");
    }
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
    if (scrollTouch && event.isTrusted && !event.touches.length && event.changedTouches.length === 1
      && !selected(doc) && !zoomed()) {
      const dx = event.changedTouches[0].clientX - scrollTouch.x;
      const dy = event.changedTouches[0].clientY - scrollTouch.y;
      const direction = dy < 0 ? "next" : "prev";
      if (Math.abs(dy) > 48 && Math.abs(dy) > Math.abs(dx) * 1.7
        && scrollTouch[direction] && scrollAtBoundary(direction) && event.cancelable) event.preventDefault();
    }
    scrollTouch = null;
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
  on("touchcancel", event => { event.stopImmediatePropagation(); touch = null; scrollTouch = null; tap = null; }, options);
  return () => { clearTimeout(tapTimer); listeners.forEach(remove => remove()); };
}
