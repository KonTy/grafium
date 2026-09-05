//! Reasoning-model ("thinking") output handling.
//!
//! Some local chat models — Qwen3, DeepSeek-R1 and other "reasoning" GGUFs —
//! emit an internal chain-of-thought wrapped in `<think>…</think>` before the
//! actual answer. Left unstripped this leaks raw reasoning to the user and,
//! worse, a model can spend its *entire* output budget inside an unterminated
//! `<think>` and never produce an answer at all (observed with an 8B Qwen3
//! model: 400/400 tokens consumed reasoning, zero answer, on a plain "say
//! hello" prompt). This module converts that raw output into either a clean
//! answer or an explicit "the model only reasoned" signal, and provides a
//! streaming filter so the UI can show a distinct "Thinking…" state and then
//! real answer deltas without ever displaying the chain-of-thought.

const OPEN_TAG: &str = "<think>";
const CLOSE_TAG: &str = "</think>";

/// Shown to the user when a reasoning model exhausted its output budget
/// without ever emitting an answer outside its `<think>` block. Far more
/// useful than dumping raw chain-of-thought at them.
pub const REASONING_ONLY_MESSAGE: &str =
    "The model spent its entire output budget reasoning and never produced an answer. \
     Try a smaller or non-reasoning model, or raise the output token limit in Settings.";

/// Outcome of stripping `<think>` reasoning from a model's raw output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkStripResult {
    /// A usable answer, with all reasoning removed.
    Answer(String),
    /// The model produced only reasoning — an unterminated `<think>`, or a
    /// think block with nothing after it — so there is no answer to show.
    ReasoningOnly,
}

/// What the UI should do after a raw token piece is fed to [`ThinkStreamFilter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStep {
    /// New answer text to append in the UI.
    Answer(String),
    /// The model is currently inside a `<think>` block — show "Thinking…".
    Thinking,
    /// Nothing user-visible changed (e.g. a partial tag is being buffered).
    Idle,
}

struct Scan {
    /// Text outside any `<think>` block (the answer so far), untrimmed.
    answer: String,
    /// `<think>` nesting depth left open at the end (`> 0` == unterminated).
    depth: usize,
    /// Whether any `<think>` open tag was seen at all.
    saw_open: bool,
}

/// Single left-to-right pass splitting `raw` into answer text vs. `<think>`
/// reasoning while tracking nesting depth. Handles multiple and nested
/// blocks, and stray or unterminated tags, without panicking. Operates on
/// `&str` byte offsets found via `find`, so it is UTF-8 safe.
fn scan(raw: &str) -> Scan {
    let mut answer = String::new();
    let mut depth: usize = 0;
    let mut saw_open = false;
    let mut rest = raw;

    loop {
        let next_open = rest.find(OPEN_TAG);
        let next_close = rest.find(CLOSE_TAG);

        let open_first = match (next_open, next_close) {
            (Some(o), Some(c)) => o < c,
            (Some(_), None) => true,
            (None, _) => false,
        };

        if let Some(o) = next_open.filter(|_| open_first) {
            if depth == 0 {
                answer.push_str(&rest[..o]);
            }
            depth += 1;
            saw_open = true;
            rest = &rest[o + OPEN_TAG.len()..];
        } else if let Some(c) = next_close {
            if depth == 0 {
                // Stray closing tag with no matching open: keep the text
                // before it, drop the tag itself.
                answer.push_str(&rest[..c]);
            }
            depth = depth.saturating_sub(1);
            rest = &rest[c + CLOSE_TAG.len()..];
        } else {
            if depth == 0 {
                answer.push_str(rest);
            }
            break;
        }
    }

    Scan {
        answer,
        depth,
        saw_open,
    }
}

/// Strip `<think>…</think>` reasoning from a *completed* model response.
///
/// Returns [`ThinkStripResult::ReasoningOnly`] when the model never produced
/// answer text (unterminated `<think>`, or a balanced block with nothing
/// after it) so the caller can surface [`REASONING_ONLY_MESSAGE`] instead of
/// raw chain-of-thought. A normal answer with no reasoning is returned
/// unchanged (only outer whitespace trimmed).
pub fn strip_think_blocks(raw: &str) -> ThinkStripResult {
    let scanned = scan(raw);
    let trimmed = scanned.answer.trim();
    if trimmed.is_empty() && scanned.saw_open {
        ThinkStripResult::ReasoningOnly
    } else {
        ThinkStripResult::Answer(trimmed.to_string())
    }
}

/// Longest suffix of `s` that is a proper (incomplete) prefix of a
/// `<think>`/`</think>` tag — text that must be withheld from the UI because
/// the next token might complete it into a tag we need to hide. Tags are
/// ASCII, so a byte-level `ends_with` lands on a UTF-8 boundary.
fn dangling_tag_prefix_len(s: &str) -> usize {
    let mut best = 0;
    for tag in [OPEN_TAG, CLOSE_TAG] {
        let max_k = (tag.len() - 1).min(s.len());
        for k in 1..=max_k {
            if s.as_bytes().ends_with(tag[..k].as_bytes()) {
                best = best.max(k);
            }
        }
    }
    best
}

/// Streaming `<think>` filter: feed it raw model token pieces in order and it
/// forwards only answer text to the UI, reporting when the model is reasoning
/// so the chain-of-thought is never shown. Robust to tags split across token
/// boundaries (e.g. `<`, `think`, `>` arriving as three separate pieces).
///
/// Implementation note: it re-scans the full accumulated output on each token
/// rather than maintaining a hand-rolled partial-tag parser. Answers are
/// bounded (a couple thousand tokens), so this stays cheap, and re-scanning a
/// *complete* string sidesteps every split-tag edge case the incremental
/// parser would have to special-case.
#[derive(Debug, Default)]
pub struct ThinkStreamFilter {
    raw: String,
    /// Byte length of answer text already emitted via [`StreamStep::Answer`].
    emitted: usize,
}

impl ThinkStreamFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw token piece; returns what the UI should do about it.
    pub fn push(&mut self, piece: &str) -> StreamStep {
        self.raw.push_str(piece);
        let scanned = scan(&self.raw);

        // Withhold a trailing partial tag: only meaningful at depth 0, where
        // the tail sits in answer text and could still grow into `<think>`.
        let mut answer = scanned.answer;
        if scanned.depth == 0 {
            let hold = dangling_tag_prefix_len(&self.raw);
            if hold > 0 && hold <= answer.len() {
                answer.truncate(answer.len() - hold);
            }
        }

        if answer.len() > self.emitted {
            let delta = answer[self.emitted..].to_string();
            self.emitted = answer.len();
            StreamStep::Answer(delta)
        } else if scanned.depth > 0 {
            StreamStep::Thinking
        } else {
            StreamStep::Idle
        }
    }

    /// Finalize the stream: returns the clean answer, or
    /// [`ThinkStripResult::ReasoningOnly`] if the model never produced answer
    /// text.
    pub fn finish(&self) -> ThinkStripResult {
        strip_think_blocks(&self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_well_formed_think_block() {
        assert_eq!(
            strip_think_blocks("<think>let me reason about this</think>The answer is 42."),
            ThinkStripResult::Answer("The answer is 42.".to_string())
        );
    }

    #[test]
    fn leaves_normal_answer_untouched() {
        assert_eq!(
            strip_think_blocks("Just a normal answer."),
            ThinkStripResult::Answer("Just a normal answer.".to_string())
        );
    }

    #[test]
    fn unterminated_think_is_reasoning_only() {
        assert_eq!(
            strip_think_blocks("<think>reasoning that never ends because the budget ran out"),
            ThinkStripResult::ReasoningOnly
        );
    }

    #[test]
    fn balanced_block_with_no_answer_is_reasoning_only() {
        assert_eq!(
            strip_think_blocks("<think>thought hard, said nothing</think>"),
            ThinkStripResult::ReasoningOnly
        );
    }

    #[test]
    fn empty_think_then_answer_keeps_answer() {
        // Qwen3 in `/no_think` mode emits an empty think block then answers.
        assert_eq!(
            strip_think_blocks("<think>\n\n</think>\n\nHello there."),
            ThinkStripResult::Answer("Hello there.".to_string())
        );
    }

    #[test]
    fn nested_and_multiple_blocks_are_all_removed() {
        assert_eq!(
            strip_think_blocks("<think>a<think>b</think>c</think>Final."),
            ThinkStripResult::Answer("Final.".to_string())
        );
        assert_eq!(
            strip_think_blocks("<think>one</think> mid <think>two</think>end"),
            ThinkStripResult::Answer("mid end".to_string())
        );
    }

    #[test]
    fn answer_before_unterminated_think_survives() {
        assert_eq!(
            strip_think_blocks("Partial answer<think>then it wandered off"),
            ThinkStripResult::Answer("Partial answer".to_string())
        );
    }

    #[test]
    fn stray_closing_tag_is_dropped_not_treated_as_reasoning() {
        assert_eq!(
            strip_think_blocks("Hello</think> world"),
            ThinkStripResult::Answer("Hello world".to_string())
        );
    }

    #[test]
    fn stream_filter_emits_answer_after_thinking() {
        let mut f = ThinkStreamFilter::new();
        let mut answer = String::new();
        let mut saw_thinking = false;
        for piece in [
            "<think>",
            "reasoning ",
            "here",
            "</think>",
            "The ",
            "answer",
        ] {
            match f.push(piece) {
                StreamStep::Answer(d) => answer.push_str(&d),
                StreamStep::Thinking => saw_thinking = true,
                StreamStep::Idle => {}
            }
        }
        assert!(saw_thinking, "should have reported a thinking state");
        assert_eq!(answer, "The answer");
        assert_eq!(
            f.finish(),
            ThinkStripResult::Answer("The answer".to_string())
        );
    }

    #[test]
    fn stream_filter_handles_tag_split_across_tokens() {
        // `<think>` arrives one character at a time, then `</think>` too.
        let mut f = ThinkStreamFilter::new();
        let mut answer = String::new();
        for piece in [
            "<", "th", "ink", ">", "x", "<", "/", "think", ">", "Hi", " there",
        ] {
            if let StreamStep::Answer(d) = f.push(piece) {
                answer.push_str(&d);
            }
        }
        assert_eq!(answer, "Hi there");
        assert_eq!(f.finish(), ThinkStripResult::Answer("Hi there".to_string()));
    }

    #[test]
    fn stream_filter_pure_reasoning_finishes_reasoning_only() {
        let mut f = ThinkStreamFilter::new();
        let mut answer = String::new();
        for piece in ["<think>", "thinking ", "forever with no close"] {
            if let StreamStep::Answer(d) = f.push(piece) {
                answer.push_str(&d);
            }
        }
        assert!(answer.is_empty());
        assert_eq!(f.finish(), ThinkStripResult::ReasoningOnly);
    }

    #[test]
    fn stream_filter_normal_answer_with_no_tags() {
        let mut f = ThinkStreamFilter::new();
        let mut answer = String::new();
        for piece in ["Hello ", "world", "!"] {
            if let StreamStep::Answer(d) = f.push(piece) {
                answer.push_str(&d);
            }
        }
        assert_eq!(answer, "Hello world!");
        assert_eq!(
            f.finish(),
            ThinkStripResult::Answer("Hello world!".to_string())
        );
    }

    #[test]
    fn stream_filter_does_not_emit_a_literal_lt_that_becomes_a_tag() {
        // A lone "<" must be withheld until we know it isn't "<think>".
        let mut f = ThinkStreamFilter::new();
        let step = f.push("answer<");
        // "answer" flushes; the trailing "<" is withheld.
        assert_eq!(step, StreamStep::Answer("answer".to_string()));
    }
}
