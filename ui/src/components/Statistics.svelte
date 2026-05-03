<script lang="ts">
  import { getCompletionCounts, getCompletedTasks, cycleTaskState } from "../lib/api";
  import { renderBlock } from "../lib/markdown";
  import type { CompletedTask } from "../lib/api";

  interface Props {
    onNavigate?: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  let completionMap = $state<Map<string, number>>(new Map());
  let completedTasks = $state<CompletedTask[]>([]);
  let loading = $state(true);
  let totalCompleted = $state(0);
  let currentStreak = $state(0);
  let longestStreak = $state(0);
  let hoveredDay: { date: string; count: number; x: number; y: number } | null = $state(null);

  const WEEKS = 26;
  const DAYS = WEEKS * 7;

  $effect(() => {
    loadStats();
  });

  async function loadStats() {
    loading = true;
    try {
      const [counts, tasks] = await Promise.all([
        getCompletionCounts(DAYS),
        getCompletedTasks(DAYS),
      ]);
      const map = new Map<string, number>();
      let total = 0;
      for (const [date, count] of counts) {
        map.set(date, count);
        total += count;
      }
      completionMap = map;
      completedTasks = tasks;
      totalCompleted = total;
      computeStreaks(map);
    } catch (e) {
      console.error("Failed to load stats:", e);
    } finally {
      loading = false;
    }
  }

  function computeStreaks(map: Map<string, number>) {
    const today = new Date();
    let streak = 0;
    let longest = 0;
    let running = 0;
    // Walk backwards from today
    for (let i = 0; i < DAYS; i++) {
      const d = new Date(today);
      d.setDate(d.getDate() - i);
      const key = d.toISOString().split("T")[0];
      if (map.has(key) && (map.get(key) ?? 0) > 0) {
        running++;
        if (i === 0 || streak > 0) streak = running;
      } else {
        if (i === 0) streak = 0; // no completion today, check yesterday
        else if (i === 1 && streak === 0) streak = 0; // nothing yesterday either
        if (running > longest) longest = running;
        running = 0;
      }
    }
    // If first day missed but second day had, check
    if (streak === 0) {
      // Check if yesterday started a streak
      let yStreak = 0;
      for (let i = 1; i < DAYS; i++) {
        const d = new Date(today);
        d.setDate(d.getDate() - i);
        const key = d.toISOString().split("T")[0];
        if (map.has(key) && (map.get(key) ?? 0) > 0) {
          yStreak++;
        } else break;
      }
      streak = yStreak;
    }
    if (running > longest) longest = running;
    currentStreak = streak;
    longestStreak = longest;
  }

  function getColor(count: number): string {
    if (count === 0) return "var(--heatmap-empty)";
    if (count === 1) return "var(--heatmap-l1)";
    if (count <= 3) return "var(--heatmap-l2)";
    if (count <= 5) return "var(--heatmap-l3)";
    return "var(--heatmap-l4)";
  }

  function generateGrid(): { date: string; count: number; col: number; row: number }[] {
    const today = new Date();
    const todayDow = today.getDay(); // 0=Sun
    const cells: { date: string; count: number; col: number; row: number }[] = [];

    // We want WEEKS columns. Last column ends on today's day-of-week row.
    // Total cells = WEEKS * 7, but we only fill up to today.
    const totalCells = WEEKS * 7;
    const startDate = new Date(today);
    startDate.setDate(startDate.getDate() - totalCells + 1 + (6 - todayDow));
    // Adjust: the grid starts on a Sunday
    startDate.setDate(startDate.getDate() - startDate.getDay());

    for (let col = 0; col < WEEKS; col++) {
      for (let row = 0; row < 7; row++) {
        const d = new Date(startDate);
        d.setDate(d.getDate() + col * 7 + row);
        if (d > today) continue;
        const key = d.toISOString().split("T")[0];
        cells.push({
          date: key,
          count: completionMap.get(key) ?? 0,
          col,
          row,
        });
      }
    }
    return cells;
  }

  function getMonthLabels(): { label: string; col: number }[] {
    const today = new Date();
    const totalCells = WEEKS * 7;
    const startDate = new Date(today);
    startDate.setDate(startDate.getDate() - totalCells + 1 + (6 - today.getDay()));
    startDate.setDate(startDate.getDate() - startDate.getDay());

    const labels: { label: string; col: number }[] = [];
    const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let lastMonth = -1;

    for (let col = 0; col < WEEKS; col++) {
      const d = new Date(startDate);
      d.setDate(d.getDate() + col * 7);
      const m = d.getMonth();
      if (m !== lastMonth) {
        labels.push({ label: months[m], col });
        lastMonth = m;
      }
    }
    return labels;
  }

  function groupByDate(tasks: CompletedTask[]): Map<string, CompletedTask[]> {
    const map = new Map<string, CompletedTask[]>();
    for (const t of tasks) {
      const d = new Date(t.timestamp);
      const key = d.toISOString().split("T")[0];
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(t);
    }
    return map;
  }

  function formatDate(dateStr: string): string {
    const d = new Date(dateStr + "T00:00:00");
    return d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric" });
  }

  function formatTime(ts: number): string {
    return new Date(ts).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  }

  function stripTaskMarker(content: string): string {
    return content.replace(/^(TODO|DOING|DONE|NOW|LATER|CANCELED)\s+/, "");
  }

  function handleCellHover(e: MouseEvent, date: string, count: number) {
    const rect = (e.target as HTMLElement).getBoundingClientRect();
    hoveredDay = { date, count, x: rect.left + rect.width / 2, y: rect.top };
  }

  function handleCellLeave() {
    hoveredDay = null;
  }

  async function uncompleteTask(blockId: string) {
    try {
      await cycleTaskState(blockId); // DONE → TODO
      await loadStats();
    } catch (e) {
      console.error("Failed to uncomplete task:", e);
    }
  }

  $effect(() => {
    // dummy read to trigger reactivity
    completionMap;
  });

  let grid = $derived(generateGrid());
  let monthLabels = $derived(getMonthLabels());
  let tasksByDate = $derived(groupByDate(completedTasks));
  let sortedDates = $derived([...tasksByDate.keys()].sort((a, b) => b.localeCompare(a)));
  const dayLabels = ["", "Mon", "", "Wed", "", "Fri", ""];
</script>

<div class="statistics-view">
  <div class="stats-header">
    <h1>Statistics</h1>
  </div>

  {#if loading}
    <div class="loading">Loading statistics...</div>
  {:else}
    <!-- Sticky heatmap section -->
    <div class="heatmap-sticky">
      <div class="heatmap-row">
        <!-- Heatmap -->
        <div class="heatmap-container">
          <!-- Month labels -->
          <div class="month-labels">
            <span class="day-spacer"></span>
            {#each monthLabels as ml}
              <span class="month-label" style="grid-column: {ml.col + 2};">{ml.label}</span>
            {/each}
          </div>
          <div class="heatmap-grid-area">
            <!-- Day labels -->
            <div class="day-labels">
              {#each dayLabels as label}
                <span class="day-label">{label}</span>
              {/each}
            </div>
            <!-- Grid -->
            <div class="heatmap-grid" style="grid-template-columns: repeat({WEEKS}, 1fr); aspect-ratio: {WEEKS} / 7;">
              {#each grid as cell}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                  class="heatmap-cell"
                  style="grid-column: {cell.col + 1}; grid-row: {cell.row + 1}; background: {getColor(cell.count)};"
                  onmouseenter={(e) => handleCellHover(e, cell.date, cell.count)}
                  onmouseleave={handleCellLeave}
                ></div>
              {/each}
            </div>
          </div>

          <!-- Legend -->
          <div class="heatmap-legend">
            <span class="legend-label">Less</span>
            <div class="legend-cell" style="background: var(--heatmap-empty);"></div>
            <div class="legend-cell" style="background: var(--heatmap-l1);"></div>
            <div class="legend-cell" style="background: var(--heatmap-l2);"></div>
            <div class="legend-cell" style="background: var(--heatmap-l3);"></div>
            <div class="legend-cell" style="background: var(--heatmap-l4);"></div>
            <span class="legend-label">More</span>
          </div>
        </div>

        <!-- Summary cards -->
        <div class="summary-cards">
          <div class="stat-card">
            <span class="stat-value">{totalCompleted}</span>
            <span class="stat-label">Completed</span>
          </div>
          <div class="stat-card">
            <span class="stat-value">{currentStreak}</span>
            <span class="stat-label">Current Streak</span>
          </div>
          <div class="stat-card">
            <span class="stat-value">{longestStreak}</span>
            <span class="stat-label">Longest Streak</span>
          </div>
          <div class="stat-card">
            <span class="stat-value">{completedTasks.length > 0 ? (totalCompleted / Math.max(1, completionMap.size)).toFixed(1) : "0"}</span>
            <span class="stat-label">Avg/Day</span>
          </div>
        </div>
      </div>
    </div>

    <!-- Tooltip -->
    {#if hoveredDay}
      <div class="heatmap-tooltip" style="left: {hoveredDay.x}px; top: {hoveredDay.y - 8}px;">
        <strong>{hoveredDay.count}</strong> task{hoveredDay.count !== 1 ? "s" : ""} completed
        <br><span class="tooltip-date">{formatDate(hoveredDay.date)}</span>
      </div>
    {/if}

    <!-- Completed tasks grouped by date -->
    <div class="completed-tasks">
      <h2 class="section-heading">Completed Tasks</h2>
      {#if sortedDates.length === 0}
        <div class="empty-state">
          <p>No completed tasks yet. Click a TODO marker to cycle it to DONE.</p>
        </div>
      {:else}
        {#each sortedDates as date}
          <div class="date-group">
            <div class="date-header">
              <span class="date-text">{formatDate(date)}</span>
              <span class="date-count">{tasksByDate.get(date)?.length ?? 0} task{(tasksByDate.get(date)?.length ?? 0) !== 1 ? "s" : ""}</span>
            </div>
            <div class="task-list">
              {#each tasksByDate.get(date) ?? [] as task}
                <div class="task-item">
                  <div class="task-check">&#10003;</div>
                  <div class="task-body">
                    <span class="task-content">{@html renderBlock(stripTaskMarker(task.content))}</span>
                    <span class="task-meta">
                      <!-- svelte-ignore a11y_click_events_have_key_events -->
                      <!-- svelte-ignore a11y_no_static_element_interactions -->
                      <span
                        class="task-page"
                        onclick={() => onNavigate?.(task.page_title)}
                      >{task.page_title}</span>
                      <span class="task-time">{formatTime(task.timestamp)}</span>
                    </span>
                  </div>
                </div>
              {/each}
            </div>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .statistics-view {
    --heatmap-empty: #2a2a3c;
    --heatmap-l1: #0e4429;
    --heatmap-l2: #006d32;
    --heatmap-l3: #26a641;
    --heatmap-l4: #39d353;
    position: relative;
    height: 100%;
    overflow-y: auto;
    padding: 0;
  }

  .stats-header {
    padding: 24px 24px 0;
  }

  .stats-header h1 {
    font-size: 1.5rem;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0;
  }

  .loading {
    padding: 48px;
    text-align: center;
    color: var(--text-muted);
  }

  /* Sticky heatmap */
  .heatmap-sticky {
    position: sticky;
    top: 0;
    z-index: 10;
    background: var(--bg-primary);
    padding: 16px 24px;
    border-bottom: 1px solid var(--border);
  }

  /* Wide: heatmap left, cards right. Narrow: stacked */
  .heatmap-row {
    display: flex;
    gap: 20px;
    align-items: flex-start;
  }

  /* Summary cards */
  .summary-cards {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
    flex-shrink: 0;
    width: 180px;
  }

  .stat-card {
    background: var(--bg-secondary);
    border-radius: 8px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .stat-value {
    font-size: 1.5rem;
    font-weight: 700;
    color: var(--accent);
  }

  .stat-label {
    font-size: 0.75rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  /* Heatmap */
  .heatmap-container {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }

  .month-labels {
    display: grid;
    grid-template-columns: 24px repeat(26, 1fr);
    font-size: 0.65rem;
    color: var(--text-muted);
    margin-bottom: 2px;
    height: 14px;
  }

  .day-spacer {
    /* empty cell to align with day-labels column */
  }

  .month-label {
    white-space: nowrap;
    overflow: hidden;
  }

  .heatmap-grid-area {
    display: flex;
    gap: 0;
    overflow: hidden;
  }

  .day-labels {
    display: grid;
    grid-template-rows: repeat(7, 1fr);
    gap: 1px;
    width: 24px;
    flex-shrink: 0;
  }

  .day-label {
    font-size: 0.6rem;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    padding-right: 4px;
    justify-content: flex-end;
  }

  .heatmap-grid {
    display: grid;
    grid-template-rows: repeat(7, 1fr);
    grid-auto-flow: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }

  .heatmap-cell {
    border-radius: 1px;
    width: 100%;
    height: 100%;
    cursor: pointer;
    transition: outline 0.1s;
  }

  .heatmap-cell:hover {
    outline: 2px solid var(--text-secondary);
    outline-offset: -1px;
  }

  /* Legend */
  .heatmap-legend {
    display: flex;
    align-items: center;
    gap: 4px;
    justify-content: flex-end;
  }

  .legend-label {
    font-size: 0.7rem;
    color: var(--text-muted);
  }

  .legend-cell {
    width: 12px;
    height: 12px;
    border-radius: 2px;
  }

  /* Tooltip */
  .heatmap-tooltip {
    position: fixed;
    transform: translateX(-50%) translateY(-100%);
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 0.8rem;
    color: var(--text-primary);
    pointer-events: none;
    z-index: 100;
    white-space: nowrap;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  }

  .tooltip-date {
    color: var(--text-muted);
    font-size: 0.7rem;
  }

  /* Completed tasks */
  .completed-tasks {
    padding: 24px;
  }

  .section-heading {
    font-size: 1.1rem;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 16px;
  }

  .empty-state {
    padding: 32px;
    text-align: center;
    color: var(--text-muted);
    font-size: 0.9rem;
  }

  .date-group {
    margin-bottom: 20px;
  }

  .date-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--border);
  }

  .date-text {
    font-weight: 600;
    color: var(--text-primary);
    font-size: 0.9rem;
  }

  .date-count {
    font-size: 0.75rem;
    color: var(--text-muted);
    background: var(--bg-secondary);
    padding: 2px 8px;
    border-radius: 10px;
  }

  .task-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .task-item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 12px;
    border-radius: 6px;
    transition: background 0.15s;
  }

  .task-item:hover {
    background: var(--bg-hover);
  }

  .task-check {
    color: var(--accent-secondary);
    font-size: 0.85rem;
    font-weight: 700;
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    display: flex;
    align-items: center;
    justify-content: center;
    margin-top: 1px;
  }

  .task-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .task-content {
    color: var(--text-primary);
    font-size: 0.9rem;
    line-height: 1.4;
    word-break: break-word;
  }

  .task-content :global(.page-link) {
    color: var(--accent);
    cursor: pointer;
  }

  .task-content :global(.tag) {
    color: var(--accent-secondary);
  }

  .task-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.75rem;
  }

  .task-page {
    color: var(--accent);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .task-page:hover {
    text-decoration: underline;
  }

  .task-time {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  /* ─── Mobile / narrow screen ─── */
  @media (max-width: 900px) {
    .heatmap-row {
      flex-direction: column;
    }

    .summary-cards {
      width: 100%;
      grid-template-columns: repeat(4, 1fr);
    }
  }

  @media (max-width: 640px) {
    .stats-header {
      padding: 16px 16px 0;
    }

    .stats-header h1 {
      font-size: 1.25rem;
    }

    .heatmap-sticky {
      padding: 12px 12px;
    }

    .stat-card {
      padding: 10px 8px;
    }

    .stat-value {
      font-size: 1.25rem;
    }

    .stat-label {
      font-size: 0.65rem;
    }

    .completed-tasks {
      padding: 16px 12px;
    }

    .task-item {
      padding: 8px 8px;
    }

    .task-meta {
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
    }
  }

  @media (max-width: 420px) {
    .summary-cards {
      grid-template-columns: repeat(2, 1fr);
      gap: 6px;
    }

    .stat-card {
      padding: 8px 6px;
    }

    .stat-value {
      font-size: 1.1rem;
    }

    .day-labels {
      width: 18px;
    }

    .month-labels {
      grid-template-columns: 18px repeat(26, 1fr);
    }

    .date-header {
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
    }
  }
</style>
