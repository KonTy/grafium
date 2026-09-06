//! How a markdown asset reference maps onto a real file.
//!
//! Media now saves beside the page that uses it, so a reference resolves
//! against the page's own directory. The tricky part is not the new shape but
//! the old one: notes written before that change refer to a single shared
//! `assets/` folder, and a broken image is easy to miss across thousands of
//! files, so the fallback that keeps them working is pinned down here.

use grafium_core::graph::resolve_asset_path;
use std::fs;
use tempfile::TempDir;

pub(crate) fn graph() -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("assets")).unwrap();
    fs::create_dir_all(tmp.path().join("pages/mybooks/coolbook/assets")).unwrap();
    fs::write(tmp.path().join("assets/shared.png"), b"shared").unwrap();
    fs::write(
        tmp.path().join("pages/mybooks/coolbook/assets/local.png"),
        b"local",
    )
    .unwrap();
    tmp
}

#[test]
fn finds_media_stored_beside_the_page() {
    let tmp = graph();
    let found = resolve_asset_path(tmp.path(), "pages/mybooks/coolbook/assets/local.png").unwrap();
    assert_eq!(fs::read(found).unwrap(), b"local");
}

#[test]
fn falls_back_to_the_shared_folder_for_older_notes() {
    // A note written before co-location says `assets/shared.png`. Rendered from
    // a nested page it now resolves page-relative first, and that file does not
    // exist — it must still find the shared one rather than render as broken.
    let tmp = graph();
    let found = resolve_asset_path(tmp.path(), "pages/mybooks/coolbook/assets/shared.png").unwrap();
    assert_eq!(fs::read(found).unwrap(), b"shared");
}

#[test]
fn page_local_media_wins_over_a_same_named_shared_file() {
    let tmp = graph();
    fs::write(
        tmp.path().join("pages/mybooks/coolbook/assets/shared.png"),
        b"page-local",
    )
    .unwrap();
    let found = resolve_asset_path(tmp.path(), "pages/mybooks/coolbook/assets/shared.png").unwrap();
    assert_eq!(fs::read(found).unwrap(), b"page-local");
}

#[test]
fn journal_media_resolves_too() {
    let tmp = graph();
    fs::create_dir_all(tmp.path().join("journals/assets")).unwrap();
    fs::write(tmp.path().join("journals/assets/today.png"), b"journal").unwrap();
    let found = resolve_asset_path(tmp.path(), "journals/assets/today.png").unwrap();
    assert_eq!(fs::read(found).unwrap(), b"journal");
}

#[test]
fn refuses_to_escape_the_graph_root() {
    let tmp = graph();
    for escape in [
        "../../etc/passwd",
        "pages/../../etc/passwd",
        "/etc/passwd",
        "",
    ] {
        assert!(
            resolve_asset_path(tmp.path(), escape).is_none(),
            "must refuse {escape:?}"
        );
    }
}

#[test]
fn the_fallback_cannot_be_used_to_escape_either() {
    // The `assets/` fallback re-resolves from the root, so it must not become a
    // way to reach a file the direct candidate would have been denied.
    let tmp = graph();
    assert!(resolve_asset_path(tmp.path(), "pages/assets/../../../etc/passwd").is_none());
}

#[test]
fn a_directory_is_not_an_asset() {
    let tmp = graph();
    assert!(resolve_asset_path(tmp.path(), "assets").is_none());
}

#[test]
fn missing_media_still_reports_missing() {
    let tmp = graph();
    assert!(resolve_asset_path(tmp.path(), "assets/nope.png").is_none());
}

// ─── Where new media is allowed to be written ────────────────────────────────

mod write_location {
    use grafium_core::graph::page_asset_dir;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn uses_the_folder_the_page_lives_in() {
        let tmp = super::graph();
        let dir = page_asset_dir(tmp.path(), "pages/mybooks/coolbook/toc.md").unwrap();
        assert_eq!(dir, tmp.path().join("pages/mybooks/coolbook").canonicalize().unwrap());
    }

    #[test]
    fn accepts_windows_separators() {
        let tmp = super::graph();
        let dir = page_asset_dir(tmp.path(), r"pages\mybooks\coolbook\toc.md").unwrap();
        assert_eq!(dir, tmp.path().join("pages/mybooks/coolbook").canonicalize().unwrap());
    }

    #[test]
    fn refuses_a_path_that_climbs_out_of_the_graph() {
        // A page titled `../../outside/note` yields exactly this stored path.
        // Trusting it would write downloaded media outside the graph.
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("graph/pages")).unwrap();
        fs::create_dir_all(tmp.path().join("outside")).unwrap();
        let root = tmp.path().join("graph");
        assert!(page_asset_dir(&root, "pages/../../outside/note.md").is_none());
    }

    #[test]
    fn refuses_an_absolute_path() {
        // `Path::join` discards the root when given an absolute path, which
        // would send media somewhere else on the filesystem entirely.
        let tmp = super::graph();
        assert!(page_asset_dir(tmp.path(), "/etc/cron.d/note.md").is_none());
    }

    #[test]
    fn refuses_a_top_level_file_with_no_directory() {
        let tmp = super::graph();
        assert!(page_asset_dir(tmp.path(), "note.md").is_none());
        assert!(page_asset_dir(tmp.path(), "").is_none());
    }

    #[test]
    fn refuses_a_directory_that_does_not_exist() {
        let tmp = super::graph();
        assert!(page_asset_dir(tmp.path(), "pages/never/created/note.md").is_none());
    }
}

// ─── Finding media for maintenance ───────────────────────────────────────────

mod collecting {
    use grafium_core::graph::collect_asset_files;
    use std::fs;

    #[test]
    fn finds_both_shared_and_page_local_media() {
        let tmp = super::graph();
        let found = collect_asset_files(tmp.path());
        assert!(found.contains(&"assets/shared.png".to_string()));
        assert!(found.contains(&"pages/mybooks/coolbook/assets/local.png".to_string()));
    }

    #[test]
    fn ignores_notes_and_anything_outside_an_assets_folder() {
        let tmp = super::graph();
        fs::write(tmp.path().join("pages/mybooks/coolbook/toc.md"), b"# toc").unwrap();
        let found = collect_asset_files(tmp.path());
        assert!(
            !found.iter().any(|f| f.ends_with(".md")),
            "notes are not media: {found:?}"
        );
    }

    #[test]
    fn ignores_grafium_internals() {
        // These are offered to the user as deletable, so a database file
        // showing up in the list would be genuinely dangerous.
        let tmp = super::graph();
        fs::create_dir_all(tmp.path().join(".grafium/assets")).unwrap();
        fs::write(tmp.path().join(".grafium/assets/index.db"), b"db").unwrap();
        let found = collect_asset_files(tmp.path());
        assert!(!found.iter().any(|f| f.contains(".grafium")), "{found:?}");
    }

    #[test]
    fn returns_an_empty_list_for_a_graph_with_no_media() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(collect_asset_files(tmp.path()).is_empty());
    }
}

// ─── Nested media ────────────────────────────────────────────────────────────

mod nested {
    use grafium_core::graph::collect_asset_files;
    use std::fs;

    /// Anki imports nest media as `assets/anki/<deck>/x.mp3`. These must be
    /// found, or a graph looks clean while nested media accumulates unseen.
    #[test]
    fn collects_media_nested_inside_assets() {
        let tmp = super::graph();
        fs::create_dir_all(tmp.path().join("assets/anki/gre")).unwrap();
        fs::write(tmp.path().join("assets/anki/gre/word.mp3"), b"audio").unwrap();
        let found = collect_asset_files(tmp.path());
        assert!(
            found.contains(&"assets/anki/gre/word.mp3".to_string()),
            "{found:?}"
        );
    }
}
