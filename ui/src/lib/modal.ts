/**
 * Shared behaviour for the app's modal dialogs.
 *
 * Every dialog in the app is a presentational backdrop wrapping a dialog box.
 * Historically the backdrop carried a bare `onclick` to dismiss the dialog and
 * the box carried an `onclick` that did nothing but `stopPropagation()` to stop
 * that dismissal from firing on clicks inside. That pairing is an
 * accessibility problem (a click target with no keyboard equivalent) and it
 * does not even close the hole it was aimed at: when a press and its release
 * land on different elements the browser dispatches `click` at their nearest
 * common ancestor, which for a drag out of the box *is* the backdrop. So
 * selecting text in a dialog and releasing slightly outside it discarded the
 * dialog, typed input and all.
 *
 * `dismissOnBackdrop` closes both holes. It requires the press *and* the
 * release to be on the backdrop itself, so a drag that started inside the box
 * can never dismiss it, and it is applied as an action rather than an event
 * attribute so the backdrop stays a genuinely non-interactive element.
 * Keyboard users dismiss with Escape via `dialogKeydown` on the dialog box.
 *
 * The listeners are plain `addEventListener` calls, but they never call
 * `stopPropagation`, so Svelte's delegated handlers inside the dialog are
 * unaffected.
 */
export function dismissOnBackdrop(node: HTMLElement, close: () => void) {
  let current = close;
  let pressedOnBackdrop = false;

  function onPointerDown(event: PointerEvent) {
    pressedOnBackdrop = event.target === node;
  }

  function onClick(event: MouseEvent) {
    const armed = pressedOnBackdrop;
    pressedOnBackdrop = false;
    if (armed && event.target === node) current();
  }

  node.addEventListener("pointerdown", onPointerDown);
  node.addEventListener("click", onClick);

  return {
    update(next: () => void) {
      current = next;
    },
    destroy() {
      node.removeEventListener("pointerdown", onPointerDown);
      node.removeEventListener("click", onClick);
    },
  };
}

/**
 * Keydown handler for a modal dialog box: Escape closes it, Tab stays inside.
 *
 * Applied to the dialog box so it works wherever focus sits inside the dialog,
 * rather than only while one particular input happens to be focused. Escape is
 * stopped so a dialog layered over another view cannot dismiss both at once.
 *
 * The Tab wrap is what makes `aria-modal="true"` honest. That attribute tells a
 * screen reader everything outside the dialog is inert, so letting Tab walk
 * focus out into background content strands the user on nodes their reader
 * refuses to announce. `event.currentTarget` is the dialog box itself, which is
 * why this must be bound with `onkeydown` on the box and not on a child.
 *
 * Only the wrapping Tab presses are intercepted; Tab moves between the dialog's
 * own controls natively.
 */
const FOCUSABLE = [
  "a[href]",
  "button:not(:disabled)",
  "input:not(:disabled)",
  "select:not(:disabled)",
  "textarea:not(:disabled)",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

export function dialogKeydown(close: () => void) {
  return (event: KeyboardEvent) => {
    if (event.isComposing) return;

    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }

    if (event.key !== "Tab") return;

    const box = event.currentTarget as HTMLElement | null;
    if (!box) return;

    const focusable = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (element) => element.tabIndex >= 0 && element.getClientRects().length > 0,
    );

    // A dialog with nothing focusable inside still must not leak focus out.
    if (focusable.length === 0) {
      event.preventDefault();
      box.focus();
      return;
    }

    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement;

    if (event.shiftKey && (active === first || active === box)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || active === box)) {
      event.preventDefault();
      first.focus();
    }
  };
}
