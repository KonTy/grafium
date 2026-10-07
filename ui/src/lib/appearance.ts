import { writable } from "svelte/store";
import { listen } from "@tauri-apps/api/event";
import { getAppTheme, getSystemAppearance, setAppTheme, setStartupChrome, type SystemAppearance } from "./api";
import { applyTheme, getThemeById, systemThemeColors } from "./themes";

interface AppearanceDependencies {
  readPreference: () => Promise<string>;
  savePreference: (id: string) => Promise<void>;
  readSystem: () => Promise<SystemAppearance>;
  listen: (refresh: () => void) => Promise<() => void>;
  /** Optional: records the background the native window should launch with. */
  syncNativeChrome?: (background: string) => Promise<void>;
}

const opaqueSystem: SystemAppearance = {
  themeName: null,
  backgroundOpacity: 1,
  nativeTransparency: false,
};

function resolveAutoTheme(system: SystemAppearance, fallbackId: string) {
  let error = system.paletteError ?? "";
  if (system.palette) {
    try {
      return { colors: systemThemeColors(system.palette, system.themeName), known: true, error };
    } catch (cause) {
      console.error("[theme] Invalid system palette:", cause);
      error = "The system palette is invalid; using an opaque built-in palette.";
    }
  }
  const known = getThemeById(system.themeName ?? fallbackId);
  if (!known && !error) error = `Unknown system theme "${system.themeName}" has no usable palette; using opaque ${fallbackId}.`;
  return { colors: (known ?? getThemeById(fallbackId)!).colors, known: !!known, error };
}

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
  let nativeChrome = "";
  const state = writable({ preference, system, autoColors: getThemeById(fallbackId)!.colors, error: "" });

  // Android paints its launch window from a file written here, so the colour
  // only has to survive until the next start. Failing to record it costs a
  // brief mismatched flash, never the theme itself, so it stays non-fatal.
  function syncNativeChrome(background: string) {
    if (background === nativeChrome || !deps.syncNativeChrome) return;
    nativeChrome = background;
    deps.syncNativeChrome(background).catch((error) => {
      console.warn("[theme] Could not record the native window background:", error);
    });
  }

  function apply(error = "") {
    const auto = resolveAutoTheme(system, fallbackId);
    const manual = getThemeById(preference);
    const paletteError = preference === "auto" ? auto.error
      : !manual ? `Unknown theme "${preference}"; using opaque ${fallbackId}.` : "";
    if (paletteError) console.warn(`[theme] ${paletteError}`);
    const colors = preference === "auto" ? auto.colors : (manual ?? getThemeById(fallbackId)!).colors;
    const opacity = preference === "auto" && system.themeName && auto.known && !auto.error && system.nativeTransparency
      ? system.backgroundOpacity : 1;
    const transparencyError = preference === "auto" && system.themeName && auto.known
      && system.backgroundOpacity < 1 && !system.nativeTransparency
      ? system.transparencyUnavailableReason : "";
    applyTheme(colors, opacity);
    syncNativeChrome(colors.bgPrimary);
    state.set({ preference, system, autoColors: auto.colors,
      error: [watchError, preferenceError, error, paletteError, transparencyError].filter(Boolean).join(" ") });
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
  syncNativeChrome: setStartupChrome,
}, typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent) ? "oled" : "github");
