import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    watch: {
      usePolling: true,
      interval: 1000,
      ignored: ["**/target/**", "**/node_modules/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: true,
  },
  test: {
    // The assistant-markdown sanitizer parses HTML with a real DOM, so the
    // tests that prove it blocks injection need one too — running them under
    // a bare Node environment would exercise the escape-everything fallback
    // instead of the sanitizer, and prove nothing about the shipped path.
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
