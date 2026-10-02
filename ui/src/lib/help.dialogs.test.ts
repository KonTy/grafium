import { describe, expect, it, vi } from "vitest";
import { closeSettingsHelpForContextualHelp } from "./help";

describe("contextual help over native dialogs", () => {
  it.each(["data-settings-help-dialog", "data-reader-menu"])("dismisses an open %s through its cancel handler", attribute => {
    const dialog = document.createElement("dialog");
    dialog.setAttribute(attribute, "");
    dialog.open = true;
    const button = document.createElement("button");
    dialog.append(button);
    const close = vi.fn((event: Event) => {
      expect(event.cancelable).toBe(true);
      event.preventDefault();
      dialog.open = false;
    });
    dialog.addEventListener("cancel", close);
    closeSettingsHelpForContextualHelp(button);
    expect(close).toHaveBeenCalledOnce();
    expect(dialog.open).toBe(false);
  });

  it("leaves closed reader menus and unrelated native dialogs alone", () => {
    for (const readerMenu of [true, false]) {
      const dialog = document.createElement("dialog");
      dialog.open = !readerMenu;
      if (readerMenu) dialog.setAttribute("data-reader-menu", "");
      const cancel = vi.fn();
      dialog.addEventListener("cancel", cancel);
      closeSettingsHelpForContextualHelp(dialog);
      expect(cancel).not.toHaveBeenCalled();
    }
    expect(() => closeSettingsHelpForContextualHelp(null)).not.toThrow();
  });
});
