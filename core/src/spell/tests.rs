use super::*;

fn speller() -> (tempfile::TempDir, Speller) {
    let dir = tempfile::tempdir().unwrap();
    let speller = Speller::open(dir.path().join("personal-dictionary.txt"));
    (dir, speller)
}

fn codes(codes: &[&str]) -> Vec<String> {
    codes.iter().map(|code| (*code).to_owned()).collect()
}

/// For each word, whether it is flagged when `languages` are enabled.
fn flagged(speller: &Speller, languages: &[&str], words: &[&str]) -> Vec<bool> {
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    speller.check_words(&words, &codes(languages))
}

fn suggest(speller: &Speller, languages: &[&str], word: &str) -> Vec<String> {
    speller.suggest(word, &codes(languages), 5)
}

#[test]
fn every_bundled_dictionary_loads() {
    for language in languages() {
        assert!(
            language.dictionary().is_some(),
            "{} did not load",
            language.code
        );
    }
}

#[test]
fn language_codes_are_unique_and_found_without_regard_to_case() {
    let mut seen = BTreeSet::new();
    for language in languages() {
        assert!(
            seen.insert(language.code.to_ascii_lowercase()),
            "{} is listed twice",
            language.code
        );
    }
    assert_eq!(language("EN-gb").map(|found| found.code), Some("en-GB"));
    assert!(language("de").is_none());
}

#[test]
fn each_language_accepts_its_words_and_flags_typos() {
    let (_dir, speller) = speller();
    let cases = [
        ("en", "library", "libary"),
        ("en-GB", "colour", "colur"),
        ("es", "biblioteca", "bibloteca"),
        ("fr", "bibliothèque", "bibliotheque"),
        ("ru", "библиотека", "библеотека"),
        ("hi", "किताब", "किताबब"),
        ("ar", "مكتبة", "مكتبةة"),
    ];
    for (code, correct, typo) in cases {
        assert_eq!(
            flagged(&speller, &[code], &[correct, typo]),
            [false, true],
            "{code}: {correct} / {typo}"
        );
    }
}

#[test]
fn american_and_british_spellings_follow_the_chosen_variant() {
    let (_dir, speller) = speller();
    assert_eq!(
        flagged(&speller, &["en"], &["color", "colour"]),
        [false, true]
    );
    assert_eq!(
        flagged(&speller, &["en-GB"], &["color", "colour"]),
        [true, false]
    );
    assert_eq!(
        flagged(&speller, &["en", "en-GB"], &["color", "colour"]),
        [false, false]
    );
}

#[test]
fn typographic_apostrophes_and_hyphens_are_accepted() {
    let (_dir, speller) = speller();
    assert_eq!(
        flagged(&speller, &["en"], &["don't", "don’t", "dont", "well-known"]),
        [false, false, true, false]
    );
    assert_eq!(
        flagged(&speller, &["fr"], &["l'homme", "l’homme", "lhomme"]),
        [false, false, true]
    );
}

#[test]
fn arabic_vowel_marks_and_tatweel_do_not_matter() {
    let (_dir, speller) = speller();
    assert_eq!(
        flagged(&speller, &["ar"], &["كتاب", "كِتَاب", "كتــاب"]),
        [false, false, false]
    );
}

#[test]
fn hindi_nukta_letters_match_however_they_were_typed() {
    let (_dir, speller) = speller();
    let precomposed = "\u{0958}\u{093E}";
    let decomposed = "\u{0915}\u{093C}\u{093E}";
    assert_eq!(
        flagged(&speller, &["hi"], &[precomposed, decomposed]),
        [false, false]
    );
}

#[test]
fn words_are_only_judged_by_languages_of_their_script() {
    let (_dir, speller) = speller();
    assert_eq!(
        flagged(&speller, &["en"], &["привет", "превет", "مرحبا", "नमस्ते"]),
        [false; 4]
    );
    assert_eq!(flagged(&speller, &["ru"], &["hello", "helo"]), [false; 2]);
    assert_eq!(flagged(&speller, &["ru"], &["превет"]), [true]);
}

#[test]
fn mixed_script_numeric_and_oversized_words_are_never_flagged() {
    let (_dir, speller) = speller();
    let oversized = "a".repeat(MAX_WORD_CHARS + 1);
    let words = [
        "helloпривет",
        "abc123",
        "v2",
        "snake_case",
        "日本語",
        "",
        "'-'",
        oversized.as_str(),
    ];
    assert_eq!(flagged(&speller, &["en", "ru"], &words), [false; 8]);
}

#[test]
fn languages_of_one_script_are_united() {
    let (_dir, speller) = speller();
    let words = ["hello", "biblioteca", "helo", "bibloteca"];
    assert_eq!(
        flagged(&speller, &["en", "es"], &words),
        [false, false, true, true]
    );
    assert_eq!(flagged(&speller, &["en"], &["biblioteca"]), [true]);
    assert_eq!(flagged(&speller, &["es"], &["hello"]), [true]);
}

#[test]
fn no_enabled_language_means_nothing_is_flagged() {
    let (_dir, speller) = speller();
    assert_eq!(flagged(&speller, &[], &["helo"]), [false]);
    assert_eq!(flagged(&speller, &["xx"], &["helo"]), [false]);
    assert_eq!(flagged(&speller, &["xx", "EN", "en"], &["helo"]), [true]);
}

#[test]
fn results_line_up_with_the_words_sent() {
    let (_dir, speller) = speller();
    let words = ["hello", "helo", "world", "wrld", "hello"];
    assert_eq!(
        flagged(&speller, &["en"], &words),
        [false, true, false, true, false]
    );
    assert!(flagged(&speller, &["en"], &[]).is_empty());
}

#[test]
fn personal_words_are_accepted_and_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("personal-dictionary.txt");
    let speller = Speller::open(path.clone());
    assert_eq!(flagged(&speller, &["en"], &["Grafium"]), [true]);

    assert_eq!(speller.add_personal_word("Grafium").unwrap(), ["Grafium"]);
    assert_eq!(flagged(&speller, &["en"], &["Grafium"]), [false]);
    assert_eq!(flagged(&speller, &["es", "en"], &["Grafium"]), [false]);

    let reopened = Speller::open(path);
    assert_eq!(reopened.personal_words(), ["Grafium"]);
    assert_eq!(flagged(&reopened, &["en"], &["Grafium"]), [false]);

    assert!(reopened.remove_personal_word("Grafium").unwrap().is_empty());
    assert_eq!(flagged(&reopened, &["en"], &["Grafium"]), [true]);
}

#[test]
fn a_personal_word_is_accepted_even_where_no_language_covers_its_script() {
    let (_dir, speller) = speller();
    speller.add_personal_word("Москва").unwrap();
    assert_eq!(flagged(&speller, &["en"], &["Москва"]), [false]);
    assert_eq!(flagged(&speller, &["ru"], &["Москва"]), [false]);
}

#[test]
fn bad_personal_words_are_refused() {
    let (_dir, speller) = speller();
    assert!(speller.add_personal_word("two words").is_err());
    assert!(speller.add_personal_word("").is_err());
    assert!(speller.personal_words().is_empty());
}

#[test]
fn suggestions_put_the_obvious_fix_first() {
    let (_dir, speller) = speller();
    let cases = [
        ("en", "recieve", "receive"),
        ("en-GB", "organization", "organisation"),
        ("es", "informacion", "información"),
        ("fr", "exellent", "excellent"),
        ("ru", "здраствуйте", "здравствуйте"),
        ("hi", "किताबब", "किताब"),
        ("ar", "جامععة", "جامعة"),
    ];
    for (code, typo, fix) in cases {
        let found = suggest(&speller, &[code], typo);
        assert_eq!(
            found.first().map(String::as_str),
            Some(fix),
            "{code}: {typo} -> {found:?}"
        );
    }
}

#[test]
fn suggestions_follow_the_capitalisation_of_the_word() {
    let (_dir, speller) = speller();
    let cases = [
        ("en", "Recieve", "Receive"),
        ("en", "RECIEVE", "RECEIVE"),
        ("es", "Informacion", "Información"),
        ("ru", "Здраствуйте", "Здравствуйте"),
    ];
    for (code, typo, fix) in cases {
        let found = suggest(&speller, &[code], typo);
        assert_eq!(
            found.first().map(String::as_str),
            Some(fix),
            "{code}: {typo} -> {found:?}"
        );
    }
}

#[test]
fn a_correction_ranked_lower_is_still_offered() {
    let (_dir, speller) = speller();
    let found = suggest(&speller, &["en"], "helo");
    assert!(found.contains(&"hello".to_owned()), "{found:?}");
}

#[test]
fn suggestions_respect_the_limit_and_the_original_word() {
    let (_dir, speller) = speller();
    assert!(speller.suggest("helo", &codes(&["en"]), 0).is_empty());
    assert!(speller.suggest("helo", &codes(&["en"]), 2).len() <= 2);
    let found = suggest(&speller, &["en"], "helo");
    assert!(!found.iter().any(|suggestion| suggestion == "helo"));
    assert!(suggest(&speller, &["en"], "  helo ").contains(&"hello".to_owned()));
}

#[test]
fn suggestions_are_only_offered_for_words_that_can_be_judged() {
    let (_dir, speller) = speller();
    assert!(suggest(&speller, &["en"], "превет").is_empty());
    assert!(suggest(&speller, &["en"], "helloпривет").is_empty());
    assert!(suggest(&speller, &["en"], "abc123").is_empty());
    assert!(suggest(&speller, &["en"], "").is_empty());
    assert!(suggest(&speller, &[], "helo").is_empty());
}

#[test]
fn suggestions_from_several_languages_are_merged_by_closeness() {
    let (_dir, speller) = speller();
    let found = suggest(&speller, &["en", "es"], "bibloteca");
    assert_eq!(found.first().map(String::as_str), Some("biblioteca"));
    // Spanish alone has five near misses for "helo"; they must not crowd out
    // the English fix.
    let found = suggest(&speller, &["es", "en"], "helo");
    assert!(found.contains(&"hello".to_owned()), "{found:?}");
}

#[test]
fn suggestions_include_personal_words() {
    let (_dir, speller) = speller();
    speller.add_personal_word("Grafium").unwrap();
    assert_eq!(suggest(&speller, &["en"], "Grafuim")[0], "Grafium");
}

#[test]
fn the_slow_search_runs_only_when_the_quick_one_finds_nothing() {
    let (_dir, speller) = speller();
    // No single-edit neighbours, so only the n-gram search finds anything.
    assert_eq!(suggest(&speller, &["en"], "nolege"), ["college"]);
    // The quick search already has answers; the n-gram search would add
    // "reverie" and is not run.
    assert_eq!(
        suggest(&speller, &["en"], "recieve"),
        ["receive", "relieve"]
    );
}

#[test]
fn splits_come_after_single_word_suggestions() {
    let mut pool = Pool::new("alot");
    pool.add(
        1,
        vec!["a lot".to_owned(), "allot".to_owned(), "alto".to_owned()],
    );
    assert_eq!(pool.finish(5), ["allot", "alto", "a lot"]);
}

#[test]
fn sources_are_merged_by_edit_distance_and_deduplicated() {
    let mut pool = Pool::new("cat");
    pool.add(
        1,
        vec!["cast".to_owned(), "cat".to_owned(), "castle".to_owned()],
    );
    pool.add(
        2,
        vec!["cut".to_owned(), "cast".to_owned(), "cats".to_owned()],
    );
    assert_eq!(pool.finish(5), ["cast", "cut", "cats", "castle"]);
}

#[test]
fn equally_close_suggestions_alternate_between_sources() {
    let mut pool = Pool::new("cat");
    pool.add(
        1,
        vec!["cut".to_owned(), "cot".to_owned(), "cit".to_owned()],
    );
    pool.add(2, vec!["cats".to_owned(), "cart".to_owned()]);
    assert_eq!(pool.finish(4), ["cut", "cats", "cot", "cart"]);
}

#[test]
fn a_single_source_keeps_its_own_ranking() {
    let mut pool = Pool::new("cat");
    pool.add(1, vec!["castle".to_owned(), "cast".to_owned()]);
    assert_eq!(pool.finish(5), ["castle", "cast"]);
}

#[test]
fn edit_distance_counts_swaps_as_one_edit() {
    assert_eq!(edit_distance("", ""), 0);
    assert_eq!(edit_distance("abc", ""), 3);
    assert_eq!(edit_distance("", "abc"), 3);
    assert_eq!(edit_distance("kitten", "sitting"), 3);
    assert_eq!(edit_distance("teh", "the"), 1);
    assert_eq!(edit_distance("recieve", "receive"), 1);
    assert_eq!(edit_distance("привет", "превет"), 1);
    assert_eq!(edit_distance("same", "same"), 0);
}

#[test]
fn the_system_locale_picks_a_starting_language() {
    let cases = [
        ("en_US.UTF-8", "en"),
        ("en-US", "en"),
        ("en", "en"),
        ("en_GB.UTF-8", "en-GB"),
        ("en-AU", "en-GB"),
        ("en_IN", "en-GB"),
        ("en-Latn-GB", "en-GB"),
        ("es_MX.UTF-8", "es"),
        ("fr-CA", "fr"),
        ("ru_RU", "ru"),
        ("hi_IN", "hi"),
        ("ar-EG", "ar"),
        ("de_DE", "en"),
        ("zh-Hans-CN", "en"),
        ("C", "en"),
        ("", "en"),
    ];
    for (locale, expected) in cases {
        assert_eq!(default_language_for_locale(locale), expected, "{locale}");
        assert!(language(expected).is_some());
    }
}
