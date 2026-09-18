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

## Follow-up questions

Chat reads the conversation, so you don't have to repeat yourself:

> **You:** tell me about the VIVO X300 Ultra
> **You:** can you flash a global OS image onto it?

The second question never names the phone, and on its own it would send a web
search off after generic flashing guides. Grafium resolves "it" against the
turns before it, so research plans, picks sources and writes its answer knowing
what you're actually asking about — including when **Deep web research** is on.

Two things worth knowing:

- **A long conversation is summarised, not dropped.** Recent turns are replayed
  word for word; older ones fold into a recap so the transcript can't crowd out
  your notes and the sources being read.
- **Starting a new chat starts a new subject.** If a follow-up gets answered as
  if it were about something else, the earlier turns are still in scope — start
  a new chat, or name the subject explicitly in the question.

## Keeping several conversations

Chat is a list, not a single box. The panel on the left of Chat holds every
conversation you've started.

- **New chat** starts another one. The old one keeps its transcript.
- Each chat is named after its first question. Hover it and click **✎** to
  rename it, or **✕** to delete it.
- Opening Chat from a page gives that page its own conversation, so asking
  about one note doesn't disturb a thread about something else.

This is the practical fix for a follow-up being answered as if it were about an
earlier subject: keep separate subjects in separate chats, and each one's
history stays clean.

**Conversations stay on this machine.** They're stored inside the graph's
`.grafium` folder, which sync never touches. A chat you had here will not
appear on your phone, on a USB stick, or on a file server, and it can't be
pulled in from one — deliberately, because a conversation can quote notes the
other end has no business receiving. Your notes sync; your chats don't.

Grafium keeps the 50 most recent conversations and drops the oldest beyond
that. Treat them as working notes, not an archive: if an answer matters, ask
Chat to save it into a page.

## Running more than one at a time

Whether two chats can work at once depends on where your model runs, which you
set in **Settings → AI / Knowledge Engine**.

- **A model reached over the network** — a cloud provider, or a server you run
  like Ollama or vLLM — handles several requests at once. Ask in two chats and
  both work in parallel.
- **A model running inside Grafium** loads one model into memory and answers
  one question at a time. A second chat waits its turn.

When a chat is waiting you'll see *"Waiting for the model — next in line"*, and
the switcher shows how many are ahead of it. Turns are served in the order you
asked, so nothing gets stuck behind a later question. **Stop** works while
waiting too: it gives up the place in line immediately instead of holding
everyone else up.

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
