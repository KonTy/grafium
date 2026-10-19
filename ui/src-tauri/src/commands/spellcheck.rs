//! Spell-check commands: bundled languages, preferences, checking, suggestions
//! and the personal dictionary. The engine lives in `grafium_core::spell`.

use grafium_core::spell::{self, Language, Speller};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tauri::State;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

const PREFERENCES_FILE: &str = "spellcheck.json";
const PERSONAL_DICTIONARY_FILE: &str = "personal-dictionary.txt";
const DEFAULT_SUGGESTIONS: usize = 5;
const MAX_SUGGESTIONS: usize = 20;

/// The speller, opened on first use so start-up does not touch the personal
/// dictionary until a note is actually checked.
#[derive(Default)]
pub struct SpellState {
    speller: OnceLock<Arc<Speller>>,
}

impl SpellState {
    fn speller(&self, app: &tauri::AppHandle) -> Result<Arc<Speller>, String> {
        if let Some(speller) = self.speller.get() {
            return Ok(speller.clone());
        }
        let path = super::app_config_dir(app)?.join(PERSONAL_DICTIONARY_FILE);
        Ok(self
            .speller
            .get_or_init(|| Arc::new(Speller::open(path)))
            .clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellcheckPreferences {
    enabled: bool,
    languages: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpellcheckPreferencePatch {
    enabled: Option<bool>,
    languages: Option<Vec<String>>,
}

/// Known language codes, spelled as the registry spells them, without repeats
/// and in the registry's order so the result does not depend on click order.
fn canonical_languages(codes: &[String]) -> Vec<String> {
    spell::languages()
        .iter()
        .filter(|language| {
            codes
                .iter()
                .any(|code| code.eq_ignore_ascii_case(language.code))
        })
        .map(|language| language.code.to_owned())
        .collect()
}

fn read_stored(dir: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read(dir.join(PREFERENCES_FILE)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(Value::Object(stored)) => Ok(stored),
            Ok(_) => Err("Spell check preferences must be a JSON object".into()),
            Err(err) => Err(format!("Could not read spell check preferences: {err}")),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(err) => Err(format!("Could not read spell check preferences: {err}")),
    }
}

/// Spell checking is on by default, in the language of the system locale until
/// the user chooses others. A default is never written back to disk.
fn resolve(stored: &Map<String, Value>, locale: &str) -> SpellcheckPreferences {
    let enabled = stored
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let languages = match stored.get("languages").and_then(Value::as_array) {
        Some(codes) => {
            let codes: Vec<String> = codes
                .iter()
                .filter_map(|code| code.as_str().map(str::to_owned))
                .collect();
            canonical_languages(&codes)
        }
        None => vec![spell::default_language_for_locale(locale).to_owned()],
    };
    SpellcheckPreferences { enabled, languages }
}

fn read_preferences(dir: &Path, locale: &str) -> Result<SpellcheckPreferences, String> {
    Ok(resolve(&read_stored(dir)?, locale))
}

fn write_preferences(
    dir: &Path,
    patch: SpellcheckPreferencePatch,
    locale: &str,
) -> Result<SpellcheckPreferences, String> {
    if let Some(codes) = &patch.languages {
        if let Some(unknown) = codes.iter().find(|code| spell::language(code).is_none()) {
            return Err(format!(
                "Spell check has no dictionary for \u{201C}{unknown}\u{201D}"
            ));
        }
    }
    // Merge under one lock so separate UI changes cannot overwrite each other.
    let _guard = WRITE_LOCK.lock().map_err(|err| err.to_string())?;
    // Keys written by another version of Grafium stay in the file.
    let mut stored = read_stored(dir)?;
    if let Some(enabled) = patch.enabled {
        stored.insert("enabled".into(), Value::Bool(enabled));
    }
    if let Some(codes) = patch.languages {
        let codes = canonical_languages(&codes);
        stored.insert("languages".into(), Value::from(codes));
    }
    let bytes = serde_json::to_vec(&stored).map_err(|err| err.to_string())?;
    grafium_core::fsutil::atomic_write(&dir.join(PREFERENCES_FILE), &bytes)
        .map_err(|err| format!("Could not save spell check preferences: {err}"))?;
    Ok(resolve(&stored, locale))
}

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    super::app_config_dir(app)
}

/// Runs `work` on the blocking pool: the first word in a script loads that
/// language's dictionary, which takes up to a few hundred milliseconds.
async fn with_speller<T, F>(
    app: &tauri::AppHandle,
    state: &SpellState,
    work: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Speller) -> Result<T, String> + Send + 'static,
{
    let speller = state.speller(app)?;
    tauri::async_runtime::spawn_blocking(move || work(&speller))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
pub fn spell_languages() -> &'static [Language] {
    spell::languages()
}

#[tauri::command]
pub fn get_spellcheck_preferences(
    app: tauri::AppHandle,
    locale: Option<String>,
) -> Result<SpellcheckPreferences, String> {
    read_preferences(&config_dir(&app)?, locale.as_deref().unwrap_or_default())
}

#[tauri::command]
pub fn set_spellcheck_preferences(
    app: tauri::AppHandle,
    preferences: SpellcheckPreferencePatch,
    locale: Option<String>,
) -> Result<SpellcheckPreferences, String> {
    write_preferences(
        &config_dir(&app)?,
        preferences,
        locale.as_deref().unwrap_or_default(),
    )
}

/// For each word, whether it is misspelled.
#[tauri::command]
pub async fn spell_check_words(
    app: tauri::AppHandle,
    state: State<'_, SpellState>,
    words: Vec<String>,
    languages: Vec<String>,
) -> Result<Vec<bool>, String> {
    with_speller(&app, &state, move |speller| {
        Ok(speller.check_words(&words, &languages))
    })
    .await
}

#[tauri::command]
pub async fn spell_suggest(
    app: tauri::AppHandle,
    state: State<'_, SpellState>,
    word: String,
    languages: Vec<String>,
    limit: Option<usize>,
) -> Result<Vec<String>, String> {
    let limit = limit.unwrap_or(DEFAULT_SUGGESTIONS).min(MAX_SUGGESTIONS);
    with_speller(&app, &state, move |speller| {
        Ok(speller.suggest(&word, &languages, limit))
    })
    .await
}

#[tauri::command]
pub async fn spell_personal_words(
    app: tauri::AppHandle,
    state: State<'_, SpellState>,
) -> Result<Vec<String>, String> {
    with_speller(&app, &state, |speller| Ok(speller.personal_words())).await
}

/// Adds a word and returns the updated list.
#[tauri::command]
pub async fn spell_add_personal_word(
    app: tauri::AppHandle,
    state: State<'_, SpellState>,
    word: String,
) -> Result<Vec<String>, String> {
    with_speller(&app, &state, move |speller| {
        speller
            .add_personal_word(&word)
            .map_err(|err| err.to_string())
    })
    .await
}

/// Removes a word and returns the updated list.
#[tauri::command]
pub async fn spell_remove_personal_word(
    app: tauri::AppHandle,
    state: State<'_, SpellState>,
    word: String,
) -> Result<Vec<String>, String> {
    with_speller(&app, &state, move |speller| {
        speller
            .remove_personal_word(&word)
            .map_err(|err| err.to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(enabled: Option<bool>, languages: Option<&[&str]>) -> SpellcheckPreferencePatch {
        SpellcheckPreferencePatch {
            enabled,
            languages: languages.map(|codes| codes.iter().map(|code| (*code).to_owned()).collect()),
        }
    }

    #[test]
    fn first_launch_checks_in_the_system_language_without_writing_anything() {
        let dir = tempfile::tempdir().unwrap();
        for (locale, expected) in [
            ("en-US", "en"),
            ("en-GB", "en-GB"),
            ("ru_RU.UTF-8", "ru"),
            ("de", "en"),
            ("", "en"),
        ] {
            assert_eq!(
                read_preferences(dir.path(), locale).unwrap(),
                SpellcheckPreferences {
                    enabled: true,
                    languages: vec![expected.to_owned()],
                },
                "{locale}"
            );
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn each_choice_survives_a_restart_without_overwriting_the_other() {
        let dir = tempfile::tempdir().unwrap();
        write_preferences(dir.path(), patch(Some(false), None), "en").unwrap();
        let saved = read_preferences(dir.path(), "fr").unwrap();
        assert!(!saved.enabled);
        assert_eq!(saved.languages, ["fr"], "languages still follow the locale");

        write_preferences(dir.path(), patch(None, Some(&["es", "en"])), "en").unwrap();
        let saved = read_preferences(dir.path(), "fr").unwrap();
        assert!(
            !saved.enabled,
            "choosing languages must not turn checking back on"
        );
        assert_eq!(saved.languages, ["en", "es"]);

        let saved = write_preferences(dir.path(), patch(Some(true), None), "en").unwrap();
        assert_eq!(saved.languages, ["en", "es"]);
        assert!(saved.enabled);
    }

    #[test]
    fn languages_are_stored_once_in_the_order_they_are_offered() {
        let dir = tempfile::tempdir().unwrap();
        let saved = write_preferences(
            dir.path(),
            patch(None, Some(&["ar", "RU", "en-gb", "ar", "en"])),
            "en",
        )
        .unwrap();
        assert_eq!(saved.languages, ["en", "en-GB", "ru", "ar"]);
    }

    #[test]
    fn choosing_no_language_is_remembered_and_is_not_the_default() {
        let dir = tempfile::tempdir().unwrap();
        write_preferences(dir.path(), patch(None, Some(&[])), "en").unwrap();
        assert!(read_preferences(dir.path(), "ru")
            .unwrap()
            .languages
            .is_empty());
    }

    #[test]
    fn an_unknown_language_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_preferences(dir.path(), patch(None, Some(&["en", "xx"])), "en").is_err());
        assert!(!dir.path().join(PREFERENCES_FILE).exists());
    }

    #[test]
    fn a_language_a_later_build_dropped_is_ignored_but_kept_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PREFERENCES_FILE),
            r#"{"enabled":true,"languages":["pt","es"],"future":{"x":1}}"#,
        )
        .unwrap();
        assert_eq!(
            read_preferences(dir.path(), "en").unwrap().languages,
            ["es"]
        );

        write_preferences(dir.path(), patch(Some(false), None), "en").unwrap();
        let stored: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(PREFERENCES_FILE)).unwrap())
                .unwrap();
        assert_eq!(stored["future"], serde_json::json!({"x": 1}));
        assert_eq!(stored["languages"], serde_json::json!(["pt", "es"]));
        assert_eq!(stored["enabled"], false);
    }

    #[test]
    fn an_unreadable_file_is_reported_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PREFERENCES_FILE);
        for content in ["{not json", "[1,2]", "7"] {
            std::fs::write(&path, content).unwrap();
            assert!(read_preferences(dir.path(), "en").is_err(), "{content}");
            assert!(write_preferences(dir.path(), patch(Some(false), None), "en").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
    }

    #[test]
    fn patches_reject_fields_this_build_does_not_know() {
        assert!(serde_json::from_str::<SpellcheckPreferencePatch>(r#"{"enabled":true}"#).is_ok());
        assert!(serde_json::from_str::<SpellcheckPreferencePatch>(r#"{"enable":true}"#).is_err());
    }

    #[test]
    fn the_language_list_serialises_for_the_settings_page() {
        let json = serde_json::to_value(spell_languages()).unwrap();
        let first = &json[0];
        assert_eq!(first["code"], "en");
        assert_eq!(first["script"], "latin");
        assert!(first["nativeName"].is_string());
        assert!(first["memoryMb"].is_u64());
        assert!(first.get("aff").is_none() && first.get("dictionary").is_none());
        assert_eq!(json.as_array().unwrap().len(), spell::languages().len());
    }
}
