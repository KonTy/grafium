<script lang="ts">
  import {
    getCompletionCounts,
    getCompletedTasks,
    listOpenTaskRows,
    taskFlowStats,
    cycleTaskState,
  } from "../lib/api";
  import {
    groupTasks,
    humanDuration,
    paceTrend,
    type OpenTaskRow,
    type TaskFlowStats,
  } from "../lib/taskBoard";
  import { renderBlock } from "../lib/markdown";
  import { hydrateRenderedMedia } from "../lib/renderedMedia";
  import type { CompletedTask } from "../lib/api";

  interface Props {
    onNavigate?: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  let completionMap = $state<Map<string, number>>(new Map());
  let completedTasks = $state<CompletedTask[]>([]);
  let openTasks = $state<OpenTaskRow[]>([]);
  let flow = $state<TaskFlowStats | null>(null);
  // Recomputed each load rather than continuously: the buckets only move when
  // the day does, and a task drifting between groups mid-read is disorienting.
  let today = $state(new Date());
  let groups = $derived(groupTasks(openTasks, today));
  let loading = $state(true);
  let totalCompleted = $state(0);
  let hoveredDay: { date: string; count: number; x: number; y: number } | null = $state(null);

  const WEEKS = 26;
  const DAYS = WEEKS * 7;

  $effect(() => {
    loadStats();
  });

  async function loadStats() {
    loading = true;
    try {
      const [counts, tasks, open, stats] = await Promise.all([
        getCompletionCounts(DAYS),
        getCompletedTasks(DAYS),
        listOpenTaskRows(),
        taskFlowStats(12),
      ]);
      today = new Date();
      flow = stats;
      const map = new Map<string, number>();
      let total = 0;
      for (const [date, count] of counts) {
        map.set(date, count);
        total += count;
      }
      completionMap = map;
      completedTasks = tasks;
      openTasks = open;
      totalCompleted = total;
    } catch (e) {
      console.error("Failed to load stats:", e);
    } finally {
      loading = false;
    }
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

  async function completeTask(blockId: string) {
    // Cycle forward until we reach DONE. TODO → DOING → DONE requires two clicks,
    // so loop up to 3 times defensively.
    try {
      for (let i = 0; i < 3; i++) {
        const next = await cycleTaskState(blockId);
        if (next === "DONE") break;
      }
      await loadStats();
    } catch (e) {
      console.error("Failed to complete task:", e);
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
    <h1>Tasks</h1>
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
            <div class="heatmap-grid">
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

        <!-- Flow metrics.
             Streaks used to live here and were deliberately removed. GitHub
             dropped its streak counter in 2016 after burnout reports: a streak
             motivates for a few weeks, then flips to fear of losing it, and
             breaking a long one leaves people worse off than before they
             started. The heatmap stays because it describes without setting a
             target you can fail. What replaced the counters is elapsed time —
             how long work waits and how long it takes — which is actionable
             without keeping score against the reader. -->
        <div class="summary-cards">
          <div class="stat-card">
            <span class="stat-value">
              {flow ? flow.throughput_7d.toFixed(1) : "—"}
              {#if flow && paceTrend(flow) !== "steady"}
                <span
                  class="trend {paceTrend(flow)}"
                  title={paceTrend(flow) === "up" ? "Ahead of last week" : "Quieter than last week"}
                >{paceTrend(flow) === "up" ? "▲" : "▼"}</span>
              {/if}
            </span>
            <span class="stat-label">Done / day</span>
          </div>
          <div class="stat-card" title="Median time from starting a task to finishing it">
            <span class="stat-value">{(flow && humanDuration(flow.median_cycle_ms)) ?? "—"}</span>
            <span class="stat-label">Time to finish</span>
          </div>
          <div class="stat-card" title="Median time a task waits before you start it">
            <span class="stat-value">{(flow && humanDuration(flow.median_wait_ms)) ?? "—"}</span>
            <span class="stat-label">Time to start</span>
          </div>
          <div class="stat-card" title="Of tasks that had a deadline, the share finished by it">
            <span class="stat-value">
              {flow?.on_time_rate != null ? `${Math.round(flow.on_time_rate * 100)}%` : "—"}
            </span>
            <span class="stat-label">On time</span>
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

    <!-- Open tasks, grouped by when they need a decision.
         "From earlier" rather than "Overdue", and no count badge on it: a
         growing red number reads as an accusation and drives avoidance rather
         than action. The oldest-task line below says the same thing in terms
         of time, which is something you can act on. -->
    <div class="open-tasks">
      <h2 class="section-heading">
        Open
        <span class="section-count">{openTasks.length}</span>
      </h2>

      {#if flow?.oldest_open_days != null && flow.oldest_open_days > 14}
        <p class="oldest-note">
          Your longest-waiting task has been open {flow.oldest_open_days} days.
        </p>
      {/if}

      {#if openTasks.length === 0}
        <div class="empty-state">
          <p>Nothing open. Add a task with a <code>- TODO ...</code> block.</p>
        </div>
      {:else}
        {#each groups as group (group.bucket)}
          <div class="task-group">
            <div class="date-header">
              <span class="date-text">{group.label}</span>
              <span class="date-count">{group.tasks.length}</span>
            </div>
            <div class="task-list">
              {#each group.tasks as task (task.block_id)}
                <div class="task-item open">
                  <!-- svelte-ignore a11y_click_events_have_key_events -->
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div
                    class="task-check open"
                    title="Mark done"
                    onclick={() => completeTask(task.block_id)}
                  >&#9633;</div>
                  <div class="task-body">
                    <button
                      class="task-content task-open"
                      onclick={() => onNavigate?.(task.page_title)}
                      title={`Open ${task.page_title}`}
                    >
                      {#if task.priority}
                        <span class="task-priority priority-{task.priority}">[#{task.priority}]</span>
                      {/if}
                      <span class="task-state task-state-{task.state.toLowerCase()}">{task.state}</span>
                      <span use:hydrateRenderedMedia={task.content}>{@html renderBlock(stripTaskMarker(task.content))}</span>
                    </button>
                    <span class="task-meta">
                      <!-- svelte-ignore a11y_click_events_have_key_events -->
                      <!-- svelte-ignore a11y_no_static_element_interactions -->
                      <span
                        class="task-page"
                        onclick={() => onNavigate?.(task.page_title)}
                      >{task.page_title}</span>
                      {#if task.deadline_date}
                        <span class="task-time">due {task.deadline_date}</span>
                      {:else if task.scheduled_date}
                        <span class="task-time">{task.scheduled_date}{task.scheduled_time ? ` ${task.scheduled_time}` : ""}</span>
                      {/if}
                    </span>
                  </div>
                </div>
              {/each}
            </div>
          </div>
        {/each}
      {/if}
    </div>

    {#if flow && flow.by_page.length > 1}
      <div class="by-page">
        <h2 class="section-heading">Where the work went</h2>
        <div class="page-bars">
          {#each flow.by_page as [title, count] (title)}
            <button class="page-bar" onclick={() => onNavigate?.(title)} title={`Open ${title}`}>
              <span class="page-bar-name">{title}</span>
              <span
                class="page-bar-fill"
                style="width: {Math.round((count / flow.by_page[0][1]) * 100)}%"
              ></span>
              <span class="page-bar-count">{count}</span>
            </button>
          {/each}
        </div>
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
                    <button
                      class="task-content task-open"
                      onclick={() => onNavigate?.(task.page_title)}
                      title={`Open ${task.page_title}`}
                    >
                      <span use:hydrateRenderedMedia={task.content}>{@html renderBlock(stripTaskMarker(task.content))}</span>
                    </button>
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

  .trend {
    font-size: 0.7em;
    vertical-align: middle;
  }

  /* Direction only, never a percentage: a hard number invites reading an
     ordinary quiet week as a failure. */
  .trend.up {
    color: var(--success, #3fb950);
  }

  .trend.down {
    color: var(--text-muted);
  }

  .oldest-note {
    margin: 0 0 12px;
    color: var(--text-secondary);
    font-size: 13px;
  }

  .task-group {
    margin-bottom: 18px;
  }

  .task-priority {
    font-weight: 700;
    margin-right: 6px;
  }

  .task-priority.priority-A {
    color: var(--danger, #f85149);
  }

  .task-priority.priority-B {
    color: var(--warning, #d29922);
  }

  .task-priority.priority-C {
    color: var(--text-muted);
  }

  .by-page {
    margin-top: 28px;
  }

  .page-bars {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .page-bar {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    min-height: 32px;
    border: none;
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    overflow: hidden;
  }

  /* Sits behind the label rather than beside it, so a long page title still
     gets the full width to be readable in. */
  .page-bar-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--accent);
    opacity: 0.16;
    pointer-events: none;
  }

  .page-bar-name {
    position: relative;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .page-bar-count {
    position: relative;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .page-bar:hover {
    background: var(--bg-hover, var(--bg-secondary));
  }

  .page-bar:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
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
  /* Keeps the calendar to its natural size instead of filling the row, so the
     summary cards sit beside it rather than being pushed to the far edge. */
  .heatmap-container {
    flex: 0 0 auto;
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
    grid-template-rows: repeat(7, var(--heatmap-cell, 11px));
    row-gap: 2px;
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

  /* Fixed cells rather than stretch-to-fill.
     `flex: 1` plus `aspect-ratio: 26/7` meant the grid took whatever width was
     going and then demanded a proportional height — on a wide window that is a
     ~430px wall of squares, dwarfing the tasks underneath it. Cells are sized
     like GitHub's contribution graph instead, so the calendar stays a glance
     rather than a centrepiece. */
  .heatmap-grid {
    display: grid;
    grid-template-rows: repeat(7, var(--heatmap-cell, 11px));
    grid-auto-columns: var(--heatmap-cell, 11px);
    grid-auto-flow: column;
    gap: 2px;
    min-width: 0;
  }

  .heatmap-cell {
    border-radius: 2px;
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

  /* Open tasks section */
  .open-tasks {
    padding: 24px 24px 0;
  }

  .section-count {
    font-size: 0.75rem;
    color: var(--text-muted);
    background: var(--bg-secondary);
    padding: 2px 8px;
    border-radius: 10px;
    margin-left: 8px;
    font-weight: 500;
  }

  .task-item.open .task-check.open {
    color: var(--text-muted);
    font-size: 1.05rem;
    cursor: pointer;
    transition: color 0.15s;
  }

  .task-item.open .task-check.open:hover {
    color: var(--accent-secondary);
  }

  .task-state {
    display: inline-block;
    font-size: 0.65rem;
    font-weight: 700;
    letter-spacing: 0.5px;
    padding: 1px 6px;
    border-radius: 3px;
    margin-right: 6px;
    background: var(--bg-secondary);
    color: var(--text-muted);
    vertical-align: 1px;
  }

  .task-state-todo { color: var(--accent); background: color-mix(in srgb, var(--accent) 15%, transparent); }
  .task-state-doing { color: var(--accent-secondary); background: color-mix(in srgb, var(--accent-secondary) 15%, transparent); }
  .task-state-now { color: var(--text-link); background: color-mix(in srgb, var(--text-link) 15%, transparent); }
  .task-state-later { color: var(--text-muted); }

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
    background: var(--bg-secondary);
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

  /* A real <button> so the row is reachable and activatable by keyboard;
     styled flat so it still reads as part of the list rather than a control. */
  .task-open {
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .task-open:hover {
    color: var(--text-link);
  }

  .task-open:focus-visible {
    outline: 2px solid var(--text-link);
    outline-offset: 2px;
    border-radius: 3px;
  }

  .task-content :global(.page-link) {
    color: var(--text-link);
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
    color: var(--text-link);
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
