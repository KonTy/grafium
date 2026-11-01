# Help - IQ Assessments

Open **Flashcards > Import pack** to import an IQ-marked JSON file, or an
Anki `.apkg` with a `grafium-iq.json` descriptor. Ordinary Anki decks retain
their existing recall-review behavior. A tag or filename containing "IQ"
alone is not enough: an assessment needs explicit accepted answers.

## Taking an assessment

Choose a pack in **IQ-marked packs**. Read its conditions before starting.
Submit a typed answer or **Skip** each question in order. Answers are compared
after trimming/collapsing whitespace and ignoring case; punctuation and
numeric formatting still matter. A blank or skipped answer earns zero.
No partial credit or AI grading is used. Questions are plain text; grid
layouts retain their line breaks.

Submitted answers are saved in the graph database. They do not grade any
spaced-repetition card or change its next review. Answers and explanations
become available only after completion. A timed test has no pauses: switching
away, sleep, and inactivity still consume its time. At expiry, remaining
questions count as unanswered. A failed answer/result save is shown explicitly;
retry without double-counting a successfully saved answer.

**Finish early (reasoning score only)** saves the partial result and reveals
the answer key; unattempted items earn zero. It cannot produce an IQ estimate
before the timed limit or completion of the full test.

Closing or navigating away abandons the visible attempt; resuming its
question screen is not currently supported. Saved responses remain in the
database; starting an assessment counts as previous exposure even without
submitted answers. Keep the graph database in backups:
assessment packs and attempts, like Studies history, are durable data and
are not recreated from the ordinary flashcard index.

## Reasoning score versus approximate IQ

All packs report correct/total, percentage, unanswered questions and elapsed
time. Without documented population norms, **no IQ number is calculated**.
These generated or authored exercises are not automatically clinical tests.

For a qualifying normed pack, Grafium uses the standard modern deviation
formula **IQ = 100 + 15z**: mean 100, standard deviation 15. The z-score for
the exact raw score comes from the pack's norm table, not from an arbitrary
percentage-to-IQ conversion or the historical mental-age formula.

Norms must match the exact ordered item set, test version, timing, age group,
language and stated instructions. Enter your age and language honestly.
Confirm that you will follow the instructions without aids. Prior practice,
including outside Grafium, must be disclosed; Grafium also checks previous
opened assessments in this graph, even across different pack IDs.
An ineligible or unfinished attempt produces only a reasoning score.

**Pack-supplied norms are unverified.** Grafium validates their format and
matches the conditions, but cannot authenticate a publication, establish
representativeness, detect undisclosed prior practice or enforce unaided
testing. The displayed estimate is not a diagnosis, certified full-scale IQ,
or evidence that training raised general intelligence. No confidence interval
is invented without a validated measurement-error model.

## Pack format

An assessment is UTF-8 JSON (maximum 32 MiB) with `schemaVersion: 1`,
`kind: "iq"`, a stable `id`, `title`, `testVersion`, `source`, `license`,
an ordered `items` array, and `norms` (null for reasoning-only packs).
Each item has a unique `id`, a plain-text `question`, an `answers` array of
accepted strings, and an `explanation`. Supply 1–10,000 unique questions.
Do not include restricted test material without reuse permission.

Minimal reasoning-only example:

```json
{
  "schemaVersion": 1,
  "kind": "iq",
  "id": "reasoning-example-v1",
  "title": "Reasoning example",
  "testVersion": "1",
  "source": "Original arithmetic exercise",
  "license": "CC0",
  "items": [
    {
      "id": "addition",
      "question": "What is 2 + 2?",
      "answers": ["4", "four"],
      "explanation": "Adding two pairs makes four."
    }
  ],
  "norms": null
}
```

A non-null `norms` object requires:

- `citation`: the HTTP(S) URL of the published norm source.
- `population`, `sampleSize`, `ageMin`, `ageMax`, `language`: documented
  reference population and eligibility.
- `testVersion`: exactly the pack's version.
- `itemSetHash`: lower-case SHA-256 of compact UTF-8 JSON containing the
  ordered array of `[id, question, answers]` arrays. Preserve exact strings,
  ordered accepted answers and unescaped non-ASCII characters. Explanations
  are excluded. This detects item changes; it does not authenticate norms.
- `timeLimitSeconds`: the published whole-test limit, or null for untimed.
- `instructions`: published administration conditions.
- `zScores`: the published standardized z-score for every integer raw score,
  from zero through total items, in ascending raw-score order. The array
  length is item count + 1; values must be finite, monotonic, and within
  -10 to 10. Use the published transformation; do not assume raw scores
  follow a normal distribution or invent norms from this exercise bank.

Malformed or mismatched norms reject the import rather than silently
producing an estimate. Reimporting an identical ID/content is idempotent.
A changed pack needs a new ID; existing questions and attempts are never
overwritten. Unknown fields are rejected to catch misspelled metadata.

See [[Help - Flashcards]] for ordinary recall review and spaced repetition.
