# Help - Chat

Chat can answer questions about your graph and help you work with selected
content. Use the chat input to ask a question, and use the right panel when
you want references or a focused answer.

For current outside information, use the research controls and review sources.

## Arrange conversations

The conversation list is docked immediately beside the main navigation menu.
Drag the divider between **Chats** and the conversation to resize it; your
adjustment is remembered on this device. Double-click the divider to reset.
You can also focus **Resize conversations** with Tab and use Left/Right
(Shift for larger steps), Home for the minimum, or End for the maximum.

**Alt+W** switches wide/narrow reading. In narrow mode, the left reading margin
becomes extra space for conversation titles instead of an empty gutter.
The right reading margin remains, and resizing keeps the active chat usable.
When the available Chat area is too small for both panels, **Chats** opens an
overlay drawer. Select a conversation, press Escape, or tap outside to close it.

## Message controls

The framed conversation area contains your messages; model diagnostics are not
chat messages. Type in the shaded composer at the bottom.
Small person and assistant icons sit beside **You** and **Grafium AI**; the
sender names remain visible. **Copy** beside a finished answer copies its
Markdown and sources, without the decorative icons.

- **Up arrow / Send:** send the message, or press Enter. Shift+Enter adds a line.
  While an answer, edit plan, or action analysis is running, the same control
  becomes a **Stop** square. It cancels that request; partial answers stay visible.
  Saving an already approved change is not interrupted.
- **Notes context:** choose which notes to include. **No notes** excludes note
  retrieval; **This page**, **Selection**, and the other choices set explicit scope.
- **Answer mode:** choose **Answer — no web**, **Web search**, or **Deep web research**.
- **Actions:** choose a Summary, Explain, or Compare prompt, or a reviewable
  note action. Prompt shortcuts fill the draft without sending it.
- **Model & index status icon**, beside Send: inspect the configured model,
  CPU/GPU status, index progress, and privacy guidance. Its icon and outline
  distinguish working, warning, and error states; keyboard and hover labels
  name the state. Open it for **Index now**, **Try faster mode** when supported,
  **Retry status** after a status failure, or **Change model**, which opens the
  AI section of Settings directly.

The status menu closes with Escape or a click outside it. Actionable model
errors remain flagged beside the composer; open **Model status** for actions.
Handled slower-mode fallback does not repeatedly ask for attention. Long
diagnostics stay under **Technical details**. Retrying never bypasses memory
safety checks, but normal recovery needs no approval.

## Ask AI to organize and draft

Use ordinary requests, or choose **Actions** inside the composer:

- **Group open tasks:** "Group all my TODOs by similarity and flag possible
  duplicates." Grafium reads the current graph's open tasks and proposes a new
  reference page with topic groups and advisory duplicate candidates.
- **Find related topics:** open the source note, then ask "Find notes related to
  this page and create a page with references." The proposal links real notes
  and explains the connections; it does not invent destination pages.
- **Clean up draft:** choose **Selection**, **Block including children**,
  **Section / Chapter**, or **This page**, then ask "Clean up these rough notes."
  Apply appends a **Suggested rewrite** below the original on that same page.
  The original is not replaced or deleted.
  Task-looking draft lines are quoted rather than becoming additional tasks.
  Cleanup can reword and organize complete paragraphs or sibling groups while
  retaining links, numbers, and nested notes. Large drafts are organized within
  disclosed batches; review their meaning and overall order before applying.

The action scope is shown before sending. Task grouping uses all open tasks in
the current graph; topic discovery reads saved Markdown notes in that graph.
Private Library books, listening history, and bookmarks are excluded. These
actions use the configured model, without web searches or an embedding-index
requirement. An external model receives the supplied note/task text according
to your normal AI settings. Large scopes take multiple model requests and may
incur provider charges. **Stop analysis** cancels without saving a draft.

Every result has an editable **Proposed changes** card. Inspect the readable
task references, coverage, and uncertainty; use **Edit draft** to revise the
text, headings, or grouping. Choose a new page title where offered.
**Apply** is the only save action. **Dismiss** returns the request to the
composer. Failed or cancelled analysis never silently applies partial output.
Oversized inputs and model-context limits produce an explicit error or disclosed
coverage, not a claim to have read material that was skipped.
Snapshots allow up to 4,096 pages, 10,000 blocks, and 4 MiB of source text.
Previews expire after 30 minutes; regenerate an expired preview before applying.
Unsupported Markdown structures are rejected rather than silently flattened.

Task groups are a saved review page of `((block-id))` references, **not copied
TODOs** and not a rewrite of the Tasks query. Original task order, dates, and
states remain unchanged. Related work is not necessarily duplicate work:
review the source tasks yourself before merging, cancelling, or deleting any.
The grouping is a snapshot; rerun the request after substantial source changes.

Apply checks the captured graph and source snapshot again. If notes changed
during review, regenerate the proposal rather than overwriting newer work.
Existing destination pages are not silently reused. Insertions support Undo;
undoing a new reference page's content may leave its empty page behind.
Unapplied proposals are session UI state, not durable notes: Apply before
closing the conversation or application if you want to keep one.

## Watching it work

The **Model & index status** menu uses short status messages. Insufficient
or unknown GPU headroom selects CPU when RAM permits; retrying GPU does not
override the admission checks. After an unexpected worker exit, Grafium permits
one automatic recovery attempt on the next model request. If that fails, it
remembers slower mode across restarts. **Keep slower mode** skips a pending
attempt; **Try faster mode** requests one more attempt explicitly. No model is
silently replaced and no request is sent to another provider.
See [[AI Setup And Privacy]] for worker shutdown, eviction, crash recovery, and
device-specific GPU-memory detection.

Deleted or changed source content is removed from the vector index by background
maintenance even when AI is disabled or no embedding model is configured.
Generating replacement vectors waits for an embedding model. Rebuilding the
graph index also queues removal of old vector identities; an in-flight embedding
cannot publish a source snapshot that changed or was deleted.

After large imports, syncs, or rebuilds, embedding progress appears in the
activity list as **Building AI search index**; answers about content that is
still being embedded may miss semantic matches until it reports **AI search
index is up to date**.

To keep startup fast, background embedding waits about a minute after Grafium
opens; models load only when Chat, summaries, or indexing need them. The AI
search index stores compact 8-bit vectors and returns space freed by deleted
or changed content when a graph opens and about hourly afterwards.

Original books use extracted, read-only source text for page/book context.
Write your own annotations in **Notes**, not into the original.
See [[Help - Books]] for extraction limits and source-preserving import.

While an answer is being produced, Chat stacks what it is doing in the
transcript itself, above the answer: `Searching your notes`, `Thinking`,
`Generating`, and for web research `Planning searches`, `Reading sources`,
`Refining the search`. The current status shimmers immediately, including while
Chat waits for the first detailed step, and a running total ticks beside it.
The sweep moves from muted grey to your theme's foreground colour, including
white-on-black themes; it does not change the size of the status text.
Finished steps stay put with the time each one took, so you can see where a slow
answer actually spent its time.

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

- **A long conversation is shortened, not replayed in full.** Recent turns are
  sent word for word; older ones are cut down to a recap so the transcript
  can't crowd out your notes and the sources being read. **What Chat
  remembers**, below, explains exactly what survives.
- **Starting a new chat starts a new subject.** If a follow-up gets answered as
  if it were about something else, the earlier turns are still in scope — start
  a new chat, or name the subject explicitly in the question.

## What Chat remembers

A model can only read so much at once, and every conversation competes for that
space with your notes and any web sources being read.

**While a conversation is short, all of it is sent** — every turn, exactly as
written. Nothing below applies until the transcript stops fitting. When it
does, Chat sends:

- **The last four turns.** These arrive as written unless one of them is very
  long on its own, in which case its middle is dropped and marked *"[... excerpt
  shortened to fit the model context ...]"* — the beginning and end both
  survive.
- **A recap of everything older**, under the heading *"Earlier in this
  conversation:"*. Each older turn is cut to its **first 220 characters** — one
  line, speaker labelled. The recap as a whole stops at about **4,000
  characters**; past that you'll see *"(earlier turns omitted)"*.
- **Less of both if it still doesn't fit.** The transcript gets roughly a third
  of the context budget, and if the prompt is still too big Grafium halves the
  transcript's share, repeatedly, until it does.

### What that means in practice

**The recap is a truncation, not a summary.** No model reads your old turns and
writes a précis of them. A turn is kept by being *recent*, and shortened by
being *cut off* — so a detail buried in the middle of a long answer forty turns
ago is simply gone, not condensed.

**Recall is by recency, not by relevance.** Chat cannot go looking through the
earlier parts of a conversation for the bit that matters to your current
question. If it has scrolled out of the last four turns, the only trace left is
that first 220 characters. Your *notes* are searched by meaning; your
conversation history is not.

So, concretely:

> **You:** *(turn 3)* the serial number is FQ7-88213-XK
> **You:** *(turn 40)* what was that serial number again?

Chat will not have it. Turn 3 is long gone from the verbatim window, and if the
number wasn't in the first 220 characters of that turn it isn't in the recap
either.

### Working with this rather than against it

- **Keep separate subjects in separate chats.** A short conversation is one
  where nothing has been cut yet.
- **Put anything that matters into a note.** Ask Chat to save an answer to a
  page — notes are searched properly, by meaning, and they last. A conversation
  is scratch paper.
- **Say the subject again when you come back to it.** Re-stating "the VIVO
  X300" costs you a few words and puts it back in the verbatim window.
- **Reopening an old chat shows you the whole transcript** — the truncation is
  about what the *model* is sent, not about what you can read. Everything you
  see on screen is still there.

## Keeping several conversations

Chat is a list, not a single box. The panel on the left of Chat holds every
conversation you've started.

- **New chat** starts another one. The old one keeps its transcript.
- Each chat is named after its first question, then renamed to something
  shorter once the model has seen the answer — a question about flashing a
  phone becomes "Flashing a global OS image". If no model is loaded, or it
  replies with nothing useful, the question stays as the name.
- **Right-click a chat** to rename or delete it. The same two actions are on
  the **✎** and **✕** buttons that appear when you hover it.
- Renaming a chat yourself is final: Grafium won't overwrite a name you typed.
- Opening Chat from a page gives that page its own conversation, so asking
  about one note doesn't disturb a thread about something else.

## Clearing chats out

The **trash** button at the top of the chat list does one of two things,
depending on whether you have anything selected.

- With **nothing selected**, it deletes every chat in the list.
- **Hover a chat** to reveal a checkbox on its left. Tick one and the trash
  button switches to deleting only what you ticked. Shift-click a second chat
  to take everything between the two. The heading changes to "3 selected" so
  you can see the button's reach before you press it.
- Either way you're asked to confirm, and the question names exactly what is
  about to go — "Delete all 7 chats?" or "Delete 3 selected chats?". If any of
  them are mid-answer, it says so: deleting stops that work.
- **Escape** clears the selection.

Deleting is permanent. There is no undo and no trash to recover from, so read
the confirmation rather than clicking through it.

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
