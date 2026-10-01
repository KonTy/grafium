import { writable } from "svelte/store";
import { listen } from "@tauri-apps/api/event";
import { getAppTheme, getSystemAppearance, setAppTheme, type SystemAppearance } from "./api";
import { applyTheme, getThemeById } from "./themes";

interface AppearanceDependencies {
  readPreference: () => Promise<string>;
  savePreference: (id: string) => Promise<void>;
  readSystem: () => Promise<SystemAppearance>;
  listen: (refresh: () => void) => Promise<() => void>;
}

const opaqueSystem: SystemAppearance = {
  themeName: null,
  backgroundOpacity: 1,
  nativeTransparency: false,
};

export function createAppearanceController(deps: AppearanceDependencies, fallbackId = "github") {
  let preference = fallbackId;
  let system = opaqueSystem;
  let revision = 0;
  let preferenceRevision = 0;
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let saves: Promise<void> = Promise.resolve();
  let watchError = "";
  let preferenceError = "";
  const state = writable({ preference, system, error: "" });

  function apply(error = "") {
    const requested = preference === "auto" ? system.themeName ?? fallbackId : preference;
    const known = getThemeById(requested);
    const theme = known ?? getThemeById(fallbackId)!;
    if (!known) console.warn(`[theme] Unknown theme "${requested}"; using opaque ${fallbackId}.`);
    const opacity = preference === "auto" && system.themeName && known && system.nativeTransparency
      ? system.backgroundOpacity : 1;
    applyTheme(theme.colors, opacity);
    state.set({ preference, system, error: [watchError, preferenceError, error].filter(Boolean).join(" ") });
  }

  async function refresh() {
    const request = ++revision;
    try {
      const next = await deps.readSystem();
      if (disposed || request !== revision) return;
      system = next;
      apply();
    } catch (error) {
      if (disposed || request !== revision) return;
      console.error("[theme] Could not read system appearance:", error);
      system = opaqueSystem;
      apply("Could not read system appearance; using an opaque background.");
    }
  }

  return {
    subscribe: state.subscribe,
    async start() {
      disposed = false;
      const initialPreferenceRevision = preferenceRevision;
      const registration = deps.listen(() => { void refresh(); }).then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      }).catch((error) => {
        console.error("[theme] Could not watch system appearance:", error);
        watchError = "Live theme updates are unavailable.";
        apply();
      });
      const restore = deps.readPreference().then((id) => {
        if (!disposed && initialPreferenceRevision === preferenceRevision) {
          preference = id;
          apply();
        }
      }).catch((error) => {
        console.error("[theme] Could not restore theme preference:", error);
        preferenceError = "Could not restore the theme; using the default palette.";
        if (!disposed) apply();
      });
      await registration;
      await Promise.all([restore, refresh()]);
    },
    refresh,
    async select(id: string) {
      const selection = ++preferenceRevision;
      preference = id;
      preferenceError = "";
      apply();
      // Serialize persistence so a slow earlier click cannot win on next launch.
      const save = saves.then(async () => {
        await deps.savePreference(id);
        if (!disposed && selection === preferenceRevision) {
          preferenceError = "";
          apply();
        }
      });
      saves = save.catch((error) => {
        console.error("[theme] Could not save theme preference:", error);
        preferenceError = "Could not save the theme preference.";
        if (!disposed) apply();
      });
      await save;
    },
    stop() {
      disposed = true;
      revision++;
      unlisten?.();
      unlisten = undefined;
    },
  };
}

export const appearance = createAppearanceController({
  readPreference: getAppTheme,
  savePreference: setAppTheme,
  readSystem: getSystemAppearance,
  listen: (refresh) => listen("smplos-theme-changed", refresh),
}, typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent) ? "oled" : "github");
