# Help - Chat

Chat can answer questions about your graph and help you work with selected
content. Use the chat input to ask a question, and use the right panel when
you want references or a focused answer.

For current outside information, use the research controls and review sources.

## Watching it work

While an answer is being produced, Chat stacks what it is doing in the
transcript itself, above the answer: `Searching your notes`, `Thinking`,
`Generating`, and for web research `Planning searches`, `Reading sources`,
`Refining the search`. The step that is currently running shimmers, and a
running total ticks beside it. Finished steps stay put with the time each one
took, so you can see where a slow answer actually spent its time.

Each step appears only when the model or the research engine really reported
it. If progress stops, the shimmer stops too and the row says so rather than
animating over a wedged request — an indicator that keeps moving after
something has died is worse than no indicator.

Once an answer is done its steps collapse into a single **Worked for…** line.

The same shimmer is used everywhere in Grafium that something is loading —
graph building, search, indexing, saving. It always means the same thing:
work is in flight and behaving normally. Where Grafium has no way to observe
real progress, the shimmer is deliberately given a time budget of about twenty
seconds; if the work outlives it the text simply goes still. Still text means
"this is taking longer than it should", so motion never over-promises.
Click it to expand the trail for that answer again, even after you have asked
something else.

If you prefer reduced motion in your system settings, nothing shimmers; the
steps and timings still appear.

## Search engines for web research

Deep research does not stop at the search results page. It reads them: every
result it decides is worth opening is fetched and parsed as a full document —
HTML, PDFs, and scanned PDFs via OCR — and it repeats that search-read-assess
cycle over several rounds, refining its queries toward whatever is still
missing. Citations point at pages it actually read.

**Settings → Research** lists the engines. Brave and DuckDuckGo are on by
default; academic sources (OpenAlex, Crossref, arXiv, Europe PMC, Semantic
Scholar, DOAJ, PubMed, Google Patents, Open Library) are free and need no key.

### Privacy-focused engines

Grafium fetches with a plain HTTP client and does not run JavaScript, which
decides what is reachable:

| Engine | Status |
| --- | --- |
| Mojeek | Built in, independent index, scrapes cleanly. Enable it. |
| SearXNG | Built in, disabled. Point it at **your own** instance. |
| Startpage | Built in, disabled. Often answers automation with a robot check. |
| Qwant | Not available — results are rendered in the browser, and its API is behind bot protection. |
| Swisscows | Not available — same, and its API rejects unsigned requests. |

Running your own **SearXNG** is the way to get the rest. It queries Qwant,
Startpage, Swisscows, Brave and others server-side and returns plain HTML, so
one instance gives you all of them with no bot walls and no per-engine
maintenance. Public instances will not work — every one tested answers an
automated client with a captcha — so this must be an instance you host.

Enable **SearXNG (self-hosted)** and set its URL to your instance; the default
assumes `http://localhost:8080`. You can also add any other engine yourself
under **Add engine** with its URL template and CSS selectors.

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
