// Theme definitions derived from smplOS colors.toml files.
// Each theme maps smplos color tokens to Grafium CSS variable values.

export interface ThemeColors {
  bgPrimary: string;
  bgSecondary: string;
  bgSidebar: string;
  bgHover: string;
  bgActive: string;
  bgInput: string;
  bgCode: string;

  textPrimary: string;
  textSecondary: string;
  textMuted: string;

  border: string;

  accent: string;
  accentSecondary: string;
  textLink: string;
  textLinkHover: string;
  textLinkVisited: string;

  danger: string;

  btnBg: string;
  btnBgHover: string;
  btnPrimaryBg: string;
  btnPrimaryFg: string;
  btnPrimaryHover: string;
  surfaceRaised: string;
  surfaceOverlay: string;

  taskTodoBg: string;
  taskTodoFg: string;
  taskDoingBg: string;
  taskDoingFg: string;
  taskDoneBg: string;
  taskDoneFg: string;
  taskLaterBg: string;
  taskLaterFg: string;

  isLight: boolean;

  /**
   * Optional visual-effects preset. When set, applyTheme adds a
   * `theme-fx-<fx>` class to <html> so global.css can layer on extra styling
   * (futuristic fonts, neon glow, scanlines, etc.) beyond plain colors.
   */
  fx?: string;
}

export interface Theme {
  id: string;
  name: string;
  colors: ThemeColors;
}

function normalizeThemeId(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^\w\s-]/g, "")
    .replace(/[\s_]+/g, "-");
}

function dark(bg: string, bgLight: string, bgLighter: string, fg: string, fgDim: string, muted: string, accent: string, accentAlt: string, danger: string, success: string, warning: string): ThemeColors {
  return {
    bgPrimary: bg,
    bgSecondary: bgLight,
    bgSidebar: bg,
    bgHover: bgLighter,
    bgActive: bgLighter,
    bgInput: bgLight,
    bgCode: bgLighter,
    textPrimary: fg,
    textSecondary: fgDim,
    textMuted: muted,
    border: bgLighter,
    accent,
    accentSecondary: success,
    textLink: accentAlt,
    textLinkHover: warning,
    textLinkVisited: accent,
    danger,
    btnBg: bgLighter,
    btnBgHover: bgLighter,
    btnPrimaryBg: accent,
    btnPrimaryFg: bg,
    btnPrimaryHover: accent,
    surfaceRaised: bgLighter,
    surfaceOverlay: bgLight,
    taskTodoBg: warning + "22",
    taskTodoFg: warning,
    taskDoingBg: accent + "22",
    taskDoingFg: accent,
    taskDoneBg: success + "22",
    taskDoneFg: success,
    taskLaterBg: accentAlt + "22",
    taskLaterFg: accentAlt,
    isLight: false,
  };
}

function light(bg: string, bgLight: string, bgLighter: string, fg: string, fgDim: string, muted: string, accent: string, accentAlt: string, danger: string, success: string, warning: string): ThemeColors {
  return {
    bgPrimary: bg,
    bgSecondary: bgLight,
    bgSidebar: bgLight,
    bgHover: bgLighter,
    bgActive: bgLighter,
    bgInput: bg,
    bgCode: bgLighter,
    textPrimary: fg,
    textSecondary: fgDim,
    textMuted: muted,
    border: bgLighter,
    accent,
    accentSecondary: success,
    textLink: accentAlt,
    textLinkHover: warning,
    textLinkVisited: accent,
    danger,
    btnBg: bgLighter,
    btnBgHover: bgLighter,
    btnPrimaryBg: accent,
    btnPrimaryFg: bg,
    btnPrimaryHover: accent,
    surfaceRaised: bgLight,
    surfaceOverlay: bg,
    taskTodoBg: warning + "22",
    taskTodoFg: warning,
    taskDoingBg: accent + "22",
    taskDoingFg: accent,
    taskDoneBg: success + "22",
    taskDoneFg: success,
    taskLaterBg: accentAlt + "22",
    taskLaterFg: accentAlt,
    isLight: true,
  };
}

export const themes: Theme[] = [
  {
    id: "catppuccin",
    name: "Catppuccin",
    colors: dark("#1e1e2e", "#45475a", "#585b70", "#cdd6f4", "#cdd6f4", "#585b70", "#89b4fa", "#f5c2e7", "#f38ba8", "#a6e3a1", "#f9e2af"),
  },
  {
    id: "catppuccin-latte",
    name: "Catppuccin Latte",
    colors: light("#eff1f5", "#dce0e8", "#ccd0da", "#4c4f69", "#6c6f85", "#acb0be", "#1e66f5", "#ea76cb", "#d20f39", "#40a02b", "#df8e1d"),
  },
  {
    id: "tokyo-night",
    name: "Tokyo Night",
    colors: dark("#1a1b26", "#24283b", "#32344a", "#a9b1d6", "#acb0d0", "#444b6a", "#7aa2f7", "#ad8ee6", "#f7768e", "#9ece6a", "#e0af68"),
  },
  {
    id: "ethereal",
    name: "Ethereal",
    colors: dark("#060B1E", "#141932", "#1e244a", "#ffcead", "#ffcead", "#6d7db6", "#7d82d9", "#c89dc1", "#ED5B5A", "#92a593", "#E9BB4F"),
  },
  {
    id: "everforest",
    name: "Everforest",
    colors: dark("#2d353b", "#374145", "#475258", "#d3c6aa", "#d3c6aa", "#475258", "#7fbbb3", "#d699b6", "#e67e80", "#a7c080", "#dbbc7f"),
  },
  {
    id: "flexoki-light",
    name: "Flexoki Light",
    colors: light("#FFFCF0", "#E6E4D9", "#DAD8CE", "#100F0F", "#878580", "#B7B5AC", "#205EA6", "#CE5D97", "#D14D41", "#879A39", "#D0A215"),
  },
  {
    id: "gruvbox",
    name: "Gruvbox",
    colors: dark("#282828", "#3c3836", "#504945", "#d4be98", "#d4be98", "#3c3836", "#7daea3", "#d3869b", "#ea6962", "#a9b665", "#d8a657"),
  },
  {
    id: "hackerman",
    name: "Hackerman",
    colors: dark("#0B0C16", "#181a2a", "#252840", "#ddf7ff", "#ddf7ff", "#6a6e95", "#82FB9C", "#86a7df", "#50f872", "#4fe88f", "#50f7d4"),
  },
  {
    id: "kanagawa",
    name: "Kanagawa",
    colors: dark("#1f1f28", "#2a2a37", "#363646", "#dcd7ba", "#dcd7ba", "#727169", "#7e9cd8", "#957fb8", "#c34043", "#76946a", "#c0a36e"),
  },
  {
    id: "matrix",
    name: "Matrix",
    colors: {
      ...dark("#000000", "#0D1A0D", "#1A2E1A", "#00FF00", "#66FF66", "#55BB55", "#00FF00", "#FF9900", "#FF9900", "#00FF00", "#FFCC33"),
      fx: "syphi",
    },
  },
  {
    id: "amber",
    name: "Amber",
    colors: dark("#070604", "#15110A", "#231B10", "#FFB347", "#E4A147", "#9B6D2A", "#8FC5FF", "#A9D1FF", "#FF8F1F", "#FFD166", "#FFCF66"),
  },
  {
    id: "matte-black",
    name: "Matte Black",
    colors: dark("#121212", "#1e1e1e", "#333333", "#bebebe", "#ffffff", "#8a8a8d", "#e68e0d", "#D35F5F", "#D35F5F", "#FFC107", "#b91c1c"),
  },
  {
    id: "nord",
    name: "Nord",
    colors: dark("#2e3440", "#3b4252", "#4c566a", "#d8dee9", "#eceff4", "#4c566a", "#81a1c1", "#b48ead", "#bf616a", "#a3be8c", "#ebcb8b"),
  },
  {
    id: "osaka-jade",
    name: "Osaka Jade",
    colors: dark("#111c18", "#1a2b22", "#23372B", "#C1C497", "#9eebb3", "#53685B", "#509475", "#D2689C", "#FF5345", "#549e6a", "#459451"),
  },
  {
    id: "ristretto",
    name: "Ristretto",
    colors: dark("#2c2525", "#3d3535", "#4e4444", "#e6d9db", "#f1e5e7", "#948a8b", "#f38d70", "#a8a9eb", "#fd6883", "#adda78", "#f9cc6c"),
  },
  {
    id: "rose-pine",
    name: "Rosé Pine",
    colors: light("#faf4ed", "#f2e9e1", "#e4dcd4", "#575279", "#575279", "#9893a5", "#56949f", "#907aa9", "#b4637a", "#286983", "#ea9d34"),
  },
  {
    // Experimental sci-fi / matrix look: neon cyan + green on near-black,
    // futuristic display font, glowing text, scanline overlay. The `fx` flag
    // activates the extra styling defined under `.theme-fx-syphi` in global.css.
    id: "syphi",
    name: "Syphi (Futuristic)",
    colors: {
      ...dark("#02050a", "#061018", "#0c2130", "#c8fff4", "#7ff0e0", "#3d7a72", "#00f0ff", "#39ff88", "#ff2e6b", "#39ff88", "#ffd23f"),
      fx: "syphi",
    },
  },
];

export function getThemeById(id: string): Theme | undefined {
  const normalized = normalizeThemeId(id);
  return themes.find((t) => t.id === normalized || normalizeThemeId(t.name) === normalized);
}

export function applyTheme(theme: ThemeColors): void {
  const root = document.documentElement;
  root.style.setProperty("--bg-primary", theme.bgPrimary);
  root.style.setProperty("--bg-secondary", theme.bgSecondary);
  root.style.setProperty("--bg-sidebar", theme.bgSidebar);
  root.style.setProperty("--bg-hover", theme.bgHover);
  root.style.setProperty("--bg-active", theme.bgActive);
  root.style.setProperty("--bg-input", theme.bgInput);
  root.style.setProperty("--bg-code", theme.bgCode);
  root.style.setProperty("--text-primary", theme.textPrimary);
  root.style.setProperty("--text-secondary", theme.textSecondary);
  root.style.setProperty("--text-muted", theme.textMuted);
  root.style.setProperty("--border", theme.border);
  root.style.setProperty("--accent", theme.accent);
  root.style.setProperty("--text-link", theme.textLink);
  root.style.setProperty("--text-link-hover", theme.textLinkHover);
  root.style.setProperty("--text-link-visited", theme.textLinkVisited);
  root.style.setProperty("--accent-secondary", theme.accentSecondary);
  root.style.setProperty("--danger", theme.danger);
  root.style.setProperty("--danger-bg", theme.danger + "22");
  root.style.setProperty("--btn-bg", theme.btnBg);
  root.style.setProperty("--btn-bg-hover", theme.btnBgHover);
  root.style.setProperty("--btn-primary-bg", theme.btnPrimaryBg);
  root.style.setProperty("--btn-primary-fg", theme.btnPrimaryFg);
  root.style.setProperty("--btn-primary-hover", theme.btnPrimaryHover);
  root.style.setProperty("--surface-raised", theme.surfaceRaised);
  root.style.setProperty("--surface-overlay", theme.surfaceOverlay);
  root.style.setProperty("--task-todo-bg", theme.taskTodoBg);
  root.style.setProperty("--task-todo-fg", theme.taskTodoFg);
  root.style.setProperty("--task-doing-bg", theme.taskDoingBg);
  root.style.setProperty("--task-doing-fg", theme.taskDoingFg);
  root.style.setProperty("--task-done-bg", theme.taskDoneBg);
  root.style.setProperty("--task-done-fg", theme.taskDoneFg);
  root.style.setProperty("--task-later-bg", theme.taskLaterBg);
  root.style.setProperty("--task-later-fg", theme.taskLaterFg);

  // Update meta color-scheme for scrollbar etc.
  root.style.colorScheme = theme.isLight ? "light" : "dark";

  // Toggle optional visual-effects presets. Remove any previous fx-* class,
  // then add the current one (if any) so extra CSS layers can apply.
  root.classList.forEach((cls) => {
    if (cls.startsWith("theme-fx-")) root.classList.remove(cls);
  });
  if (theme.fx) root.classList.add(`theme-fx-${theme.fx}`);
}
