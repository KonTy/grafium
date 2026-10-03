import type { StreamPhase } from "./chatStatus";

/** Stroke paths on a 24px grid, drawn with `currentColor`. */
export const STEP_ICONS: Record<StreamPhase | "elapsed", readonly string[]> = {
  retrieving: ["M17 11a6 6 0 1 1-12 0 6 6 0 0 1 12 0Z", "m15.5 15.5 4.5 4.5"],
  loading_model: ["M12 4v11", "m7 10 5 5 5-5", "M5 20h14"],
  processing_prompt: ["M6 3h8l4 4v14H6Z", "M14 3v4h4", "M9 12h6", "M9 16h6"],
  thinking: ["M9 18h6", "M10 21h4", "M12 3a6 6 0 0 0-3.5 10.9V16h7v-2.1A6 6 0 0 0 12 3Z"],
  generating: ["M4 20h4L19 9l-4-4L4 16Z", "m13 7 4 4"],
  searching_web: [
    "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z", "M3 12h18",
    "M12 3a14 14 0 0 1 0 18", "M12 3a14 14 0 0 0 0 18",
  ],
  reading_sources: ["M3 5h6a3 3 0 0 1 3 3v12a2 2 0 0 0-2-2H3Z", "M21 5h-6a3 3 0 0 0-3 3v12a2 2 0 0 1 2-2h7Z"],
  planning: ["M9 6h11", "M9 12h11", "M9 18h11", "M4.5 6h.01", "M4.5 12h.01", "M4.5 18h.01"],
  assessing: ["M9 3h6v3H9Z", "M7 4.5H5V21h14V4.5h-2", "m9 14 2 2 4-4"],
  refining: ["M4 5h16l-6 7v6l-4 2v-8Z"],
  synthesizing: ["M4 4h16v16H4Z", "M8 9h8", "M8 13h8", "M8 17h5"],
  elapsed: ["M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z", "M12 7v5l3 2"],
};
