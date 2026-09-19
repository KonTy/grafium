Chat is optional. Set up a provider before expecting AI answers; the core
note-taking examples do not require one. Start with [[AI Setup And Privacy]]
before connecting an account or downloading a model.

## Choose sources separately from the provider

**Local graph** searches your notes without web search. **Internet** enables web search for the answer. The scope is remembered across chats and restarts.

These labels choose **where to search**, not **where the model runs**. With a cloud provider, your prompt and retrieved note context are sent to that provider—even in Local graph scope. Do not include material you are not comfortable sending.

A local provider requires a suitable model, setup, and enough memory and compute. Model downloads and optional services can involve network traffic. Local graph scope is not a promise of “no internet traffic.”

## What AI can do

- Answer questions about a selected page, journal day, block, or your graph.
- Find relevant blocks and explain how they support an answer.
- Run multi-step web research with citations when Internet and Research are enabled.
- Suggest concepts and links for review; suggestions do not become links until you accept them.
- Help rewrite, summarize, assess writing style, and create study material.
- Process speech or media when the relevant optional model is installed.
- File an answer for you: say `add the above answer to today's journal under
  [[health/supplements]]` or `save that as a new page and find links it can
  connect to`. Grafium shows an editable **Proposed changes** card first, and
  nothing is written until you press Apply. `Ctrl+Z` undoes it afterwards.

Generated answers are drafts, not authoritative sources. Verify important
claims and save only checked conclusions back into your notes.

## What Chat forgets

Chat keeps your conversations and reloads them when you reopen Grafium, but the
model is not sent all of one. Recent turns go in full unless one is enormous;
older ones are cut to a single short line each. The cut is mechanical — nothing
summarizes your old turns, and nothing searches them for the part relevant to
your question.

So a detail you mentioned thirty turns ago is likely gone, even though you can
still scroll up and read it. Your **notes** are searched by meaning and they
last; a conversation is scratch paper. Ask Chat to save anything worth keeping
into a page.

Clear the scratch paper with the **trash** button above the chat list: it takes
every chat when nothing is selected, or just the ones you tick. [[Help - Chat]]
sets out exactly what survives and what doesn't.

## Ask a question you can check

Try asking for the distinction between aperture and magnification, then compare the answer with [[Space/Telescopes]]. Ask which notes support the answer. Treat generated text as a draft to verify, not a new source of truth.

With **Internet** selected, **Research** runs a multi-step process: plan searches, read sources, look for gaps, and synthesize a cited answer. Research is unavailable in Local graph scope; its preference returns when you switch back to Internet.

Follow citations and check whether a source actually supports the claim. Keep a useful, verified explanation in [[Learning/Reading Notes]], with its source information.
