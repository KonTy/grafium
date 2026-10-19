//! Script detection and normalisation, applied to every word before lookup.
//!
//! A word is only checked against languages written in its script, and only
//! when it is made of one script. Words in a script nobody enabled (Greek,
//! Hebrew, Han, …), words that mix scripts, and anything with digits or other
//! symbols are never flagged: we cannot judge them, so we say nothing.

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

/// Longest word we look up. Longer runs are identifiers or pasted data.
pub const MAX_WORD_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Script {
    Latin,
    Cyrillic,
    Devanagari,
    Arabic,
}

enum CharClass {
    Script(Script),
    /// Combining marks, apostrophes and hyphens take the script around them.
    Neutral,
    /// A letter of an uncovered script, a digit, or punctuation.
    Other,
}

fn classify(c: char) -> CharClass {
    use CharClass::{Neutral, Other, Script as In};
    match c {
        // Hindi and Arabic digits, danda and Arabic punctuation sit inside
        // the script blocks below but are not letters.
        '\u{0964}'..='\u{0970}'
        | '\u{060C}'
        | '\u{061B}'
        | '\u{061F}'
        | '\u{0660}'..='\u{066D}'
        | '\u{06D4}'
        | '\u{06F0}'..='\u{06F9}' => Other,
        'a'..='z'
        | 'A'..='Z'
        | '\u{00AA}'
        | '\u{00BA}'
        | '\u{00C0}'..='\u{00D6}'
        | '\u{00D8}'..='\u{00F6}'
        | '\u{00F8}'..='\u{024F}'
        | '\u{1E00}'..='\u{1EFF}'
        | '\u{2C60}'..='\u{2C7F}'
        | '\u{A720}'..='\u{A7FF}' => In(Script::Latin),
        '\u{0400}'..='\u{052F}'
        | '\u{1C80}'..='\u{1C8F}'
        | '\u{2DE0}'..='\u{2DFF}'
        | '\u{A640}'..='\u{A69F}' => In(Script::Cyrillic),
        '\u{0900}'..='\u{097F}' | '\u{A8E0}'..='\u{A8FF}' => In(Script::Devanagari),
        '\u{0600}'..='\u{06FF}'
        | '\u{0750}'..='\u{077F}'
        | '\u{08A0}'..='\u{08FF}'
        | '\u{FB50}'..='\u{FDFF}'
        | '\u{FE70}'..='\u{FEFF}' => In(Script::Arabic),
        '\u{0300}'..='\u{036F}' | '\u{200C}' | '\u{200D}' | '\'' | '-' => Neutral,
        _ => Other,
    }
}

/// The single script a word is written in, or `None` when it mixes scripts,
/// contains anything that is not a letter, or has no letters at all.
pub fn script_of(word: &str) -> Option<Script> {
    let mut found = None;
    for c in word.chars() {
        match classify(c) {
            CharClass::Script(script) => match found {
                None => found = Some(script),
                Some(existing) if existing == script => {}
                Some(_) => return None,
            },
            CharClass::Neutral => {}
            CharClass::Other => return None,
        }
    }
    found
}

/// Canonical form used for lookup: composed Unicode, no zero-width joiners or
/// soft hyphens, straight apostrophes and hyphens.
///
/// Composition matters for Hindi, whose dictionary stores a nukta letter as
/// base plus nukta while keyboards and pasted text often use the single
/// precomposed code point.
pub fn normalize(word: &str) -> String {
    word.nfc()
        .filter(|c| {
            !matches!(
                c,
                '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}'
            )
        })
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            '\u{2010}' | '\u{2011}' => '-',
            c => c,
        })
        .collect()
}

/// Whether `word` (already normalised) can be stored in a personal dictionary:
/// letters of a covered script, with `'` or `-` only between letters.
pub fn is_word_shape(word: &str) -> bool {
    let (Some(first), Some(last)) = (word.chars().next(), word.chars().next_back()) else {
        return false;
    };
    word.chars().count() <= MAX_WORD_CHARS
        && !matches!(first, '\'' | '-')
        && !matches!(last, '\'' | '-')
        && script_of(word).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_routed_by_script() {
        assert_eq!(script_of("hello"), Some(Script::Latin));
        assert_eq!(script_of("über"), Some(Script::Latin));
        assert_eq!(script_of("don't"), Some(Script::Latin));
        assert_eq!(script_of("привет"), Some(Script::Cyrillic));
        assert_eq!(script_of("हिन्दी"), Some(Script::Devanagari));
        assert_eq!(script_of("مكتبة"), Some(Script::Arabic));
    }

    #[test]
    fn mixed_symbol_and_uncovered_words_are_not_judged() {
        assert_eq!(script_of("helloпривет"), None);
        assert_eq!(script_of("abc123"), None);
        assert_eq!(script_of("καλημέρα"), None);
        assert_eq!(script_of("שלום"), None);
        assert_eq!(script_of("日本語"), None);
        assert_eq!(script_of("१२३"), None);
        assert_eq!(script_of("٣٤٥"), None);
        assert_eq!(script_of("है।"), None);
        assert_eq!(script_of("'-'"), None);
        assert_eq!(script_of(""), None);
    }

    #[test]
    fn normalisation_composes_and_straightens() {
        assert_eq!(normalize("don’t"), "don't");
        assert_eq!(normalize("l’homme"), "l'homme");
        assert_eq!(normalize("well\u{2011}known"), "well-known");
        assert_eq!(normalize("soft\u{00AD}hyphen"), "softhyphen");
        // Precomposed क़ becomes क + nukta, which is how the dictionary writes it.
        assert_eq!(normalize("\u{0958}"), "\u{0915}\u{093C}");
        assert_eq!(normalize("e\u{0301}"), "é");
        assert_eq!(normalize("क्\u{200D}ष"), "क्ष");
    }

    #[test]
    fn personal_words_need_letters_and_no_dangling_punctuation() {
        assert!(is_word_shape("Grafium"));
        assert!(is_word_shape("o'clock"));
        assert!(is_word_shape("well-known"));
        assert!(is_word_shape("Москва"));
        assert!(!is_word_shape(""));
        assert!(!is_word_shape("'tis"));
        assert!(!is_word_shape("dash-"));
        assert!(!is_word_shape("two words"));
        assert!(!is_word_shape("a/b"));
        assert!(!is_word_shape("v2"));
        assert!(!is_word_shape(&"a".repeat(MAX_WORD_CHARS + 1)));
    }
}
