import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/**
 * `style: false` under Vitest only.
 *
 * Every `<style>` block in this app is plain CSS -- nothing uses `lang="scss"`
 * or similar -- so the style pass is a no-op for correctness. It is not a no-op
 * for tests: it routes each block through Vite's `preprocessCSS`, and Vitest
 * 2.x hands Vite 6 a config shape its environment API does not accept, so
 * merely importing any component that has styles dies with "Cannot create proxy
 * with a non-object as target or handler". Skipping it lets component tests
 * mount the real components instead of style-free stand-ins. The production
 * build keeps the full preprocessor.
 */
export default {
  preprocess: vitePreprocess(process.env.VITEST ? { style: false } : undefined),
};
