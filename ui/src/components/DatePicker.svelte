<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    calendarDate,
    calendarDateKey,
    calendarMonthLength,
    mondayWeekday,
    navigateCalendarDay,
    navigateCalendarGrid,
    parseCalendarDate,
    shiftCalendarMonth,
  } from "../lib/datePickerNavigation";

  interface Props {
    x: number;
    y: number;
    showClear?: boolean;
    /** Initial local YYYY-MM-DD focus; later updates never move keyboard focus. */
    selectedDate?: string;
    markedDates?: readonly string[];
    /** Visible day month only, with a 1-based month. */
    onMonthChange?: (year: number, month: number) => void;
    onSelect: (date: string) => void;
    onCancel: () => void;
  }

  let { x, y, showClear = true, selectedDate, markedDates = [], onMonthChange, onSelect, onCancel }: Props = $props();

  const today = new Date();
  const todayKey = calendarDateKey(today);
  const initialDate = untrack(() => parseCalendarDate(selectedDate)) ?? calendarDate(today.getFullYear(), today.getMonth(), today.getDate());
  let focusedDate = $state(initialDate);
  let viewYear = $derived(focusedDate.getFullYear());
  let viewMonth = $derived(focusedDate.getMonth()); // 0-indexed
  let viewMode = $state<"days" | "months" | "years">("days");
  const YEAR_PAGE = 12;
  let yearPageStart = $derived(Math.floor(viewYear / YEAR_PAGE) * YEAR_PAGE);
  let markers = $derived(new Set(markedDates));
  let picker: HTMLDivElement;
  let mounted = false;
  const id = $props.id();

  const DAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
  const DAY_NAMES = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const MONTH_NAMES = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
  const dateLabel = new Intl.DateTimeFormat("en", { weekday: "long", year: "numeric", month: "long", day: "numeric" });
  let monthStart = $derived(mondayWeekday(calendarDate(viewYear, viewMonth)));
  let monthLength = $derived(calendarMonthLength(viewYear, viewMonth));
  let calendarTitle = $derived(viewMode === "years"
    ? `${yearPageStart}–${yearPageStart + YEAR_PAGE - 1}`
    : viewMode === "months" ? `Choose month in ${viewYear}` : `${MONTH_NAMES[viewMonth]} ${viewYear}`);

  let lastNotifiedMonth = "";
  $effect(() => {
    if (viewMode !== "days") return;
    const year = viewYear;
    const month = viewMonth + 1;
    const key = `${year}-${month}`;
    if (key === lastNotifiedMonth) return;
    lastNotifiedMonth = key;
    // Data arriving from the caller must not subscribe this effect or move focus.
    untrack(() => onMonthChange?.(year, month));
  });

  onMount(() => {
    const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = picker;
    let ownsFocus = false;
    const trackFocus = (event: FocusEvent) => {
      ownsFocus = dialog.contains(event.target as Node);
    };
    document.addEventListener("focusin", trackFocus);
    mounted = true;
    dialog.querySelector<HTMLButtonElement>('[data-calendar-cell][tabindex="0"]')?.focus({ preventScroll: true });

    return () => {
      mounted = false;
      document.removeEventListener("focusin", trackFocus);
      const restore = dialog.contains(document.activeElement) || (ownsFocus && document.activeElement === document.body);
      void tick().then(() => {
        if (restore && invoker?.isConnected && (document.activeElement === document.body || dialog.contains(document.activeElement))) {
          invoker.focus({ preventScroll: true });
        }
      });
    };
  });

  async function focusCell() {
    const previous = document.activeElement;
    await tick();
    if (mounted && (picker.contains(document.activeElement) || (document.activeElement === document.body && previous && !previous.isConnected))) {
      picker.querySelector<HTMLButtonElement>('[data-calendar-cell][tabindex="0"]')?.focus({ preventScroll: true });
    }
  }

  function changePage(direction: number) {
    focusedDate = shiftCalendarMonth(focusedDate, direction * (viewMode === "years" ? YEAR_PAGE * 12 : viewMode === "months" ? 12 : 1));
  }

  function changeMode(mode: "days" | "months" | "years") {
    viewMode = mode;
    void focusCell();
  }

  function selectMonth(month: number) {
    focusedDate = shiftCalendarMonth(focusedDate, month - viewMonth);
    changeMode("days");
  }

  function selectYear(year: number) {
    focusedDate = shiftCalendarMonth(focusedDate, (year - viewYear) * 12);
    changeMode("months");
  }

  function selectDate(day: number) {
    onSelect(calendarDateKey(calendarDate(viewYear, viewMonth, day)));
  }

  function selectToday() {
    focusedDate = calendarDate(today.getFullYear(), today.getMonth(), today.getDate());
    onSelect(todayKey);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.isComposing || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "Tab") {
      const buttons = [...picker.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')].filter((button) => button.tabIndex >= 0);
      const first = buttons[0];
      const last = buttons[buttons.length - 1];
      if (e.shiftKey && (document.activeElement === first || document.activeElement === picker)) {
        e.preventDefault();
        last?.focus();
      } else if (!e.shiftKey && (document.activeElement === last || document.activeElement === picker)) {
        e.preventDefault();
        first?.focus();
      }
      e.stopPropagation();
      return;
    }
    if (e.shiftKey && e.key !== "PageUp" && e.key !== "PageDown") return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
      return;
    }
    if (e.key === "Enter" || e.key === " ") {
      // Keep native button activation, without letting application shortcuts see it.
      e.stopPropagation();
      return;
    }
    if (!(e.target instanceof HTMLElement) || !e.target.hasAttribute("data-calendar-cell")) return;

    let next: Date | null = null;
    if (viewMode === "days") {
      next = navigateCalendarDay(focusedDate, e.key, e.shiftKey);
    } else if (!e.shiftKey) {
      const index = viewMode === "months" ? viewYear * 12 + viewMonth : viewYear;
      const moved = navigateCalendarGrid(index, e.key);
      if (moved !== null) {
        const months = viewMode === "months" ? moved - index : (Math.max(1, Math.min(9999, moved)) - viewYear) * 12;
        next = shiftCalendarMonth(focusedDate, months);
      }
    }
    if (next) {
      e.preventDefault();
      e.stopPropagation();
      focusedDate = next;
      void focusCell();
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

<div class="date-picker-backdrop" role="presentation" onclick={onCancel}>
  <div
    bind:this={picker}
    class="date-picker"
    style={style}
    role="dialog"
    aria-modal="true"
    aria-label="Choose date"
    aria-describedby={`${id}-instructions`}
    tabindex="-1"
    onkeydown={handleKeydown}
    onclick={(e) => e.stopPropagation()}
  >
    <p class="dp-sr-only" id={`${id}-instructions`}>Use arrow keys to move, Home and End for the first and last cell in a row, and Page Up or Page Down to change pages. In the day calendar, hold Shift with Page Up or Page Down to change years. Enter or Space chooses, Escape closes. Tab moves between controls.</p>
    <span class="dp-sr-only" aria-live="polite" aria-atomic="true">{calendarTitle}</span>
    <div class="dp-header">
      <button class="dp-nav" type="button" onclick={() => changePage(-1)} aria-label={viewMode === "years" ? "Previous 12 years" : viewMode === "months" ? "Previous year" : "Previous month"}>&lsaquo;</button>
      <div class="dp-title-group">
        {#if viewMode === "years"}
          <span class="dp-title">{yearPageStart}–{yearPageStart + YEAR_PAGE - 1}</span>
        {:else}
          {#if viewMode === "days"}
            <button class="dp-title-btn" type="button" onclick={() => changeMode("months")} aria-label={`Choose month, ${MONTH_NAMES[viewMonth]}`}>{MONTHS[viewMonth]}</button>
          {/if}
          <button class="dp-title-btn" type="button" onclick={() => changeMode("years")} aria-label="Choose year">{viewYear}</button>
        {/if}
      </div>
      <button class="dp-nav" type="button" onclick={() => changePage(1)} aria-label={viewMode === "years" ? "Next 12 years" : viewMode === "months" ? "Next year" : "Next month"}>&rsaquo;</button>
    </div>
    {#if viewMode === "months"}
      <div class="dp-month-grid" role="grid" aria-label={`Months in ${viewYear}`}>
        {#each Array(4) as _, row}
          <div class="dp-row" role="row">
            {#each Array(3) as _, column}
              {@const index = row * 3 + column}
              <div class="dp-choice-cell" role="gridcell" aria-selected={index === viewMonth}>
                <button
                  class="dp-choice"
                  class:current={index === viewMonth}
                  class:today={viewYear === today.getFullYear() && index === today.getMonth()}
                  type="button"
                  data-calendar-cell
                  tabindex={index === viewMonth ? 0 : -1}
                  aria-label={`${MONTH_NAMES[index]} ${viewYear}`}
                  onclick={() => selectMonth(index)}
                >
                  {MONTHS[index]}
                </button>
              </div>
            {/each}
          </div>
        {/each}
      </div>
    {:else if viewMode === "years"}
      <div class="dp-month-grid" role="grid" aria-label={`Years ${calendarTitle}`}>
        {#each Array(4) as _, row}
          <div class="dp-row" role="row">
            {#each Array(3) as _, column}
              {@const year = yearPageStart + row * 3 + column}
              <div class="dp-choice-cell" role="gridcell" aria-selected={year === viewYear}>
                <button
                  class="dp-choice"
                  class:current={year === viewYear}
                  class:today={year === today.getFullYear()}
                  type="button"
                  data-calendar-cell
                  tabindex={year === viewYear ? 0 : -1}
                  disabled={year < 1 || year > 9999}
                  onclick={() => selectYear(year)}
                >
                  {year}
                </button>
              </div>
            {/each}
          </div>
        {/each}
      </div>
    {:else}
      <div role="grid" aria-label={calendarTitle}>
        <div class="dp-days-header" role="row">
          {#each DAYS as d, index}
            <span class="dp-day-name" role="columnheader" aria-label={DAY_NAMES[index]}>{d}</span>
          {/each}
        </div>
        <div class="dp-grid" role="rowgroup">
          {#each Array(Math.ceil((monthStart + monthLength) / 7)) as _, row}
            <div class="dp-row" role="row">
              {#each Array(7) as _, column}
                {@const day = row * 7 + column - monthStart + 1}
                {#if day < 1 || day > monthLength}
                  <span class="dp-cell empty" role="gridcell"></span>
                {:else}
                  {@const date = calendarDate(viewYear, viewMonth, day)}
                  {@const key = calendarDateKey(date)}
                  {@const hasNotes = markers.has(key)}
                  <div role="gridcell" aria-selected={key === selectedDate}>
                    <button
                      class="dp-cell"
                      class:today={key === todayKey}
                      class:has-notes={hasNotes}
                      type="button"
                      data-calendar-cell
                      data-date={key}
                      tabindex={day === focusedDate.getDate() ? 0 : -1}
                      aria-label={`${dateLabel.format(date)}${hasNotes ? ", has notes" : ""}`}
                      aria-current={key === todayKey ? "date" : undefined}
                      onfocus={() => { focusedDate = date; }}
                      onclick={() => selectDate(day)}
                    >
                      <span class="dp-day-number">{day}</span>
                    </button>
                  </div>
                {/if}
              {/each}
            </div>
          {/each}
        </div>
      </div>
    {/if}
    <div class="dp-footer">
      <button class="dp-today-btn" type="button" onclick={selectToday}>Today</button>
      {#if showClear}
        <button class="dp-clear-btn" type="button" onclick={() => onSelect("")}>Clear</button>
      {/if}
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

  .dp-sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .date-picker button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
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
    min-width: 32px;
    min-height: 32px;
    padding: 4px 8px;
    border-radius: 4px;
  }

  .dp-nav:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .dp-title-group {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .dp-title,
  .dp-title-btn {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
    background: none;
    border: none;
    padding: 4px 6px;
    border-radius: 4px;
  }

  .dp-title-btn {
    cursor: pointer;
  }

  .dp-title-btn:hover {
    background: var(--bg-hover);
  }

  .dp-month-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
    min-height: 168px;
  }

  .dp-row {
    display: contents;
  }

  .dp-choice-cell {
    display: flex;
  }

  .dp-choice {
    width: 100%;
    border: none;
    border-radius: 6px;
    padding: 10px 4px;
    background: none;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 12px;
  }

  .dp-choice:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .dp-choice.current {
    outline: 1px solid var(--border);
  }

  .dp-choice.today {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
    font-weight: 600;
  }

  .dp-choice:disabled {
    opacity: 0.4;
    cursor: default;
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
    width: 100%;
    padding: 0;
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
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
    font-weight: 600;
  }

  .dp-day-number {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.7em;
    height: 1.7em;
    border: 1px solid transparent;
    border-radius: 50%;
  }

  .dp-cell.has-notes .dp-day-number {
    border-color: var(--text-muted);
  }

  .dp-cell.today.has-notes .dp-day-number {
    border-color: currentColor;
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
    /* This popover ships to a phone; 4px padding on 0.7rem text gave a ~22px
       tap target. */
    flex: 1 0 auto;
    min-height: 32px;
    padding: 6px 8px;
    font-size: 0.7rem;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  .dp-today-btn:hover,
  .dp-clear-btn:hover {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
  }
</style>
