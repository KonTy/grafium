# Help - Chat

Chat can answer questions about your graph and help you work with selected
content. Use the chat input to ask a question, and use the right panel when
you want references or a focused answer.

For current outside information, use the research controls and review sources.

## Asking Chat to change your notes

Chat can also *do* things, not just answer. Ask in plain language:

- `add the above answer to today's journal and file it under [[health/supplements]]`
- `save this answer as a new page called Supplements and find links it can connect to`
- `add that as a task for tomorrow`
- `tag this page with [[longevity]]`

Grafium replies with a **Proposed changes** card instead of writing anything.
The card shows each change in plain language, and the heading, tags and text are
all editable. **Nothing touches your notes until you press Apply.** Press
**Dismiss** to throw the plan away — your request goes back into the box so you
can reword it.

After you apply, `Ctrl+Z` undoes the change. If a plan wrote to more than one
page the notice tells you how many undo steps it takes.

Chat only offers a plan when your message reads like an instruction. Ordinary
questions are answered exactly as before, so nothing gets slower.

### What it can do

| Ask for | What happens |
| --- | --- |
| A journal entry | Adds to that day's page, creating the day if needed |
| A new page | Creates the page and adds the text |
| Adding to a page | Appends to the end; existing notes are never overwritten |
| Tags | Adds real `[[links]]`, so the page shows up in backlinks |
| A task | Adds a `TODO` bullet the task board picks up |
| Finding links | Runs link suggestions on the page it just wrote |
| Rewriting a block | Replaces the block this conversation is about |

Tags keep their nesting: `[[health/supplements]]` stays under `health`.

### Why it works this way

The answer is reused word for word rather than retyped by the model, so a long
answer is never quietly shortened on its way into your notes. Everything is an
append except an explicit block rewrite, and that rewrite is checked against the
version of the block Grafium read — if you edited it in the meantime the write is
refused rather than overwriting your edit.
