Start a block with `TODO`, `DOING`, `DONE`, or `CANCELED` to give it a task state. The sample tasks live in [[Projects/Observatory Night/Plan]], so this guide does not add extra work to your dashboard.

**Scheduled** means when you intend to work on something. **Deadline** means when it is due. Use a task's date controls to pick either date, and clear a date when it no longer applies.

Try this:

1. Open the plan and choose one open task.
2. Set a scheduled date with its date picker.
3. Return to [[Projects/Task Dashboard]] and inspect the scheduled column.
4. Clear the date if you were only experimenting.

## See the work and the activity

Open **Tasks** with **Ctrl/Cmd+Shift+T**. Group and sort open tasks by due date or priority instead of visiting each source page.

Interactive completion and note-edit heatmaps let you explore activity. Flow metrics include cycle time, lead time, and on-time completion. These views describe recorded activity; an empty history is not a verdict on your work. Complete a sample task or edit a note, then explore what the view records.

The built-in view and [[Projects/Task Dashboard]] offer two approaches: use the ready-made controls, or write a focused SQL view of your own.

## Repeating work

For a recurring task, first choose a real date. In its saved `SCHEDULED` or `DEADLINE` timestamp, add a repeat interval before the closing `>`:

- `.+1w` — one week from when you complete it.
- `++1w` — keep the weekly cadence, advancing until the next date is in the future.
- `+1w` — advance exactly one week from the previous date, even if still in the past.

The units are `h`, `d`, `w`, `m`, and `y`. Completing a repeating task records the completion and reopens it for the next occurrence. Try recurrence on a practice task, not on work whose dates you need to preserve.

Task dates and the journal calendar serve different purposes. [[Journal And Calendar]] takes you to an entire daily page—including past and future dates.
