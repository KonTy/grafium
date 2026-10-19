//! Offline spell checking.
//!
//! Hunspell dictionaries, run by the pure-Rust `spellbook` crate, are embedded
//! in the binary deflated (see `build.rs`). A language is inflated and parsed
//! the first time a word in its script is checked, so languages that are never
//! enabled cost nothing at run time and only their compressed size on disk.
//!
//! Words are routed by script. A word is checked against the enabled
//! languages written in its script and is accepted when any of them knows it
//! (an English and a Spanish note can share a page). A word in a script that
//! no enabled language covers, or one that mixes scripts, is never flagged.

mod personal;
mod script;

pub use personal::PersonalDictionary;
pub use script::Script;

use crate::error::Result;
use flate2::read::DeflateDecoder;
use parking_lot::RwLock;
use script::{normalize, script_of, MAX_WORD_CHARS};
use serde::Serialize;
use spellbook::Dictionary;
use std::{
    collections::BTreeSet,
    io::{self, Read},
    path::PathBuf,
    sync::OnceLock,
};

/// One bundled dictionary. Adding a language is one `language!` line below
/// plus a directory in `core/dictionaries/`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
    pub native_name: &'static str,
    pub script: Script,
    /// Approximate memory in MB once loaded (measured on 64-bit Linux).
    pub memory_mb: u32,
    #[serde(skip)]
    aff: &'static [u8],
    #[serde(skip)]
    dic: &'static [u8],
    #[serde(skip)]
    dictionary: OnceLock<Option<Dictionary>>,
}

macro_rules! language {
    ($code:literal, $name:literal, $native:literal, $script:ident, $memory:literal) => {
        Language {
            code: $code,
            name: $name,
            native_name: $native,
            script: Script::$script,
            memory_mb: $memory,
            aff: include_bytes!(concat!(env!("OUT_DIR"), "/dictionaries/", $code, ".aff")),
            dic: include_bytes!(concat!(env!("OUT_DIR"), "/dictionaries/", $code, ".dic")),
            dictionary: OnceLock::new(),
        }
    };
}

static LANGUAGES: [Language; 7] = [
    language!("en", "English (US)", "English (US)", Latin, 3),
    language!("en-GB", "English (UK)", "English (UK)", Latin, 3),
    language!("es", "Spanish", "Español", Latin, 6),
    language!("fr", "French", "Français", Latin, 6),
    language!("ru", "Russian", "Русский", Cyrillic, 13),
    language!("hi", "Hindi", "हिन्दी", Devanagari, 15),
    language!("ar", "Arabic", "العربية", Arabic, 45),
];

/// Every bundled language, in the order they are offered.
pub fn languages() -> &'static [Language] {
    &LANGUAGES
}

pub fn language(code: &str) -> Option<&'static Language> {
    LANGUAGES
        .iter()
        .find(|language| language.code.eq_ignore_ascii_case(code))
}

/// The language to start with for a system locale such as `en-GB` or `ru_RU`.
/// Falls back to US English, which is also what an unsupported locale gets.
pub fn default_language_for_locale(locale: &str) -> &'static str {
    let locale = locale.to_ascii_lowercase().replace('_', "-");
    let mut parts = locale.split(|c: char| c == '-' || c == '.' || c == '@');
    let primary = parts.next().unwrap_or("");
    let region = parts.find(|part| part.len() == 2);
    match primary {
        "en" if matches!(region, Some("gb" | "au" | "nz" | "ie" | "in" | "za" | "sg")) => "en-GB",
        "es" => "es",
        "fr" => "fr",
        "ru" => "ru",
        "hi" => "hi",
        "ar" => "ar",
        _ => "en",
    }
}

impl Language {
    fn dictionary(&self) -> Option<&Dictionary> {
        self.dictionary
            .get_or_init(|| match load(self.aff, self.dic) {
                Ok(dictionary) => Some(dictionary),
                Err(error) => {
                    tracing::error!(language = self.code, %error, "could not load the spell-check dictionary");
                    None
                }
            })
            .as_ref()
    }
}

fn inflate(bytes: &[u8]) -> io::Result<String> {
    let mut text = String::new();
    DeflateDecoder::new(bytes).read_to_string(&mut text)?;
    Ok(text)
}

fn load(aff: &[u8], dic: &[u8]) -> std::result::Result<Dictionary, String> {
    let aff = inflate(aff).map_err(|error| error.to_string())?;
    let dic = inflate(dic).map_err(|error| error.to_string())?;
    Dictionary::new(&aff, &dic).map_err(|error| error.to_string())
}

/// The languages named by `codes`, without repeats; unknown codes are ignored
/// so a saved choice that a later build dropped does not break checking.
fn resolve(codes: &[String]) -> Vec<&'static Language> {
    let mut found: Vec<&'static Language> = Vec::new();
    for code in codes {
        if let Some(language) = language(code) {
            if !found.iter().any(|known| known.code == language.code) {
                found.push(language);
            }
        }
    }
    found
}

/// Spell checking plus the user's personal dictionary.
pub struct Speller {
    personal: RwLock<PersonalDictionary>,
}

impl Speller {
    /// `personal_path` is the text file holding the user's own words.
    pub fn open(personal_path: PathBuf) -> Self {
        Self {
            personal: RwLock::new(PersonalDictionary::load(personal_path)),
        }
    }

    /// For each word, whether it is misspelled in the given languages.
    pub fn check_words(&self, words: &[String], languages: &[String]) -> Vec<bool> {
        let languages = resolve(languages);
        let personal = self.personal.read();
        words
            .iter()
            .map(|word| !is_acceptable(word, &languages, &personal))
            .collect()
    }

    /// Up to `limit` corrections for `word`, best first.
    ///
    /// The quick edit-based search runs first. Hunspell's n-gram search scans
    /// the whole dictionary (up to ~300 ms for Russian or Arabic), so it only
    /// runs when the quick one found nothing.
    pub fn suggest(&self, word: &str, languages: &[String], limit: usize) -> Vec<String> {
        let word = normalize(word.trim());
        if limit == 0 || word.is_empty() || word.chars().count() > MAX_WORD_CHARS {
            return Vec::new();
        }
        let Some(script) = script_of(&word) else {
            return Vec::new();
        };
        let dictionaries: Vec<&Dictionary> = resolve(languages)
            .into_iter()
            .filter(|language| language.script == script)
            .filter_map(Language::dictionary)
            .collect();

        let mut pool = Pool::new(&word);
        let mut found = Vec::new();
        self.personal.read().suggest(&word, &mut found);
        pool.add(0, std::mem::take(&mut found));

        for ngram in [false, true] {
            for (index, dictionary) in dictionaries.iter().enumerate() {
                dictionary
                    .suggester()
                    .with_ngram_suggestions(ngram)
                    .suggest(&word, &mut found);
                pool.add(index + 1, std::mem::take(&mut found));
            }
            if !pool.is_empty() {
                break;
            }
        }
        pool.finish(limit)
    }

    pub fn personal_words(&self) -> Vec<String> {
        self.personal.read().words()
    }

    /// Adds a word and returns the updated list.
    pub fn add_personal_word(&self, word: &str) -> Result<Vec<String>> {
        let mut personal = self.personal.write();
        personal.add(word)?;
        Ok(personal.words())
    }

    /// Removes a word and returns the updated list.
    pub fn remove_personal_word(&self, word: &str) -> Result<Vec<String>> {
        let mut personal = self.personal.write();
        personal.remove(word)?;
        Ok(personal.words())
    }
}

/// Whether `word` is fine as written, or cannot be judged.
fn is_acceptable(word: &str, languages: &[&Language], personal: &PersonalDictionary) -> bool {
    let word = normalize(word);
    if word.is_empty() || word.chars().count() > MAX_WORD_CHARS {
        return true;
    }
    let Some(script) = script_of(&word) else {
        return true;
    };
    let mut candidates = languages
        .iter()
        .filter(|language| language.script == script)
        .peekable();
    if candidates.peek().is_none() {
        return true;
    }
    if personal.check(&word) {
        return true;
    }
    // A dictionary that failed to load cannot call anything wrong.
    candidates.any(|language| {
        language
            .dictionary()
            .map_or(true, |dictionary| dictionary.check(&word))
    })
}

struct Candidate {
    text: String,
    source: usize,
    /// Position in its source's own list, best first.
    rank: usize,
}

/// Collects suggestions from several sources and orders them.
struct Pool<'a> {
    word: &'a str,
    candidates: Vec<Candidate>,
}

impl<'a> Pool<'a> {
    fn new(word: &'a str) -> Self {
        Self {
            word,
            candidates: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn add(&mut self, source: usize, suggestions: Vec<String>) {
        for (rank, text) in suggestions.into_iter().enumerate() {
            if text != self.word && !self.candidates.iter().any(|known| known.text == text) {
                self.candidates.push(Candidate { text, source, rank });
            }
        }
    }

    fn finish(mut self, limit: usize) -> Vec<String> {
        let sources: BTreeSet<usize> = self.candidates.iter().map(|c| c.source).collect();
        let lowered = self.word.to_lowercase();
        // One source keeps its own ranking. Several are merged by how close
        // each is to what was typed, so a Spanish dictionary's far-fetched
        // guesses do not outrank English's exact fix. Equally close guesses
        // alternate between sources, best-ranked first, so one language with
        // many near misses cannot push out the other's. Splitting a word in
        // two is rarely what was meant, so those go last.
        let merge = sources.len() > 1;
        self.candidates.sort_by_cached_key(|candidate| {
            let distance = if merge {
                edit_distance(&lowered, &candidate.text.to_lowercase())
            } else {
                0
            };
            (
                candidate.text.contains(' '),
                distance,
                candidate.rank,
                candidate.source,
            )
        });
        self.candidates
            .into_iter()
            .take(limit)
            .map(|candidate| candidate.text)
            .collect()
    }
}

/// Edit distance counting a swap of neighbouring letters as one edit.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut before_previous = vec![0usize; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(before_previous[j - 2] + 1);
            }
            current[j] = best;
        }
        std::mem::swap(&mut before_previous, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests;
