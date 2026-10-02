export const CHAT_HISTORY_DEFAULT = 240;
export const CHAT_HISTORY_MIN = 180;
export const CHAT_DRAWER_BREAKPOINT = 640;
// 400px for the composer, 24px pane padding, and the 8px divider.
const CONVERSATION_RESERVE = 432;

export function chatLayout(width: number, offset: number, narrowPaddingPct: number) {
  const drawer = width <= CHAT_DRAWER_BREAKPOINT;
  const inset = drawer ? 0 : Math.min(width * Math.max(0, Math.min(40, narrowPaddingPct)) / 100,
    Math.max(0, width - CHAT_HISTORY_MIN - CONVERSATION_RESERVE));
  const maximum = Math.max(CHAT_HISTORY_MIN, width - inset - CONVERSATION_RESERVE);
  const automatic = CHAT_HISTORY_DEFAULT + inset;
  const history = Math.max(CHAT_HISTORY_MIN, Math.min(maximum, automatic + offset));
  return { drawer, inset, maximum, automatic, history };
}
