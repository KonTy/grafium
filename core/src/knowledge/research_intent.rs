//! Detecting a "research this on the internet" intent in a Chat question.
//!
//! Chat normally answers only from the local knowledge graph. This module
//! recognises when the user has explicitly asked Chat to *also* go and look
//! something up on the open web ("does creatine cause cancer — research on the
//! internet") and, when it does, hands back the question with the trigger
//! phrase stripped off so neither the graph retrieval nor the web search
//! queries are polluted by the words "research on the internet".
//!
//! Why this lives in `core` (and not the command layer): triggering a live
//! internet research pass is a genuine product decision with real
//! consequences (it makes network requests, spends model tokens, and takes
//! seconds), so the rule for *when* it fires must be one testable thing rather
//! than an ad-hoc string check smeared across the UI. It is deliberately
//! **conservative about false positives**: a note that merely mentions the
//! web ("I was researching my family history", "my internet search history")
//! must never silently kick off a web crawl. Two design choices enforce that:
//!
//!   1. **Anchoring.** A trigger only counts when it sits at the very START or
//!      the very END of the question — i.e. as an imperative wrapped around
//!      the real question — never buried mid-sentence where it is far more
//!      likely to be part of what the user is actually asking about ("how do I
//!      search the web for papers?").
//!   2. **A real boundary.** A trailing trigger must be set off from the
//!      question by punctuation (`—`, `,`, `?`, …) or a filler word
//!      ("please", "and", "can you"), never merely a space. That single rule
//!      is what separates "does creatine cause cancer, search the web" (a
//!      command) from "how do I search the web" (a question about searching).
//!
//! Matching is intentionally plain string work (no regex) so every rule is
//! visible and unit-testable, and case-insensitive via ASCII lowercasing — a
//! byte-preserving transform, so an offset found in the lowercased copy slices
//! the original-cased string at exactly the same place, letting the cleaned
//! question keep the user's original capitalisation.

/// A recognised research request: the user's question with the research
/// trigger phrase removed, ready to feed to both the graph retrieval and the
/// web search planner without the trigger words skewing either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchIntent {
    /// The question with the trigger phrase (and any dangling separators the
    /// trigger left behind) stripped, e.g. `"does creatine cause cancer"` from
    /// `"does creatine cause cancer — research on the internet"`.
    ///
    /// May be empty or a bare referent when the user's message was *only* an
    /// instruction ("look it up on the internet") — see
    /// [`Self::needs_conversation_context`].
    pub cleaned_question: String,
    /// The request carries no topic of its own, so the caller must resolve it
    /// against the conversation before searching.
    ///
    /// "look it up on the internet" is unambiguously a research request, but
    /// what to research lives in the previous turn. Refusing outright (the
    /// earlier behaviour) made the feature look broken for exactly the
    /// phrasing people reach for mid-conversation.
    pub needs_conversation_context: bool,
}

/// Self-contained trigger phrases: each reads as a complete "go look this up
/// online" imperative on its own, so it can be matched whole at the start or
/// end of a question. Kept explicit (rather than generated from a grammar) so
/// the exact supported surface is greppable and testable. Every phrase already
/// names a web target ("online", "on the internet", "the web", …) — bare verbs
/// like "search" or "research" are intentionally absent, because they don't by
/// themselves mean "leave the graph and hit the internet".
const TRIGGER_PHRASES: &[&str] = &[
    // research + web
    "research on the internet",
    "research on the web",
    "research on the net",
    "research this on the internet",
    "research this on the web",
    "research this on the net",
    "research it on the internet",
    "research it on the web",
    "research that on the internet",
    "research that on the web",
    "research online",
    "research this online",
    "research it online",
    "research that online",
    // do (some) research
    "do some research online",
    "do some research on the internet",
    "do some research on the web",
    "do research online",
    "do research on the internet",
    "do research on the web",
    "do a bit of research online",
    "do more research online",
    // search
    "search the web",
    "search the internet",
    "search the net",
    "search online",
    "web search",
    // look up
    "look this up online",
    "look it up online",
    "look that up online",
    "look up online",
    "look this up on the internet",
    "look it up on the internet",
    "look that up on the internet",
    "look up on the internet",
    "look this up on the web",
    "look it up on the web",
    "look up on the web",
    // check
    "check the web",
    "check the internet",
    "check online",
    // find out
    "find out online",
    "find out on the internet",
    "find out on the web",
    // google (the verb)
    "google this",
    "google it",
    "google that",
];

/// Verbs that, in the "verb … web-target" wrap-around shape ("research
/// creatine and cancer online", "search for X on the web"), signal a research
/// request. Curated to research-y verbs so ordinary sentences that happen to
/// end in "online" ("watch the video online", "is this available online")
/// don't trip it. Bare "search"/"look" are excluded on purpose — "search my
/// emails online" shouldn't leave the graph.
/// Verbs that, when pointed at a web target, form a "go and look this up"
/// imperative: "look on the internet what is X", "browse the web for X".
///
/// Kept separate from [`WRAP_VERBS`] because these lead the sentence rather
/// than wrapping it, and the set is broader — a leading imperative is far less
/// ambiguous than a trailing one, so it can afford verbs like "look" and
/// "find" that would be unsafe at the end of a sentence.
const IMPERATIVE_VERBS: &[&str] = &[
    "look up",
    "look",
    "search for",
    "search",
    "find out",
    "find",
    "check",
    "browse",
    "research",
    "google",
    "read up",
    "read",
    "learn",
    "use",
    "query",
    "consult",
    "scan",
];

/// Filler that may sit between the verb and the web target: "look **up on**
/// the internet", "search **for** the web".
const IMPERATIVE_CONNECTORS: &[&str] = &[
    "up", "for", "at", "on", "in", "into", "through", "over", "across", "around", "to", "the",
];

/// What may follow the web target and still leave a real question behind:
/// either a connector ("browse the web **for** X") or a question opener
/// ("look on the internet **what** is X").
///
/// Requiring one of these is what keeps the rule from firing on ordinary
/// sentences that merely contain a verb next to a web word — "look at the
/// internet history page" leaves "history page", which is neither, so it
/// stays a normal question.
const POST_TARGET_CONNECTORS: &[&str] = &[
    "for",
    "about",
    "and",
    "to",
    "regarding",
    "concerning",
    "on",
    "if",
    "whether",
];

/// Bare web nouns, usable as the object of an imperative verb ("check **the
/// internet**", "browse **the web**"). [`WEB_TARGETS`] holds the
/// prepositional forms instead ("on the internet"), which is what a trailing
/// or wrap-around match needs.
const WEB_NOUNS: &[&str] = &[
    "the internet",
    "the web",
    "the net",
    "internet",
    "web",
    "online",
];

const WRAP_VERBS: &[&str] = &[
    "find out about",
    "read up on",
    "read about",
    "learn about",
    "search for",
    "google for",
    "find out",
    "look up",
    "research",
    "browse",
    "google",
    "check",
    "find",
];

/// Web-target tails for the wrap-around shape. Only forms that explicitly say
/// "on the internet"/"online" qualify — a plain "the internet" tail
/// ("learn about the internet") is a legitimate topic, not a command.
const WEB_TARGETS: &[&str] = &[
    "over the internet",
    "over the web",
    "across the internet",
    "on the internet",
    "in the internet",
    "on the web",
    "in the web",
    "on the net",
    "online",
];

/// Prepositional web references that can be lifted out of the middle of a
/// sentence, leaving a coherent question behind.
///
/// Deliberately excludes the bare "online", which reads as part of the
/// question far more often ("is this article available online") than as an
/// instruction when it appears mid-sentence.
const EMBEDDED_WEB_TARGETS: &[&str] = &[
    "over the internet",
    "over the web",
    "across the internet",
    "on the internet",
    "in the internet",
    "on the web",
    "in the web",
    "on the net",
];

/// Objects that mean the verb is aimed at the user's own graph, not the open
/// web — "check my notes on the internet of things" is a question about
/// notes, however much it looks like a web request.
const LOCAL_OBJECTS: &[&str] = &[
    "my note",
    "my journal",
    "my graph",
    "my page",
    "the notes",
    "my entries",
    "my writing",
];

/// Openers that mark a question *about* how searching works, rather than a
/// request to go and search.
const MECHANISM_OPENERS: &[&str] = &[
    "how do",
    "how does",
    "how did",
    "how can",
    "how would",
    "how should",
    "explain how",
    "what is the best way",
    "whats the best way",
    "why do",
    "why does",
];

/// Openers that mark the remainder as a genuine question/command, which is
/// what lets a *leading* trigger be separated from its question by only a
/// space ("search the web whether coffee is bad"). Without one of these — and
/// without punctuation or a connector preposition — a leading trigger followed
/// by a bare word ("search the web tips") is treated as the user's own phrase,
/// not a command, and does not fire.
const QUESTION_OPENERS: &[&str] = &[
    "whether",
    "if",
    "is",
    "are",
    "was",
    "were",
    "am",
    "do",
    "does",
    "did",
    "can",
    "could",
    "should",
    "would",
    "will",
    "has",
    "have",
    "had",
    "who",
    "what",
    "whats",
    "when",
    "where",
    "why",
    "how",
    "which",
    "whom",
    "whose",
    "list",
    "explain",
    "tell",
    "give",
    "show",
    "find",
    "name",
    "summarize",
    "summarise",
    "compare",
    "describe",
    "define",
];

/// Prepositions that dangle after a leading trigger ("search the web **for**
/// X", "research online **about** X") and should be dropped from the cleaned
/// question. Question words like "whether"/"if" are deliberately not here —
/// those belong to the question and are kept.
const LEADING_PREPOSITIONS: &[&str] = &[
    "for",
    "about",
    "regarding",
    "concerning",
    "around",
    "on",
    "onto",
    "re",
    "into",
];

/// Filler that can sit between the question and a *trailing* trigger and still
/// mark it as an appended command ("summarize my day **and** search the web",
/// "… **please** look it up online"). Presence of one of these (or of
/// punctuation) is what authorises a trailing match; a bare space never does.
/// Ordered longest-first so multi-word fillers are stripped before their
/// single-word prefixes.
const TRAILING_FILLERS: &[&str] = &[
    "can you",
    "could you",
    "would you",
    "will you",
    "please",
    "kindly",
    "pls",
    "plz",
    "then",
    "also",
    "and",
    "now",
    "so",
    "too",
];

/// Optional polite lead-ins that may precede a *leading* trigger ("can you
/// search the web for X", "please google this: Y"). Consumed only when a
/// trigger actually follows, so they never alter a question that merely starts
/// this way ("can you drink coffee — research online" keeps "can you drink
/// coffee" via the trailing path). Ordered longest-first.
const POLITE_PREFIXES: &[&str] = &[
    "could you please",
    "please can you",
    "can you please",
    "can you",
    "could you",
    "would you",
    "will you",
    "please",
    "kindly",
    "hey",
    "okay",
    "pls",
    "plz",
    "ok",
];

/// Characters that count as a separator between the question and a trigger, or
/// as dangling punctuation to trim. Note `?`, `.` and `!` are treated as
/// separators when they sit *between* the question and a trailing trigger
/// (they clearly end the question), but a trailing `?`/`.`/`!` on the cleaned
/// question itself is preserved by [`clean_fragment`].
const SEPARATOR_CHARS: &[char] = &[',', ':', ';', '-', '\u{2013}', '\u{2014}', '?', '.', '!'];

/// Phrases by which the user explicitly asks Chat to answer from the model's
/// own knowledge rather than from their notes.
///
/// This exists because the answering regime is otherwise chosen purely from
/// retrieval scores: if *any* note matched, Chat is told to answer from the
/// notes and to say so plainly when they don't cover the question. That is
/// right for "when did I paint my room", but it makes Chat refuse a direct
/// request — asked "but based on your knowledge that you don't have in my
/// notes", it answered "I do not have knowledge outside of the notes
/// provided", which is both unhelpful and untrue of the model.
///
/// Unlike a research trigger, these need no anchoring: they are not
/// imperatives wrapped around a question but statements about how to answer,
/// and they read naturally mid-sentence ("what do *you* know about X, not
/// from my notes"). The phrases are specific enough that a bare mention is
/// unlikely — note the deliberate absence of a plain "knowledge" or "you
/// know", which would fire on ordinary questions.
const GENERAL_KNOWLEDGE_PHRASES: &[&str] = &[
    "based on your knowledge",
    "based on your own knowledge",
    "from your knowledge",
    "from your own knowledge",
    "using your knowledge",
    "using your own knowledge",
    "your general knowledge",
    "general knowledge",
    "not on notes",
    "not on my notes",
    "not in my notes",
    "not in the notes",
    "not from my notes",
    "not from the notes",
    "outside my notes",
    "outside of my notes",
    "without my notes",
    "without using my notes",
    "without the notes",
    "ignore my notes",
    "ignore the notes",
    "don't use my notes",
    "dont use my notes",
    "do not use my notes",
    "what do you know about",
];

/// Whether the user explicitly asked to be answered from the model's own
/// knowledge instead of (or in addition to) their notes.
///
/// When true, the caller forces the general-knowledge answering regime even
/// though retrieval may have returned hits — an explicit instruction from the
/// user should outrank a similarity score.
pub fn wants_general_knowledge(question: &str) -> bool {
    let norm = normalize_ws(question);
    let low: String = norm.chars().map(|c| c.to_ascii_lowercase()).collect();
    // Apostrophes vary by keyboard/autocorrect; normalise so "don't" and
    // "don\u{2019}t" both match the same phrase.
    let low = low.replace('\u{2019}', "'");
    GENERAL_KNOWLEDGE_PHRASES
        .iter()
        .any(|phrase| low.contains(phrase))
}

/// Detect whether `question` is asking Chat to research on the web, returning
/// the question with the trigger phrase stripped when so. Returns `None` for
/// ordinary questions (the common case) — including ones that merely *mention*
/// the web without commanding a search.
pub fn detect_research_intent(question: &str) -> Option<ResearchIntent> {
    let norm = normalize_ws(question);
    if norm.is_empty() {
        return None;
    }
    // ASCII lowercasing preserves byte layout, so offsets found in `low` slice
    // `norm` at the identical position while keeping original capitalisation.
    let low: String = norm.chars().map(|c| c.to_ascii_lowercase()).collect();

    let cleaned = match_leading(&low, &norm)
        .or_else(|| match_leading_imperative(&low, &norm))
        .or_else(|| match_trailing(&low, &norm))
        .or_else(|| match_wraparound(&low, &norm))
        .or_else(|| match_embedded(&low, &norm))?;

    let cleaned = clean_fragment(&cleaned);
    // A residue only counts as a topic if something survives stripping
    // referents and filler. Checking for bare pronouns alone was not enough:
    // "look on the internet" leaves the verb "look", which is not a pronoun,
    // so it was accepted and the web was searched for the word "look" —
    // returning a dictionary definition instead of the subject under
    // discussion.
    let needs_context = !crate::knowledge::conversation::is_self_contained(&cleaned);
    Some(ResearchIntent {
        cleaned_question: cleaned,
        needs_conversation_context: needs_context,
    })
}

/// Whether the cleaned question is nothing but a back-reference with no
/// subject of its own ("this", "that", "it").
///
/// "can you research this on the internet" cleans down to the single word
/// `"this"`, which must never reach the search planner as a literal query.
/// Detection deliberately still fires for these — the request *is* a research
/// request — and it's the caller's job to resolve the referent against the
/// conversation (see [`crate::knowledge::conversation::resolve_followup`]) and
/// to decline only when there's nothing to resolve it to.
pub fn is_contentless_referent(cleaned: &str) -> bool {
    const REFERENTS: &[&str] = &["this", "that", "it", "these", "those", "them", "the same"];
    let low: String = cleaned
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    REFERENTS.contains(&low.as_str())
}

/// Match a web reference sitting in the *middle* of a request, lifting it out
/// and leaving the rest as the question.
///
/// The anchored rules (leading/trailing/wrap-around) all assume the
/// instruction brackets the question, which repeatedly failed on the way
/// people actually write: "can you find the latest paper he published on the
/// internet and summarize it" puts the web reference in the middle, with real
/// question on both sides. Requiring an anchor meant no research happened and
/// Chat answered from notes it didn't have.
///
/// Conservative because it's the loosest rule here: it needs a genuine search
/// verb *before* the web reference, refuses questions about how searching
/// works, and refuses verbs aimed at the user's own notes.
fn match_embedded(low: &str, norm: &str) -> Option<String> {
    if MECHANISM_OPENERS
        .iter()
        .any(|opener| low.starts_with(opener))
    {
        return None;
    }

    for target in EMBEDDED_WEB_TARGETS {
        let Some(at) = low.find(target) else {
            continue;
        };
        if !word_boundary_at(low, at + target.len()) {
            continue;
        }
        let before = &low[..at];
        // A search verb must introduce the reference, otherwise this is just a
        // sentence that mentions the internet.
        if !IMPERATIVE_VERBS
            .iter()
            .any(|verb| contains_word(before, verb))
        {
            continue;
        }
        if LOCAL_OBJECTS.iter().any(|obj| before.contains(obj)) {
            continue;
        }

        // Splice the sentence back together without the web reference.
        let head = norm[..at].trim_end();
        let tail = norm[at + target.len()..].trim_start();
        let joined = match (head.is_empty(), tail.is_empty()) {
            (true, true) => String::new(),
            (true, false) => tail.to_string(),
            (false, true) => head.to_string(),
            (false, false) => format!("{head} {tail}"),
        };
        let joined = clean_fragment(&joined);
        if joined.is_empty() {
            continue;
        }
        return Some(joined);
    }
    None
}

/// Whether `haystack` contains `needle` as a whole word.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let at = from + rel;
        let before_ok = at == 0
            || !haystack[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
        if before_ok && word_boundary_at(haystack, at + needle.len()) {
            return true;
        }
        from = at + needle.len();
    }
    false
}

/// Match a leading "go look this up" imperative built compositionally from a
/// verb and a web target, rather than from an enumerated phrase.
///
/// The original design listed trigger phrases literally, which turned out to
/// be unusable in practice: "look on the internet what is scientology?" — a
/// completely ordinary way to ask — matched nothing, because only "look this
/// up on the internet" had been enumerated. Chasing that with more literals is
/// endless, so this recognises the *shape* instead:
///
/// ```text
/// [polite] <verb> [connector…] <web-noun> [post-connector | question-opener] <question>
/// ```
///
/// The requirement that a connector or question opener follow the web noun is
/// what keeps it conservative: "look at the internet history page" leaves
/// "history page", which is neither, so it doesn't fire.
fn match_leading_imperative(low: &str, norm: &str) -> Option<String> {
    for &offset in &leading_start_offsets(low) {
        let low_rest = &low[offset..];
        let Some(verb) = longest_prefix_match(low_rest, IMPERATIVE_VERBS) else {
            continue;
        };
        let mut cursor = verb.len();

        // Consume any connectors sitting between the verb and the web noun,
        // bounded so a long run of prepositions can't wander into the question.
        for _ in 0..3 {
            let rest = low_rest[cursor..].trim_start();
            let consumed = low_rest.len() - rest.len() - cursor;
            match longest_prefix_match(rest, IMPERATIVE_CONNECTORS) {
                // Only skip a connector if a web noun still follows it,
                // otherwise "look for scientology" would eat "for".
                Some(conn) if word_boundary_at(rest, conn.len()) => {
                    let after = rest[conn.len()..].trim_start();
                    if longest_prefix_match(after, WEB_NOUNS).is_some()
                        || longest_prefix_match(after, IMPERATIVE_CONNECTORS).is_some()
                    {
                        cursor += consumed + conn.len();
                        continue;
                    }
                    break;
                }
                _ => break,
            }
        }

        let rest = low_rest[cursor..].trim_start();
        let Some(noun) = longest_prefix_match(rest, WEB_NOUNS) else {
            continue;
        };
        if !word_boundary_at(rest, noun.len()) {
            continue;
        }
        let after_noun = rest[noun.len()..].trim_start();
        if after_noun.is_empty() {
            continue;
        }

        // The question must be introduced by a connector or read as a question
        // in its own right; anything else is an ordinary sentence.
        let question_start = match longest_prefix_match(after_noun, POST_TARGET_CONNECTORS) {
            Some(conn) if word_boundary_at(after_noun, conn.len()) => {
                after_noun[conn.len()..].trim_start()
            }
            _ if starts_with_question_opener(after_noun) => after_noun,
            _ => continue,
        };
        if question_start.is_empty() {
            continue;
        }

        // Map the offset back onto the original-cased string. ASCII
        // lowercasing is byte-preserving, so the arithmetic is exact.
        let consumed = low.len() - question_start.len();
        return Some(norm[consumed..].to_string());
    }
    None
}

/// Longest phrase in `options` that prefixes `s`, so "look up" wins over
/// "look" and "the internet" over "internet".
fn longest_prefix_match<'a>(s: &str, options: &[&'a str]) -> Option<&'a str> {
    options
        .iter()
        .filter(|opt| s.starts_with(*opt))
        .max_by_key(|opt| opt.len())
        .copied()
}

/// Whether position `at` in `s` ends a whole word, so "web" doesn't match
/// inside "webinar".
fn word_boundary_at(s: &str, at: usize) -> bool {
    match s[at..].chars().next() {
        None => true,
        Some(c) => !c.is_alphanumeric(),
    }
}

/// Match a trigger at the very start: `[polite] <trigger> <sep/connector>
/// <question>`. Returns the (uncleaned) question tail.
fn match_leading(low: &str, norm: &str) -> Option<String> {
    for &offset in &leading_start_offsets(low) {
        let low_rest = &low[offset..];
        for phrase in sorted_phrases() {
            if !low_rest.starts_with(phrase) {
                continue;
            }
            let after = offset + phrase.len();
            // The character right after the phrase must be a boundary, so a
            // trigger can't be the prefix of a longer word ("google" must not
            // fire on "googley").
            match low[after..].chars().next() {
                // The whole message is the instruction ("look it up on the
                // internet"). That's still a research request; the topic comes
                // from the conversation, which the caller resolves.
                None => return Some(String::new()),
                Some(c) if c.is_alphanumeric() => continue,
                _ => {}
            }
            if let Some(q) = strip_leading_boundary(&norm[after..]) {
                return Some(q);
            }
            // Trailing punctuation only ("search the web?") — same case.
            if norm[after..]
                .trim()
                .chars()
                .all(|c| SEPARATOR_CHARS.contains(&c))
            {
                return Some(String::new());
            }
        }
    }
    None
}

/// Byte offsets in `low` at which a leading trigger may begin: 0, plus just
/// past any polite lead-in ("can you ", "please ", …).
fn leading_start_offsets(low: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for polite in POLITE_PREFIXES {
        if let Some(rest) = low.strip_prefix(polite) {
            if rest.starts_with(' ') {
                offsets.push(polite.len() + 1);
                break;
            }
        }
    }
    offsets
}

/// Given the text right after a leading trigger, validate the boundary and
/// return the question with dangling separators/prepositions removed, or
/// `None` if the boundary is too weak to treat as a command.
fn strip_leading_boundary(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let mut saw_signal = false;

    // A run of separator punctuation (":", "—", ",", …).
    let after_punct = trimmed.trim_start_matches(|c| SEPARATOR_CHARS.contains(&c) || c == ' ');
    if after_punct.len() != trimmed.len() {
        saw_signal = true;
    }
    let after_punct = after_punct.trim_start();

    // A single dangling connector preposition ("for"/"about"/…) is dropped.
    let (candidate, stripped_prep) = strip_leading_word(after_punct, LEADING_PREPOSITIONS);
    if stripped_prep {
        saw_signal = true;
    }
    let candidate = candidate.trim_start();

    if candidate.is_empty() {
        return None;
    }

    // With no punctuation/preposition boundary, only accept when the remainder
    // reads as a real question/command; otherwise the "trigger" is just the
    // start of the user's own phrase ("search the web tips for beginners").
    if !saw_signal && !starts_with_question_opener(candidate) {
        return None;
    }
    Some(candidate.to_string())
}

/// Match a trigger at the very end: `<question> <sep-or-filler> <trigger>`.
/// The separator must be punctuation or a filler word — never a bare space —
/// so a question *about* searching ("how do I search the web") is left alone.
fn match_trailing(low: &str, norm: &str) -> Option<String> {
    // Ignore trailing sentence punctuation on the whole input ("… google it?").
    let low_t = low.trim_end_matches([' ', '?', '.', '!']);
    for phrase in sorted_phrases() {
        if !low_t.ends_with(phrase) {
            continue;
        }
        let start = low_t.len() - phrase.len();
        // Char just before the trigger must be a boundary (not part of a word).
        if start > 0 {
            let before = low[..start].chars().next_back();
            if matches!(before, Some(c) if c.is_alphanumeric()) {
                continue;
            }
        } else {
            // Whole input is just the trigger — no question to research.
            continue;
        }
        if let Some(q) = strip_trailing_boundary(&norm[..start]) {
            return Some(q);
        }
    }
    None
}

/// Validate the boundary before a trailing trigger and return the question.
/// Requires evidence of a real separator (punctuation or filler); a bare space
/// alone is rejected.
/// Words that can't meaningfully end a standalone question, used to tell a
/// real question from a dangling fragment when deciding whether a trailing
/// trigger separated only by a space is a command.
///
/// "does creatine cause cancer" ends on a content word and is clearly a whole
/// question, so "… research on the internet" after it is an instruction.
/// "how do I" ends on a pronoun and is obviously mid-sentence, so "… search
/// the web" after it is part of what's being asked, not a command.
const DANGLING_TAIL_WORDS: &[&str] = &[
    "i", "you", "we", "they", "he", "she", "it", "to", "for", "of", "on", "in", "at", "by", "with",
    "from", "the", "a", "an", "and", "or", "but", "do", "does", "did", "can", "could", "should",
    "would", "will", "how", "what", "when", "where", "why", "who", "is", "are", "was", "were",
    "my", "your", "their", "about", "into", "over", "across", "use", "using",
];

/// Whether `prefix` reads as a complete question in its own right, which is
/// what licenses treating a space-separated trailing trigger as a command.
fn prefix_is_standalone_question(prefix: &str) -> bool {
    let words: Vec<&str> = prefix.split_whitespace().collect();
    // Two words can't carry a real question plus leave a trigger unambiguous.
    if words.len() < 3 {
        return false;
    }
    let last = words[words.len() - 1]
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    !last.is_empty() && !DANGLING_TAIL_WORDS.contains(&last.as_str())
}

fn strip_trailing_boundary(prefix: &str) -> Option<String> {
    let mut p = prefix.trim_end();
    if p.is_empty() {
        return None;
    }
    let mut saw_signal = false;
    loop {
        // Strip a trailing filler word/phrase ("… and", "… please").
        let (shorter, stripped) = strip_trailing_word(p, TRAILING_FILLERS);
        if stripped {
            p = shorter.trim_end();
            saw_signal = true;
            continue;
        }
        // Strip trailing separator punctuation ("… —", "… ,", "…?").
        if let Some(c) = p.chars().next_back() {
            if SEPARATOR_CHARS.contains(&c) {
                p = p[..p.len() - c.len_utf8()].trim_end();
                saw_signal = true;
                continue;
            }
        }
        break;
    }
    if p.is_empty() {
        return None;
    }
    // Requiring punctuation or a filler word here was too strict to be usable:
    // people type "does creatine cause cancer research on the internet" without
    // a comma, and the feature simply never fired. A bare space is accepted
    // when what precedes it is itself a complete question, which keeps the
    // case this guard exists for ("how do I search the web") excluded, since
    // its prefix is a dangling fragment rather than a question.
    if !saw_signal && !prefix_is_standalone_question(p) {
        return None;
    }
    Some(p.to_string())
}

/// Match the wrap-around shape `<verb> <question> <web-target>` ("research
/// creatine and cancer online"). A strong signal on its own (research verb +
/// explicit web tail), so it needs no extra boundary rule beyond both ends
/// being present with a non-empty middle.
fn match_wraparound(low: &str, norm: &str) -> Option<String> {
    let low_t = low.trim_end_matches([' ', '?', '.', '!']);
    for &offset in &leading_start_offsets(low) {
        // Recompute the trimmed end relative to the (possibly polite-skipped)
        // start so target matching lines up with `low_t`.
        if low_t.len() <= offset {
            continue;
        }
        let rest_t = &low_t[offset..];
        for verb in WRAP_VERBS {
            let Some(after_verb) = rest_t.strip_prefix(verb) else {
                continue;
            };
            if !after_verb.starts_with(' ') {
                continue; // verb glued to a longer word
            }
            for target in WEB_TARGETS {
                if !rest_t.ends_with(target) {
                    continue;
                }
                let target_start = rest_t.len() - target.len();
                // Need whitespace immediately before the web target.
                if !rest_t[..target_start].ends_with(' ') {
                    continue;
                }
                let q_start = offset + verb.len();
                let q_end = offset + target_start;
                if q_end <= q_start {
                    continue;
                }
                let middle = norm[q_start..q_end].trim();
                if !middle.is_empty() {
                    return Some(middle.to_string());
                }
            }
        }
    }
    None
}

// ── small string helpers ────────────────────────────────────────────────────

/// Collapse runs of ASCII/Unicode whitespace to single spaces and trim ends.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Phrases sorted longest-first, so a more specific phrase ("research this on
/// the internet") is preferred over a shorter one it contains.
fn sorted_phrases() -> Vec<&'static str> {
    let mut v = TRIGGER_PHRASES.to_vec();
    v.sort_by_key(|p| std::cmp::Reverse(p.len()));
    v
}

/// If `s` starts with one of `words` as a whole word (followed by a space or
/// end), return the remainder after it and `true`; otherwise `s` and `false`.
fn strip_leading_word<'a>(s: &'a str, words: &[&str]) -> (&'a str, bool) {
    let lower: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    for w in words {
        if let Some(rest) = lower.strip_prefix(w) {
            if rest.is_empty() || rest.starts_with(' ') {
                return (&s[w.len()..], true);
            }
        }
    }
    (s, false)
}

/// If `s` ends with one of `words` as a whole word (preceded by a space or
/// start), return the prefix before it and `true`; otherwise `s` and `false`.
fn strip_trailing_word<'a>(s: &'a str, words: &[&str]) -> (&'a str, bool) {
    let lower: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    for w in words {
        if let Some(prefix) = lower.strip_suffix(w) {
            if prefix.is_empty() || prefix.ends_with(' ') {
                return (&s[..s.len() - w.len()], true);
            }
        }
    }
    (s, false)
}

/// Whether `s`'s first word is one of the [`QUESTION_OPENERS`].
fn starts_with_question_opener(s: &str) -> bool {
    let first = s
        .split([' ', '\''])
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    QUESTION_OPENERS.contains(&first.as_str())
}

/// Trim dangling separators/whitespace a stripped trigger leaves behind, while
/// preserving a trailing `?`/`.`/`!` that belongs to the question itself.
fn clean_fragment(s: &str) -> String {
    let trimmed = s.trim();
    let trimmed = trimmed.trim_start_matches(|c: char| c.is_whitespace() || is_dangling_sep(c));
    let trimmed = trimmed.trim_end_matches(|c: char| c.is_whitespace() || is_dangling_sep(c));
    trimmed.to_string()
}

/// Separators that are safe to trim from either end of a cleaned question
/// (excludes `?`/`.`/`!`, which can legitimately end a question).
fn is_dangling_sep(c: char) -> bool {
    matches!(c, ',' | ':' | ';' | '-' | '\u{2013}' | '\u{2014}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleaned(q: &str) -> Option<String> {
        detect_research_intent(q).map(|i| i.cleaned_question)
    }

    // ── the motivating example ──────────────────────────────────────────────

    #[test]
    fn strips_trailing_research_trigger_with_em_dash() {
        assert_eq!(
            cleaned("does creatine cause cancer — research on the internet").as_deref(),
            Some("does creatine cause cancer")
        );
    }

    // ── positive: trailing placement, varied separators ─────────────────────

    #[test]
    fn trailing_variants_fire_and_clean() {
        let cases = [
            (
                "does creatine cause cancer, research this on the internet",
                "does creatine cause cancer",
            ),
            ("why do cats purr, search online", "why do cats purr"),
            ("who wrote 1984? search the web", "who wrote 1984"),
            ("are eggs healthy? look it up online", "are eggs healthy"),
            ("summarize my day and search the web", "summarize my day"),
            (
                "best mechanical keyboards: web search",
                "best mechanical keyboards",
            ),
            ("capital of peru — google it", "capital of peru"),
            (
                "is intermittent fasting effective, please look it up on the internet",
                "is intermittent fasting effective",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(cleaned(input).as_deref(), Some(want), "input: {input:?}");
        }
    }

    // ── positive: leading placement, varied connectors ──────────────────────

    #[test]
    fn leading_variants_fire_and_clean() {
        let cases = [
            (
                "research on the internet: does creatine cause cancer",
                "does creatine cause cancer",
            ),
            (
                "search the web for the best creatine studies",
                "the best creatine studies",
            ),
            (
                "look it up on the internet — are eggs healthy",
                "are eggs healthy",
            ),
            ("google this: capital of peru", "capital of peru"),
            (
                "search online whether coffee is bad for you",
                "whether coffee is bad for you",
            ),
            (
                "research online about quantum computing advances",
                "quantum computing advances",
            ),
            (
                "do some research online on how vaccines work",
                "how vaccines work",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(cleaned(input).as_deref(), Some(want), "input: {input:?}");
        }
    }

    // ── positive: polite lead-ins ───────────────────────────────────────────

    #[test]
    fn polite_lead_ins_are_consumed_before_a_leading_trigger() {
        assert_eq!(
            cleaned("can you search the web for creatine and cancer").as_deref(),
            Some("creatine and cancer")
        );
        assert_eq!(
            cleaned("please google this: who painted guernica").as_deref(),
            Some("who painted guernica")
        );
        assert_eq!(
            cleaned("could you please research online how tariffs work").as_deref(),
            Some("how tariffs work")
        );
    }

    // ── positive: wrap-around "verb … online" ───────────────────────────────

    #[test]
    fn wraparound_verb_then_web_target_fires() {
        let cases = [
            ("research creatine and cancer online", "creatine and cancer"),
            (
                "search for the best budget laptops on the web",
                "the best budget laptops",
            ),
            (
                "look up whether creatine causes cancer online",
                "whether creatine causes cancer",
            ),
            (
                "find out about the mediterranean diet on the internet",
                "the mediterranean diet",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(cleaned(input).as_deref(), Some(want), "input: {input:?}");
        }
    }

    // ── casing / punctuation robustness ─────────────────────────────────────

    #[test]
    fn matching_is_case_insensitive_but_preserves_question_case() {
        assert_eq!(
            cleaned("Does Creatine Cause Cancer — RESEARCH ON THE INTERNET").as_deref(),
            Some("Does Creatine Cause Cancer")
        );
        assert_eq!(
            cleaned("SEARCH THE WEB for Rust ownership").as_deref(),
            Some("Rust ownership")
        );
    }

    #[test]
    fn tolerates_extra_whitespace_and_trailing_punctuation() {
        assert_eq!(
            cleaned("  does   creatine cause cancer   ,   search the web  ").as_deref(),
            Some("does creatine cause cancer")
        );
        assert_eq!(
            cleaned("what is a monad — research on the internet??").as_deref(),
            Some("what is a monad")
        );
    }

    // ── negative: must NOT fire ──────────────────────────────────────────────

    #[test]
    fn does_not_fire_on_incidental_mentions() {
        let negatives = [
            "I was researching my family history",
            "my internet search history is private",
            "how do I search the web for academic papers?",
            "what is the best way to search the internet",
            "explain how web search engines rank pages",
            "is this article available online",
            "the meeting is online tomorrow",
            "teach me to research effectively",
            "what does google do with my data",
            "download the dataset online",
            "watch the lecture online",
            "learn about the internet's history",
        ];
        for input in negatives {
            assert_eq!(cleaned(input), None, "should not fire: {input:?}");
        }
    }

    #[test]
    fn bare_trigger_fires_but_needs_conversation_context() {
        // Mid-conversation these are perfectly normal requests — the topic is
        // in the previous turn, so they must fire and be flagged as needing
        // context rather than be rejected outright.
        for input in ["search the web", "research on the internet", "google it"] {
            let intent =
                detect_research_intent(input).unwrap_or_else(|| panic!("should fire: {input:?}"));
            assert!(
                intent.needs_conversation_context,
                "{input:?} carries no topic of its own"
            );
        }
    }

    #[test]
    fn trailing_trigger_after_a_dangling_fragment_does_not_fire() {
        // "how do I" is a dangling fragment, so the trailing words are part
        // of what's being asked, not a command…
        assert_eq!(cleaned("how do I search the web"), None);
        assert_eq!(cleaned("how do I search the web for academic papers"), None);
        // …but the same words with a separating comma are a command.
        assert_eq!(
            cleaned("how do I bake bread, search the web").as_deref(),
            Some("how do I bake bread")
        );
    }

    /// Requiring punctuation before a trailing trigger made the feature
    /// effectively unreachable — people type the request as one flat sentence.
    /// A bare space is enough when what precedes it is a whole question.
    #[test]
    fn trailing_trigger_fires_after_a_complete_question_without_punctuation() {
        assert_eq!(
            cleaned("does creatine cause cancer research on the internet").as_deref(),
            Some("does creatine cause cancer")
        );
        assert_eq!(
            cleaned("does creatine cause cancer search the web").as_deref(),
            Some("does creatine cause cancer")
        );
        assert_eq!(
            cleaned("what is the best protein powder search online").as_deref(),
            Some("what is the best protein powder")
        );
    }

    /// The literal-phrase design missed the most natural way to ask, which is
    /// why the feature appeared not to work at all: "look on the internet what
    /// is scientology?" enumerated nothing. These are all composed from a verb
    /// plus a web target rather than listed individually.
    #[test]
    fn leading_imperatives_fire_compositionally() {
        for (input, want) in [
            (
                "look on the internet what is scientology?",
                "what is scientology?",
            ),
            ("look on the web what is scientology", "what is scientology"),
            (
                "look up on the internet what is scientology",
                "what is scientology",
            ),
            ("check the internet for scientology", "scientology"),
            ("look online for scientology", "scientology"),
            ("browse the web for scientology", "scientology"),
            (
                "use the internet to tell me about scientology",
                "tell me about scientology",
            ),
            ("find scientology on the internet", "scientology"),
        ] {
            assert_eq!(
                cleaned(input).as_deref(),
                Some(want),
                "should fire for {input:?}"
            );
        }
    }

    /// A verb next to a web word is not automatically a command. Each of
    /// these leaves something that is neither a connector nor a question, or
    /// aims the verb at the user's own notes.
    #[test]
    fn verb_near_a_web_word_alone_does_not_fire() {
        for input in [
            "look at the internet history page",
            "what are my notes about the internet",
            "find my notes about scientology",
            "search my journal for scientology",
            "when did I last use the web browser",
            "check my notes on the internet of things",
        ] {
            assert_eq!(cleaned(input), None, "should not fire for {input:?}");
        }
    }

    /// The anchored rules all assumed the instruction brackets the question,
    /// which failed on the most natural way to write a research request: the
    /// web reference lands in the middle, with real question on both sides.
    #[test]
    fn a_web_reference_in_mid_sentence_fires_and_is_lifted_out() {
        for (input, want) in [
            (
                "can you find the latest paper he published on the internet and summarize it",
                "can you find the latest paper he published and summarize it",
            ),
            (
                "look up his latest paper on the web and explain it simply",
                "look up his latest paper and explain it simply",
            ),
        ] {
            assert_eq!(cleaned(input).as_deref(), Some(want), "for {input:?}");
        }
        // "in the internet" is not idiomatic English but is extremely common,
        // and refusing it just makes the feature look broken.
        assert!(cleaned("find the latest paper in the internet").is_some());
    }

    /// The embedded rule is the loosest one here, so its guards matter most:
    /// questions about how searching works, and verbs aimed at the user's own
    /// notes, must still be left alone.
    #[test]
    fn the_embedded_rule_keeps_its_guards() {
        for input in [
            "how do I search the web for academic papers",
            "explain how web search engines rank pages",
            "check my notes on the internet of things",
            "summarize my notes on the internet of things",
            "what did I write about the internet",
            "when did I last look at the web archive",
            "my internet search history is private",
        ] {
            assert_eq!(cleaned(input), None, "should not fire for {input:?}");
        }
    }

    /// A back-referencing request is still a research request — "can you
    /// research this on the internet" must fire — but the residue is a bare
    /// referent that the caller has to resolve against the conversation before
    /// it can be searched for.
    #[test]
    fn a_bare_back_reference_fires_but_is_flagged_contentless() {
        for input in [
            "can you research this on the internet",
            "research it on the web",
            "look it up on the internet",
        ] {
            let intent = detect_research_intent(input)
                .unwrap_or_else(|| panic!("should fire for {input:?}"));
            assert!(
                intent.needs_conversation_context,
                "{input:?} cleaned to {:?}, which should need conversation context",
                intent.cleaned_question
            );
        }
    }

    #[test]
    fn leading_trigger_without_boundary_or_question_word_does_not_fire() {
        // "search the web tips for beginners" is the user's own phrase, not a
        // command to search for "tips for beginners".
        assert_eq!(cleaned("search the web tips for beginners"), None);
    }

    #[test]
    fn google_does_not_match_inside_a_longer_word() {
        assert_eq!(cleaned("what is a googol number"), None);
        assert_eq!(
            cleaned("tell me about googley eyes, and check online").as_deref(),
            Some("tell me about googley eyes")
        );
    }

    #[test]
    fn empty_or_whitespace_is_none() {
        assert_eq!(cleaned(""), None);
        assert_eq!(cleaned("    "), None);
    }

    #[test]
    fn an_empty_cleaned_question_is_always_flagged() {
        // The invariant the caller relies on: an empty residue is never handed
        // on as a search query without being marked as needing context first.
        for input in [
            "search the web",
            "research on the internet",
            "look it up on the internet",
            "google it",
            "— research on the internet",
            ": : : search the web",
            "does creatine cause cancer, search the web",
        ] {
            if let Some(intent) = detect_research_intent(input) {
                if intent.cleaned_question.trim().is_empty() {
                    assert!(
                        intent.needs_conversation_context,
                        "{input:?} produced an empty query without flagging it"
                    );
                }
            }
        }
    }

    #[test]
    fn keeps_internal_question_words_for_leading_whether_if() {
        assert_eq!(
            cleaned("search the web if creatine is safe").as_deref(),
            Some("if creatine is safe")
        );
    }

    #[test]
    fn returns_intent_struct_shape() {
        let intent = detect_research_intent("what is rust — search the web").unwrap();
        assert_eq!(intent.cleaned_question, "what is rust");
    }
}
