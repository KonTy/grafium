import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  ANDROID_BACK_EVENT,
  dismissOverlayForAndroidBack,
} from "./androidBack";

describe("Android back navigation", () => {
  it("lets the focused overlay consume Back as Escape", () => {
    const target = new EventTarget();
    target.addEventListener("keydown", (event) => event.preventDefault());

    expect(dismissOverlayForAndroidBack(target)).toBe(true);
  });

  it("falls through when no overlay consumes Escape", () => {
    expect(dismissOverlayForAndroidBack(new EventTarget())).toBe(false);
  });

  it("keeps the native and frontend event names connected", () => {
    const activity = readFileSync(
      join(process.cwd(), "src-tauri/android/MainActivity.kt"),
      "utf8",
    );
    const app = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");

    expect(activity).toContain(`CustomEvent('${ANDROID_BACK_EVENT}')`);
    expect(app).toContain("window.addEventListener(ANDROID_BACK_EVENT, handleAndroidBack)");
    expect(app).toContain("if (navIndex > 0) void goBack()");
  });
});
