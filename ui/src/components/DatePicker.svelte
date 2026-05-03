<script lang="ts">
  interface Props {
    x: number;
    y: number;
    onSelect: (date: string) => void;
    onCancel: () => void;
  }

  let { x, y, onSelect, onCancel }: Props = $props();

  let today = new Date();
  let viewYear = $state(today.getFullYear());
  let viewMonth = $state(today.getMonth()); // 0-indexed

  const DAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

  function daysInMonth(year: number, month: number): number {
    return new Date(year, month + 1, 0).getDate();
  }

  function firstDayOfWeek(year: number, month: number): number {
    // 0=Mon, ..., 6=Sun
    const d = new Date(year, month, 1).getDay();
    return d === 0 ? 6 : d - 1;
  }

  function prevMonth() {
    if (viewMonth === 0) {
      viewMonth = 11;
      viewYear--;
    } else {
      viewMonth--;
    }
  }

  function nextMonth() {
    if (viewMonth === 11) {
      viewMonth = 0;
      viewYear++;
    } else {
      viewMonth++;
    }
  }

  function selectDate(day: number) {
    const m = String(viewMonth + 1).padStart(2, "0");
    const d = String(day).padStart(2, "0");
    onSelect(`${viewYear}-${m}-${d}`);
  }

  function isToday(day: number): boolean {
    return viewYear === today.getFullYear() && viewMonth === today.getMonth() && day === today.getDate();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    }
  }

  // Compute position to stay within viewport
  let style = $derived((() => {
    let top = y;
    let left = x;
    // Keep within reasonable bounds
    if (left + 260 > window.innerWidth) left = window.innerWidth - 270;
    if (top + 300 > window.innerHeight) top = y - 310;
    if (left < 10) left = 10;
    if (top < 10) top = 10;
    return `top:${top}px;left:${left}px`;
  })());
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="date-picker-backdrop" onclick={onCancel}>
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="date-picker" style={style} onclick={(e) => e.stopPropagation()}>
    <div class="dp-header">
      <button class="dp-nav" onclick={prevMonth}>&lsaquo;</button>
      <span class="dp-title">{MONTHS[viewMonth]} {viewYear}</span>
      <button class="dp-nav" onclick={nextMonth}>&rsaquo;</button>
    </div>
    <div class="dp-days-header">
      {#each DAYS as d}
        <span class="dp-day-name">{d}</span>
      {/each}
    </div>
    <div class="dp-grid">
      {#each Array(firstDayOfWeek(viewYear, viewMonth)) as _}
        <span class="dp-cell empty"></span>
      {/each}
      {#each Array(daysInMonth(viewYear, viewMonth)) as _, i}
        <button
          class="dp-cell"
          class:today={isToday(i + 1)}
          onclick={() => selectDate(i + 1)}
        >
          {i + 1}
        </button>
      {/each}
    </div>
    <div class="dp-footer">
      <button class="dp-today-btn" onclick={() => { viewYear = today.getFullYear(); viewMonth = today.getMonth(); selectDate(today.getDate()); }}>Today</button>
      <button class="dp-clear-btn" onclick={() => onSelect("")}>Clear</button>
    </div>
  </div>
</div>

<style>
  .date-picker-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 1000;
  }

  .date-picker {
    position: fixed;
    width: 250px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    z-index: 1001;
  }

  .dp-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .dp-nav {
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 18px;
    padding: 4px 8px;
    border-radius: 4px;
  }

  .dp-nav:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .dp-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .dp-days-header {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
    margin-bottom: 4px;
  }

  .dp-day-name {
    font-size: 0.65rem;
    color: var(--text-muted);
    text-align: center;
    padding: 2px 0;
  }

  .dp-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
  }

  .dp-cell {
    aspect-ratio: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.75rem;
    border-radius: 4px;
    border: none;
    background: none;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .dp-cell:not(.empty):hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .dp-cell.today {
    background: var(--accent);
    color: white;
    font-weight: 600;
  }

  .dp-cell.empty {
    cursor: default;
  }

  .dp-footer {
    display: flex;
    justify-content: space-between;
    margin-top: 8px;
    gap: 8px;
  }

  .dp-today-btn,
  .dp-clear-btn {
    flex: 1;
    padding: 4px 8px;
    font-size: 0.7rem;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  .dp-today-btn:hover,
  .dp-clear-btn:hover {
    background: var(--accent);
    color: white;
  }
</style>
