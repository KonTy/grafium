export const ANDROID_BACK_EVENT = "grafium-android-back";

export function dismissOverlayForAndroidBack(target: EventTarget): boolean {
  const escape = new KeyboardEvent("keydown", {
    key: "Escape",
    code: "Escape",
    bubbles: true,
    cancelable: true,
  });
  target.dispatchEvent(escape);
  return escape.defaultPrevented;
}
