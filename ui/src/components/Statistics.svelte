<script lang="ts">
  import {
    getCompletionCounts,
    getCompletedTasks,
    listOpenTaskRows,
    taskFlowStats,
    cycleTaskState,
    getBlock,
    updateTaskState,
    getNoteEditCounts,
    getNoteEditsForDay,
  } from "../lib/api";
  import {
    groupOpenTasks,
    humanDuration,
    paceTrend,
    type OpenTaskSort,
    type OpenTaskRow,
    type OpenTaskView,
    type TaskFlowStats,
  } from "../lib/taskBoard";
  import { renderBlock } from "../lib/markdown";
  import { hydrateRenderedMedia } from "../lib/renderedMedia";
  import { pushUndo } from "../lib/undoStack";
  import type { CompletedTask, NoteEditDayEntry } from "../lib/api";

  interface Props {
    onNavigate?: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  let completionMap = $state<Map<string, number>>(new Map());
  let noteEditMap = $state<Map<string, number>>(new Map());
  let completedTasks = $state<CompletedTask[]>([]);
  let openTasks = $state<OpenTaskRow[]>([]);
  let flow = $state<TaskFlowStats | null>(null);
  // Recomputed each load rather than continuously: the buckets only move when
  // the day does, and a task drifting between groups mid-read is disorienting.
  let today = $state(new Date());
  let openTaskView = $state<OpenTaskView>("date");
  let openTaskSort = $state<OpenTaskSort>("smart");
  let groups = $derived.by(() => groupOpenTasks(openTasks, today, openTaskView, openTaskSort));
  let loading = $state(true);
  let totalCompleted = $state(0);
  let totalEditedNotes = $state(0);
  let hoveredDay: { date: string; count: number; x: number; y: number; label: string } | null = $state(null);
  type HeatmapKind = "task" | "note";
  let selectedDay: { kind: HeatmapKind; date: string; count: number } | null = $state(null);
  let noteEditsForSelectedDay = $state<NoteEditDayEntry[]>([]);
  let noteEditsLoading = $state(false);

  const MIN_WEEKS = 26;
  const HISTORY_DAYS = 365 * 10;
  const OPEN_TASK_VIEW_OPTIONS: Array<{ value: OpenTaskView; label: string }> = [
    { value: "date", label: "By date" },
    { value: "priority", label: "By priority" },
    { value: "status", label: "By status" },
    { value: "page", label: "By page" },
  ];
  const OPEN_TASK_SORT_OPTIONS: Array<{ value: OpenTaskSort; label: string }> = [
    { value: "smart", label: "Smart" },
    { value: "priority", label: "Priority" },
    { value: "date", label: "Date" },
    { value: "oldest", label: "Oldest" },
    { value: "newest", label: "Newest" },
    { value: "page", label: "Page" },
  ];
  const OPEN_TASK_VIEWS = new Set<OpenTaskView>(OPEN_TASK_VIEW_OPTIONS.map((option) => option.value));
  const OPEN_TASK_SORTS = new Set<OpenTaskSort>(OPEN_TASK_SORT_OPTIONS.map((option) => option.value));

  $effect(() => {
    loadStats();
  });

  async function loadStats() {
    loading = true;
    try {
      const [counts, tasks, open, stats, noteCounts] = await Promise.all([
        getCompletionCounts(HISTORY_DAYS),
        getCompletedTasks(HISTORY_DAYS),
        listOpenTaskRows(),
        taskFlowStats(12),
        getNoteEditCounts(HISTORY_DAYS),
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
      const edits = new Map<string, number>();
      let editedTotal = 0;
      for (const [date, count] of noteCounts) {
        edits.set(date, count);
        editedTotal += count;
      }
      noteEditMap = edits;
      completedTasks = tasks;
      openTasks = open;
      totalCompleted = total;
      totalEditedNotes = editedTotal;
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

  function getNoteEditColor(count: number): string {
    if (count === 0) return "var(--heatmap-empty)";
    if (count === 1) return "var(--note-heatmap-l1)";
    if (count <= 3) return "var(--note-heatmap-l2)";
    if (count <= 5) return "var(--note-heatmap-l3)";
    return "var(--note-heatmap-l4)";
  }

  interface GridRange {
    start: Date;
    today: Date;
    weeks: number;
  }

  function startOfDay(date: Date): Date {
    return new Date(date.getFullYear(), date.getMonth(), date.getDate());
  }

  function addDays(date: Date, days: number): Date {
    const next = new Date(date);
    next.setDate(next.getDate() + days);
    return next;
  }

  function daysBetween(start: Date, end: Date): number {
    return Math.round((startOfDay(end).getTime() - startOfDay(start).getTime()) / 86_400_000);
  }

  function dateFromKey(key: string): Date | null {
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(key);
    if (!match) return null;
    return new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  }

  function dateKey(date: Date): string {
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, "0");
    const day = String(date.getDate()).padStart(2, "0");
    return `${year}-${month}-${day}`;
  }

  function earliestActivityDate(...maps: Map<string, number>[]): Date | null {
    let earliest: Date | null = null;
    for (const map of maps) {
      for (const [key, count] of map) {
        if (count <= 0) continue;
        const date = dateFromKey(key);
        if (!date) continue;
        if (!earliest || date < earliest) earliest = date;
      }
    }
    return earliest;
  }

  function getGridRange(taskCounts: Map<string, number>, noteCounts: Map<string, number>): GridRange {
    const today = startOfDay(new Date());
    const currentWeekEnd = addDays(today, 6 - today.getDay());
    const defaultStart = addDays(currentWeekEnd, -(MIN_WEEKS * 7) + 1);
    const firstActivity = earliestActivityDate(taskCounts, noteCounts);
    const start = firstActivity && firstActivity < defaultStart
      ? addDays(firstActivity, -firstActivity.getDay())
      : defaultStart;
    const weeks = Math.max(MIN_WEEKS, Math.ceil((daysBetween(start, currentWeekEnd) + 1) / 7));
    return { start, today, weeks };
  }

  function generateGrid(counts: Map<string, number>, range: GridRange): { date: string; count: number; col: number; row: number }[] {
    const cells: { date: string; count: number; col: number; row: number }[] = [];

    for (let col = 0; col < range.weeks; col++) {
      for (let row = 0; row < 7; row++) {
        const d = addDays(range.start, col * 7 + row);
        if (d > range.today) continue;
        const key = dateKey(d);
        cells.push({
          date: key,
          count: counts.get(key) ?? 0,
          col,
          row,
        });
      }
    }
    return cells;
  }

  function getMonthLabels(range: GridRange): { label: string; col: number }[] {
    const labels: { label: string; col: number }[] = [];
    const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let lastMonth = -1;

    for (let col = 0; col < range.weeks; col++) {
      const d = addDays(range.start, col * 7);
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
      const key = dateKey(d);
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

  function sourceLabel(source: string): string {
    switch (source) {
      case "app":
        return "edited in Grafium";
      case "file":
        return "changed on disk";
      case "file-mtime":
        return "file modified";
      case "backfill":
        return "backfilled";
      default:
        return source;
    }
  }

  function noteEditMeta(edit: NoteEditDayEntry): string {
    const count = edit.edit_count === 1 ? "1 edit" : `${edit.edit_count} edits`;
    return `${count} · ${sourceLabel(edit.source)} · ${formatTime(edit.last_edited_at)}`;
  }

  function isSelectedCell(kind: HeatmapKind, date: string): boolean {
    return selectedDay?.kind === kind && selectedDay.date === date;
  }

  function heatmapCellLabel(kind: HeatmapKind, date: string, count: number): string {
    const noun = kind === "note" ? "note edit" : "task completion";
    return `${formatDate(date)}: ${count} ${noun}${count === 1 ? "" : "s"}`;
  }

  async function selectHeatmapDay(kind: HeatmapKind, date: string, count: number) {
    selectedDay = { kind, date, count };
    if (kind !== "note") {
      noteEditsForSelectedDay = [];
      noteEditsLoading = false;
      return;
    }

    noteEditsLoading = true;
    try {
      const edits = await getNoteEditsForDay(date);
      if (selectedDay?.kind === "note" && selectedDay.date === date) {
        noteEditsForSelectedDay = edits;
      }
    } catch (e) {
      console.error("Failed to load note edits:", e);
      if (selectedDay?.kind === "note" && selectedDay.date === date) {
        noteEditsForSelectedDay = [];
      }
    } finally {
      if (selectedDay?.kind === "note" && selectedDay.date === date) {
        noteEditsLoading = false;
      }
    }
  }

  function clearSelectedDay() {
    selectedDay = null;
    noteEditsForSelectedDay = [];
    noteEditsLoading = false;
  }

  function handleOpenTaskViewChange(event: Event) {
    const next = (event.currentTarget as HTMLSelectElement).value as OpenTaskView;
    if (OPEN_TASK_VIEWS.has(next)) openTaskView = next;
  }

  function handleOpenTaskSortChange(event: Event) {
    const next = (event.currentTarget as HTMLSelectElement).value as OpenTaskSort;
    if (OPEN_TASK_SORTS.has(next)) openTaskSort = next;
  }

  function pushTaskContentUndo(
    pageId: string,
    blockId: string,
    beforeContent: string,
    afterContent: string,
  ) {
    if (beforeContent === afterContent) return;
    pushUndo({
      type: "update_block",
      pageId,
      blockId,
      beforeContent,
      afterContent,
    });
  }

  function handleCellHover(e: MouseEvent, date: string, count: number, label: string) {
    const rect = (e.target as HTMLElement).getBoundingClientRect();
    hoveredDay = { date, count, x: rect.left + rect.width / 2, y: rect.top, label };
  }

  function handleCellLeave() {
    hoveredDay = null;
  }

  async function uncompleteTask(blockId: string) {
    try {
      const before = await getBlock(blockId);
      const afterContent = await cycleTaskState(blockId); // DONE → TODO
      pushTaskContentUndo(before.page_id, blockId, before.content, afterContent);
      await loadStats();
    } catch (e) {
      console.error("Failed to uncomplete task:", e);
    }
  }

  async function completeTask(blockId: string) {
    try {
      const before = await getBlock(blockId);
      const afterContent = await updateTaskState(blockId, "DONE");
      pushTaskContentUndo(before.page_id, blockId, before.content, afterContent);
      await loadStats();
    } catch (e) {
      console.error("Failed to complete task:", e);
    }
  }

  function openTaskSource(pageTitle: string, blockId: string) {
    window.dispatchEvent(
      new CustomEvent("navigate-page", {
        detail: { pageName: pageTitle, targetBlockId: blockId },
      })
    );
  }

  async function handleRenderedTaskClick(event: MouseEvent, blockId: string, pageTitle: string) {
    const target = event.target instanceof Element ? event.target : null;
    const checkbox = target?.closest(".task-checkbox");
    const marker = target?.closest(".task-marker");
    const pageLink = target?.closest(".page-link");
    const tag = target?.closest(".tag");
    if (checkbox instanceof HTMLElement && checkbox.dataset.taskAction === "done") {
      event.preventDefault();
      event.stopPropagation();
      await completeTask(blockId);
      return;
    }
    if (marker) {
      event.preventDefault();
      event.stopPropagation();
      try {
        const before = await getBlock(blockId);
        const afterContent = await cycleTaskState(blockId);
        pushTaskContentUndo(before.page_id, blockId, before.content, afterContent);
        await loadStats();
      } catch (e) {
        console.error("Failed to cycle task:", e);
      }
      return;
    }
    if (pageLink instanceof HTMLElement && pageLink.dataset.page) {
      event.preventDefault();
      event.stopPropagation();
      onNavigate?.(pageLink.dataset.page);
      return;
    }
    if (tag instanceof HTMLElement && tag.dataset.tag) {
      event.preventDefault();
      event.stopPropagation();
      onNavigate?.(tag.dataset.tag);
      return;
    }
    openTaskSource(pageTitle, blockId);
  }

  $effect(() => {
    // dummy read to trigger reactivity
    completionMap;
    noteEditMap;
  });

  let gridRange = $derived(getGridRange(completionMap, noteEditMap));
  let heatmapColumnsStyle = $derived(`grid-template-columns: repeat(${gridRange.weeks}, var(--heatmap-cell, 11px));`);
  let taskHeatmapEl: HTMLDivElement | null = $state(null);
  let noteHeatmapEl: HTMLDivElement | null = $state(null);
  let scrolledHeatmapToEnd = false;

  function scrollHeatmapToLatest(el: HTMLDivElement | null) {
    if (!el) return;
    el.scrollLeft = el.scrollWidth;
  }

  $effect(() => {
    if (loading) {
      scrolledHeatmapToEnd = false;
      return;
    }
    const task = taskHeatmapEl;
    const note = noteHeatmapEl;
    void gridRange.weeks;
    if (!task || !note || scrolledHeatmapToEnd) return;
    scrolledHeatmapToEnd = true;
    requestAnimationFrame(() => {
      scrollHeatmapToLatest(task);
      scrollHeatmapToLatest(note);
    });
  });
  let taskGrid = $derived(generateGrid(completionMap, gridRange));
  let noteEditGrid = $derived(generateGrid(noteEditMap, gridRange));
  let monthLabels = $derived(getMonthLabels(gridRange));
  let tasksByDate = $derived(groupByDate(completedTasks));
  let sortedDates = $derived([...tasksByDate.keys()].sort((a, b) => b.localeCompare(a)));
  let selectedCompletedTasks = $derived(selectedDay?.kind === "task" ? (tasksByDate.get(selectedDay.date) ?? []) : []);
  const dayLabels = ["", "Mon", "", "Wed", "", "Fri", ""];
</script>

<div class="statistics-view" data-main-scroll-pane>
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
          <div class="activity-heatmaps">
            <div class="activity-heatmap-panel" bind:this={taskHeatmapEl}>
              <div class="heatmap-title-row">
                <h2>Task completions</h2>
                <span>{totalCompleted} done</span>
              </div>
              <div class="month-labels" style={heatmapColumnsStyle}>
                {#each monthLabels as ml}
                  <span class="month-label" style="grid-column: {ml.col + 1};">{ml.label}</span>
                {/each}
              </div>
              <div class="heatmap-grid-area">
                <div class="day-labels">
                  {#each dayLabels as label}
                    <span class="day-label">{label}</span>
                  {/each}
                </div>
                <div class="heatmap-grid" style={heatmapColumnsStyle}>
                  {#each taskGrid as cell}
                    <button
                      type="button"
                      class="heatmap-cell"
                      class:selected={isSelectedCell("task", cell.date)}
                      style="grid-column: {cell.col + 1}; grid-row: {cell.row + 1}; background: {getColor(cell.count)};"
                      aria-label={heatmapCellLabel("task", cell.date, cell.count)}
                      title={heatmapCellLabel("task", cell.date, cell.count)}
                      onclick={() => void selectHeatmapDay("task", cell.date, cell.count)}
                      onmouseenter={(e) => handleCellHover(e, cell.date, cell.count, "task")}
                      onmouseleave={handleCellLeave}
                    ></button>
                  {/each}
                </div>
              </div>
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

            <div class="activity-heatmap-panel note-edit-heatmap" bind:this={noteHeatmapEl}>
              <div class="heatmap-title-row">
                <h2>Note edits</h2>
                <span>{totalEditedNotes} note-day{totalEditedNotes === 1 ? "" : "s"}</span>
              </div>
              <div class="month-labels" style={heatmapColumnsStyle}>
                {#each monthLabels as ml}
                  <span class="month-label" style="grid-column: {ml.col + 1};">{ml.label}</span>
                {/each}
              </div>
              <div class="heatmap-grid-area">
                <div class="day-labels">
                  {#each dayLabels as label}
                    <span class="day-label">{label}</span>
                  {/each}
                </div>
                <div class="heatmap-grid" style={heatmapColumnsStyle}>
                  {#each noteEditGrid as cell}
                    <button
                      type="button"
                      class="heatmap-cell"
                      class:selected={isSelectedCell("note", cell.date)}
                      style="grid-column: {cell.col + 1}; grid-row: {cell.row + 1}; background: {getNoteEditColor(cell.count)};"
                      aria-label={heatmapCellLabel("note", cell.date, cell.count)}
                      title={heatmapCellLabel("note", cell.date, cell.count)}
                      onclick={() => void selectHeatmapDay("note", cell.date, cell.count)}
                      onmouseenter={(e) => handleCellHover(e, cell.date, cell.count, "note")}
                      onmouseleave={handleCellLeave}
                    ></button>
                  {/each}
                </div>
              </div>
              <div class="heatmap-legend">
                <span class="legend-label">Less</span>
                <div class="legend-cell" style="background: var(--heatmap-empty);"></div>
                <div class="legend-cell" style="background: var(--note-heatmap-l1);"></div>
                <div class="legend-cell" style="background: var(--note-heatmap-l2);"></div>
                <div class="legend-cell" style="background: var(--note-heatmap-l3);"></div>
                <div class="legend-cell" style="background: var(--note-heatmap-l4);"></div>
                <span class="legend-label">More</span>
              </div>
            </div>
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
        {#if hoveredDay.label === "note"}
          <strong>{hoveredDay.count}</strong> note{hoveredDay.count !== 1 ? "s" : ""} edited
        {:else}
          <strong>{hoveredDay.count}</strong> task{hoveredDay.count !== 1 ? "s" : ""} completed
        {/if}
        <br><span class="tooltip-date">{formatDate(hoveredDay.date)}</span>
      </div>
    {/if}

    {#if selectedDay}
      <section class="day-detail" aria-live="polite">
        <div class="day-detail-header">
          <div>
            <h2 class="section-heading">
              {selectedDay.kind === "note" ? "Edited notes" : "Completed tasks"}
            </h2>
            <p>{formatDate(selectedDay.date)}</p>
          </div>
          <button type="button" class="day-detail-close" onclick={clearSelectedDay} aria-label="Close day details">×</button>
        </div>

        {#if selectedDay.kind === "note"}
          {#if noteEditsLoading}
            <div class="empty-state compact">Loading edited notes...</div>
          {:else if noteEditsForSelectedDay.length === 0}
            <div class="empty-state compact">No note edits recorded for this day.</div>
          {:else}
            <div class="note-edit-list">
              {#each noteEditsForSelectedDay as edit (`${edit.page_id ?? ""}:${edit.file_path ?? edit.page_title}`)}
                <button
                  type="button"
                  class="note-edit-row"
                  onclick={() => onNavigate?.(edit.page_title)}
                  title={`Open ${edit.page_title}`}
                >
                  <span class="note-edit-title">{edit.page_title}</span>
                  <span class="note-edit-meta">{noteEditMeta(edit)}</span>
                  {#if edit.file_path}
                    <span class="note-edit-path">{edit.file_path}</span>
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        {:else if selectedCompletedTasks.length === 0}
          <div class="empty-state compact">No completed tasks recorded for this day.</div>
        {:else}
          <div class="task-list">
            {#each selectedCompletedTasks as task (task.block_id)}
              <div class="task-item">
                <div class="task-body">
                  <button
                    class="task-content task-open"
                    onclick={(event) => handleRenderedTaskClick(event, task.block_id, task.page_title)}
                    title={`Open ${task.page_title}`}
                  >
                    <span class="rendered-content" use:hydrateRenderedMedia={task.content}>{@html renderBlock(task.content)}</span>
                  </button>
                  <span class="task-meta">
                    <!-- svelte-ignore a11y_click_events_have_key_events -->
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <span
                      class="task-page"
                      onclick={() => openTaskSource(task.page_title, task.block_id)}
                    >{task.page_title}</span>
                    <span class="task-time">{formatTime(task.timestamp)}</span>
                  </span>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    <!-- Open tasks, grouped by when they need a decision.
         "From earlier" rather than "Overdue", and no count badge on it: a
         growing red number reads as an accusation and drives avoidance rather
         than action. The oldest-task line below says the same thing in terms
         of time, which is something you can act on. -->
    <div class="open-tasks">
      <div class="section-heading-row">
        <h2 class="section-heading">
          Open
          <span class="section-count">{openTasks.length}</span>
        </h2>
        <div class="task-controls" aria-label="Open task display options">
          <label>
            <span>View</span>
            <select value={openTaskView} onchange={handleOpenTaskViewChange}>
              {#each OPEN_TASK_VIEW_OPTIONS as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </label>
          <label>
            <span>Sort</span>
            <select value={openTaskSort} onchange={handleOpenTaskSortChange}>
              {#each OPEN_TASK_SORT_OPTIONS as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </label>
        </div>
      </div>

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
        {#each groups as group (`${openTaskView}:${openTaskSort}:${group.id}`)}
          <div class="task-group">
            <div class="date-header">
              <span class="date-text">{group.label}</span>
              <span class="date-count">{group.tasks.length}</span>
            </div>
            <div class="task-list">
              {#each group.tasks as task (task.block_id)}
                <div class="task-item open">
                  <div class="task-body">
                    <button
                      class="task-content task-open"
                      onclick={(event) => handleRenderedTaskClick(event, task.block_id, task.page_title)}
                      title={`Open ${task.page_title}`}
                    >
                      <span class="rendered-content" use:hydrateRenderedMedia={task.content}>{@html renderBlock(task.content)}</span>
                    </button>
                    <span class="task-meta">
                      <!-- svelte-ignore a11y_click_events_have_key_events -->
                      <!-- svelte-ignore a11y_no_static_element_interactions -->
                      <span
                        class="task-page"
                        onclick={() => openTaskSource(task.page_title, task.block_id)}
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
                  <div class="task-body">
                    <button
                      class="task-content task-open"
                      onclick={(event) => handleRenderedTaskClick(event, task.block_id, task.page_title)}
                      title={`Open ${task.page_title}`}
                    >
                      <span class="rendered-content" use:hydrateRenderedMedia={task.content}>{@html renderBlock(task.content)}</span>
                    </button>
                    <span class="task-meta">
                      <!-- svelte-ignore a11y_click_events_have_key_events -->
                      <!-- svelte-ignore a11y_no_static_element_interactions -->
                      <span
                        class="task-page"
                        onclick={() => openTaskSource(task.page_title, task.block_id)}
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
    --note-heatmap-l1: color-mix(in srgb, var(--accent-blue, #60a5fa) 34%, var(--heatmap-empty));
    --note-heatmap-l2: color-mix(in srgb, var(--accent-blue, #60a5fa) 52%, var(--heatmap-empty));
    --note-heatmap-l3: color-mix(in srgb, var(--accent-blue, #60a5fa) 74%, var(--heatmap-empty));
    --note-heatmap-l4: var(--accent-cyan, #94e2d5);
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
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .activity-heatmaps {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }

  .activity-heatmap-panel {
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-secondary) 68%, transparent);
    overflow-x: auto;
    scrollbar-width: thin;
    scrollbar-color: var(--border) transparent;
  }

  .activity-heatmap-panel::-webkit-scrollbar {
    width: 2px;
    height: 2px;
  }

  .activity-heatmap-panel::-webkit-scrollbar-track {
    background: transparent;
  }

  .activity-heatmap-panel::-webkit-scrollbar-thumb {
    background: var(--border);
    border-radius: 1px;
  }

  .note-edit-heatmap {
    border-color: color-mix(in srgb, var(--accent-blue, var(--accent)) 36%, var(--border));
  }

  .heatmap-title-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 6px;
  }

  .heatmap-title-row h2 {
    margin: 0;
    color: var(--text-primary);
    font-size: 0.82rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .heatmap-title-row span {
    color: var(--text-muted);
    font-size: 0.72rem;
    white-space: nowrap;
  }

  .month-labels {
    display: grid;
    column-gap: 2px;
    width: max-content;
    margin-left: 24px;
    font-size: 0.65rem;
    color: var(--text-muted);
    margin-bottom: 2px;
    height: 14px;
  }

  .month-label {
    white-space: nowrap;
    overflow: hidden;
  }

  .heatmap-grid-area {
    display: flex;
    gap: 0;
    width: max-content;
    overflow: visible;
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
    gap: 2px;
    min-width: 0;
  }

  .heatmap-cell {
    border: none;
    padding: 0;
    appearance: none;
    border-radius: 2px;
    width: 100%;
    height: 100%;
    cursor: pointer;
    transition: outline 0.1s;
  }

  .heatmap-cell:focus-visible,
  .heatmap-cell:hover {
    outline: 2px solid var(--text-secondary);
    outline-offset: -1px;
  }

  .heatmap-cell.selected {
    outline: 2px solid var(--text-primary);
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

  .day-detail {
    margin: 16px 24px 0;
    padding: 14px;
    border: 1px solid color-mix(in srgb, var(--accent) 34%, var(--border));
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-secondary) 72%, transparent);
  }

  .day-detail-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }

  .day-detail-header .section-heading {
    margin: 0;
  }

  .day-detail-header p {
    margin: 3px 0 0;
    color: var(--text-muted);
    font-size: 0.78rem;
  }

  .day-detail-close {
    width: 28px;
    height: 28px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg-primary);
    color: var(--text-secondary);
    font: inherit;
    font-size: 1.15rem;
    line-height: 1;
    cursor: pointer;
  }

  .day-detail-close:hover,
  .day-detail-close:focus-visible {
    border-color: var(--accent);
    color: var(--text-primary);
  }

  .empty-state.compact {
    padding: 14px;
    font-size: 0.82rem;
  }

  .note-edit-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .note-edit-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 3px 12px;
    align-items: baseline;
    padding: 8px 10px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .note-edit-row:hover,
  .note-edit-row:focus-visible {
    background: var(--bg-hover, var(--bg-primary));
    outline: none;
  }

  .note-edit-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-link);
    font-weight: 600;
  }

  .note-edit-meta {
    color: var(--text-muted);
    font-size: 0.76rem;
    white-space: nowrap;
  }

  .note-edit-path {
    grid-column: 1 / -1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: 0.72rem;
  }

  /* Open tasks section */
  .open-tasks {
    padding: 24px 24px 0;
  }

  .section-heading-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }

  .section-heading-row .section-heading {
    margin: 0;
  }

  .task-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .task-controls label {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-muted);
    font-size: 0.75rem;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }

  .task-controls select {
    min-height: 30px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    font: inherit;
    font-size: 0.78rem;
    line-height: 1.2;
    padding: 4px 28px 4px 9px;
    text-transform: none;
    cursor: pointer;
  }

  .task-controls select:hover,
  .task-controls select:focus {
    border-color: var(--accent);
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

    .activity-heatmaps {
      width: 100%;
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
      position: static;
      top: auto;
      z-index: auto;
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
