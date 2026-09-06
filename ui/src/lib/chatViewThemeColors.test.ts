import { describe, it, expect } from "vitest";
// Raw import (typed by vite/client's `*?raw`) so we can assert on the component
// source without compiling it.
import chatViewSource from "../components/ChatView.svelte?raw";

// WHY: status/error/warning/thinking colours must come from the theme palette
// (var(--accent-*)), which themeContrast.test.ts guarantees is WCAG-AA on every
// theme surface. Raw hex bypasses that guard and fails on light themes —
// measured against Catppuccin Latte, #f87171 = 2.45:1, #fbbf24 = 1.48:1,
// #a78bfa = 2.41:1, all far below the 4.5:1 needed for 12px text. Ban these
// specific literals from ChatView so a regression can't silently reintroduce
// them.
const BANNED = ["#f87171", "#fbbf24", "#a78bfa", "#d9a441"];

describe("ChatView status colours stay themed", () => {
  for (const hex of BANNED) {
    it(`does not hardcode ${hex}`, () => {
      expect(chatViewSource.toLowerCase().includes(hex)).toBe(false);
    });
  }
});
