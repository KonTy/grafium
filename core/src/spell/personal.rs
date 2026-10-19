//! The user's own words.
//!
//! A plain text file, one word per line. It only ever grows by one appended
//! line or shrinks by the lines of one removed word: everything else in the
//! file (comments, hand-written entries we do not understand) is written back
//! exactly as it was. A file that cannot be read is never replaced.

use super::script::{is_word_shape, normalize};
use crate::{
    error::{CoreError, Result},
    fsutil,
};
use spellbook::Dictionary;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

pub struct PersonalDictionary {
    path: PathBuf,
    /// The file as read, so unknown lines survive every rewrite.
    text: String,
    words: BTreeSet<String>,
    dictionary: Option<Dictionary>,
    /// Why the file could not be read. While set, nothing is written.
    unreadable: Option<String>,
}

impl PersonalDictionary {
    /// Opens the file at `path`. A missing file is an empty dictionary; one
    /// that exists but cannot be read leaves the dictionary empty and
    /// read-only, so it is checked against but never overwritten.
    pub fn load(path: PathBuf) -> Self {
        let (text, unreadable) = match fs::read_to_string(&path) {
            Ok(text) => (text, None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (String::new(), None),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read the personal dictionary");
                (String::new(), Some(error.to_string()))
            }
        };
        let words = parse_words(&text);
        let dictionary = build(&words);
        Self {
            path,
            text,
            words,
            dictionary,
            unreadable,
        }
    }

    /// The stored words, ordered for display.
    pub fn words(&self) -> Vec<String> {
        let mut words: Vec<String> = self.words.iter().cloned().collect();
        words.sort_by_cached_key(|word| (word.to_lowercase(), word.clone()));
        words
    }

    /// Whether the dictionary accepts `word` (already normalised), applying
    /// the usual rules: a lower-case entry also accepts a capitalised or
    /// upper-case word, a capitalised entry does not accept lower case.
    pub fn check(&self, word: &str) -> bool {
        self.dictionary
            .as_ref()
            .is_some_and(|dictionary| dictionary.check(word))
    }

    pub fn suggest(&self, word: &str, out: &mut Vec<String>) {
        out.clear();
        if let Some(dictionary) = &self.dictionary {
            dictionary
                .suggester()
                .with_ngram_suggestions(false)
                .suggest(word, out);
        }
    }

    /// Adds `word`. Returns `false` when it was already stored.
    pub fn add(&mut self, word: &str) -> Result<bool> {
        let word = normalize(word.trim());
        if !is_word_shape(&word) {
            return Err(CoreError::Other(format!(
                "\u{201C}{word}\u{201D} is not a word that can be added to the dictionary"
            )));
        }
        if self.words.contains(&word) {
            return Ok(false);
        }
        self.ensure_writable()?;

        let mut text = self.text.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&word);
        text.push('\n');
        self.save(&text)?;

        self.text = text;
        self.words.insert(word);
        self.dictionary = build(&self.words);
        Ok(true)
    }

    /// Removes every line holding `word`. Returns `false` when it was not stored.
    pub fn remove(&mut self, word: &str) -> Result<bool> {
        let word = normalize(word.trim());
        if !self.words.contains(&word) {
            return Ok(false);
        }
        self.ensure_writable()?;

        let text: String = self
            .text
            .split_inclusive('\n')
            .filter(|line| entry(line).as_deref() != Some(word.as_str()))
            .collect();
        self.save(&text)?;

        self.text = text;
        self.words.remove(&word);
        self.dictionary = build(&self.words);
        Ok(true)
    }

    fn ensure_writable(&self) -> Result<()> {
        match &self.unreadable {
            Some(reason) => Err(CoreError::Other(format!(
                "Your personal dictionary could not be read ({reason}), so it was left untouched"
            ))),
            None => Ok(()),
        }
    }

    fn save(&self, text: &str) -> Result<()> {
        fsutil::atomic_write(&self.path, text.as_bytes())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The word on one line of the file, if the line holds one.
fn entry(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let word = normalize(line);
    is_word_shape(&word).then_some(word)
}

fn parse_words(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(entry).collect()
}

fn build(words: &BTreeSet<String>) -> Option<Dictionary> {
    if words.is_empty() {
        return None;
    }
    let mut dic = format!("{}\n", words.len());
    for word in words {
        dic.push_str(word);
        dic.push('\n');
    }
    match Dictionary::new("SET UTF-8\n", &dic) {
        Ok(dictionary) => Some(dictionary),
        Err(error) => {
            tracing::error!(%error, "could not build the personal dictionary");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(dir: &tempfile::TempDir) -> PersonalDictionary {
        PersonalDictionary::load(dir.path().join("personal.txt"))
    }

    #[test]
    fn a_missing_file_is_empty_and_is_not_created_by_reading() {
        let dir = tempfile::tempdir().unwrap();
        let personal = open(&dir);
        assert!(personal.words().is_empty());
        assert!(!personal.check("Grafium"));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn added_words_are_checked_saved_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut personal = open(&dir);
        assert!(personal.add("Grafium").unwrap());
        assert!(!personal.add("Grafium").unwrap());
        assert!(personal.check("Grafium"));
        assert!(personal.check("GRAFIUM"));
        assert!(
            !personal.check("grafium"),
            "a capitalised entry is case-sensitive"
        );

        let reopened = open(&dir);
        assert_eq!(reopened.words(), vec!["Grafium"]);
        assert!(reopened.check("Grafium"));
    }

    #[test]
    fn a_lower_case_entry_accepts_any_capitalisation() {
        let dir = tempfile::tempdir().unwrap();
        let mut personal = open(&dir);
        personal.add("kubectl").unwrap();
        assert!(personal.check("kubectl"));
        assert!(personal.check("Kubectl"));
        assert!(personal.check("KUBECTL"));
    }

    #[test]
    fn words_are_stored_in_normalised_form() {
        let dir = tempfile::tempdir().unwrap();
        let mut personal = open(&dir);
        personal.add("  don’t ").unwrap();
        assert_eq!(personal.words(), vec!["don't"]);
        assert!(!personal.add("don't").unwrap());
    }

    #[test]
    fn removing_a_word_leaves_every_other_line_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.txt");
        fs::write(&path, "# my words\nGrafium\nnot a word line\nTauri").unwrap();
        let mut personal = PersonalDictionary::load(path.clone());
        assert_eq!(personal.words(), vec!["Grafium", "Tauri"]);

        assert!(personal.remove("Grafium").unwrap());
        assert!(!personal.remove("Grafium").unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# my words\nnot a word line\nTauri"
        );

        personal.add("Svelte").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# my words\nnot a word line\nTauri\nSvelte\n"
        );
    }

    #[test]
    fn unusable_words_are_rejected_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut personal = open(&dir);
        for word in ["", "two words", "a/b", "v2", "-x", "日本"] {
            assert!(personal.add(word).is_err(), "{word:?}");
        }
        assert!(!dir.path().join("personal.txt").exists());
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.txt");
        let invalid = [0x47, 0x72, 0xFF, 0xFE, b'\n'];
        fs::write(&path, invalid).unwrap();

        let mut personal = PersonalDictionary::load(path.clone());
        assert!(personal.words().is_empty());
        assert!(personal.add("Grafium").is_err());
        assert!(personal.remove("Grafium").is_ok());
        assert_eq!(fs::read(&path).unwrap(), invalid);
    }

    #[test]
    fn suggestions_come_from_the_personal_words() {
        let dir = tempfile::tempdir().unwrap();
        let mut personal = open(&dir);
        personal.add("Grafium").unwrap();
        let mut out = Vec::new();
        personal.suggest("Grafuim", &mut out);
        assert_eq!(out.first().map(String::as_str), Some("Grafium"));
    }
}
