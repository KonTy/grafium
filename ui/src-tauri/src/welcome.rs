//! Embedded, fresh-install-only sample graph. Never refresh an existing tutorial.

use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path};

const SEED_VERSION: u8 = 12;

struct Resource {
    path: &'static str,
    content: &'static str,
}

macro_rules! resource {
    ($path:literal) => {
        Resource {
            path: $path,
            content: include_str!(concat!("../resources/welcome/", $path)),
        }
    };
}

const RESOURCES: &[Resource] = &[
    resource!("pages/Welcome To Grafium.md"),
    resource!("pages/Start With One Idea.md"),
    resource!("pages/Writing.md"),
    resource!("pages/Writing/Outline.md"),
    resource!("pages/Writing/Connections.md"),
    resource!("pages/Writing/Formatting.md"),
    resource!("pages/Writing/Tables.md"),
    resource!("pages/Writing/Find It Again.md"),
    resource!("pages/Projects.md"),
    resource!("pages/Projects/Observatory Night.md"),
    resource!("pages/Projects/Observatory Night/Plan.md"),
    resource!("pages/Projects/Observatory Night/Log.md"),
    resource!("pages/Projects/Tasks And Dates.md"),
    resource!("pages/Projects/Task Dashboard.md"),
    resource!("pages/Learning.md"),
    resource!("pages/Learning/Reading Notes.md"),
    resource!("pages/Learning/Recall.md"),
    resource!("pages/Learning/Flashcards.md"),
    resource!("pages/Learning/Reading Shelf.md"),
    resource!("pages/Space.md"),
    resource!("pages/Space/Orbits.md"),
    resource!("pages/Space/Orbits/Gravity.md"),
    resource!("pages/Space/Light.md"),
    resource!("pages/Space/Telescopes.md"),
    resource!("pages/Explore Your Graph.md"),
    resource!("pages/Explore Your Graph/Space Flight.md"),
    resource!("pages/Journal And Calendar.md"),
    resource!("pages/Your Workspace.md"),
    resource!("pages/Your Files.md"),
    resource!("pages/Create Your Own Graph.md"),
    resource!("pages/Chat And Research.md"),
    resource!("pages/Imports And Media.md"),
    resource!("pages/On Android.md"),
    resource!("pages/personal.md"),
    resource!("pages/personal/diary.md"),
    resource!("journals/1969_07_20.md"),
    resource!("assets/welcome/orbit.svg"),
];

fn inspect(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot inspect '{}': {error}", path.display())),
    }
}

fn check_directory(path: &Path) -> Result<(), String> {
    if let Some(metadata) = inspect(path)? {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!(
                "Welcome graph directory '{}' is not a regular directory",
                path.display()
            ));
        }
    }
    Ok(())
}

fn contains_markdown(root: &Path) -> Result<bool, String> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        if inspect(&directory)?.is_none() {
            continue;
        }
        check_directory(&directory)?;
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("Cannot read '{}': {error}", directory.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("Cannot read '{}': {error}", directory.display()))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|error| format!("Cannot inspect '{}': {error}", path.display()))?;
            if kind.is_symlink() {
                return Err(format!(
                    "Refusing to seed through a symbolic link: '{}'",
                    path.display()
                ));
            }
            if kind.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn preflight_destination(root: &Path, relative: &Path) -> Result<(), String> {
    let mut path = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "Invalid Welcome resource path: {}",
                relative.display()
            ));
        }
        path.push(component);
        if components.peek().is_some() {
            check_directory(&path)?;
        } else if inspect(&path)?.is_some() {
            return Err(format!(
                "Refusing to replace existing Welcome graph file '{}'",
                path.display()
            ));
        }
    }
    Ok(())
}

fn write_new_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create '{}': {error}", parent.display()))?;
    }
    // create_new also protects a destination that appeared after preflight.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Cannot create '{}': {error}", path.display()))?;
    file.write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Cannot finish '{}': {error}", path.display()))
}

/// Called only for the built-in default graph. Existing notes or any previous
/// seed marker are a permanent opt-out, even after the user removes sample pages.
pub(crate) fn seed_tutorial_graph(graph_root: &Path, metadata_dir: &str) -> Result<bool, String> {
    let mut components = Path::new(metadata_dir).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err("Welcome metadata directory must be a single directory name".to_string());
    }
    check_directory(graph_root)?;
    let metadata_path = graph_root.join(metadata_dir);
    check_directory(&metadata_path)?;
    for version in 1..=SEED_VERSION {
        if inspect(&metadata_path.join(format!("tutorial-seeded-v{version}")))?.is_some() {
            return Ok(false);
        }
    }
    if contains_markdown(graph_root)? {
        return Ok(false);
    }

    // Inspect the complete destination set before creating even the first page.
    for directory in ["pages", "journals", "knowledge"] {
        check_directory(&graph_root.join(directory))?;
    }
    for resource in RESOURCES {
        preflight_destination(graph_root, Path::new(resource.path))?;
    }
    let marker = Path::new(metadata_dir).join(format!("tutorial-seeded-v{SEED_VERSION}"));
    preflight_destination(graph_root, &marker)?;

    for directory in ["pages", "journals", "knowledge"] {
        let path = graph_root.join(directory);
        fs::create_dir_all(&path)
            .map_err(|error| format!("Cannot create '{}': {error}", path.display()))?;
    }
    for resource in RESOURCES {
        write_new_file(&graph_root.join(resource.path), resource.content)?;
    }
    write_new_file(&graph_root.join(marker), "seeded_v12\n")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafium_core::parser::links::{extract_links, ExtractedLink};
    use grafium_core::parser::parse_page;
    use grafium_core::Graph;
    use std::collections::{BTreeMap, HashSet};
    use tempfile::{Builder, TempDir};

    fn fixture() -> TempDir {
        Builder::new()
            .prefix(".welcome-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("test directory must stay inside the worktree")
    }

    fn seed(root: &Path) -> Result<bool, String> {
        seed_tutorial_graph(root, ".grafium")
    }

    fn snapshot(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(directory) = stack.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    result.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        result
    }

    #[test]
    fn fresh_seed_is_complete_and_idempotent() {
        let temp = fixture();
        assert_eq!(seed(temp.path()), Ok(true));
        for resource in RESOURCES {
            assert_eq!(
                fs::read_to_string(temp.path().join(resource.path)).unwrap(),
                resource.content
            );
        }
        let before = snapshot(temp.path());
        assert_eq!(seed(temp.path()), Ok(false));
        assert_eq!(snapshot(temp.path()), before);
    }

    #[test]
    fn creates_missing_graph_and_subdirectories() {
        let temp = fixture();
        let root = temp.path().join("new/default-graph");
        assert_eq!(seed(&root), Ok(true));
        for directory in [
            "pages",
            "journals",
            "knowledge",
            ".grafium",
            "assets/welcome",
        ] {
            assert!(root.join(directory).is_dir());
        }
    }

    #[test]
    fn preserves_existing_notes_anywhere_in_graph() {
        for relative in [
            "pages/Welcome To Grafium.md",
            "pages/nested/Edited.MD",
            "journals/2020_01_01.md",
            "knowledge/Custom.md",
            "Other note.md",
        ] {
            let temp = fixture();
            let path = temp.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "An irreplaceable edited note").unwrap();
            let before = snapshot(temp.path());
            assert_eq!(seed(temp.path()), Ok(false), "{relative}");
            assert_eq!(snapshot(temp.path()), before);
        }
    }

    #[test]
    fn every_legacy_marker_opts_out_with_or_without_notes() {
        for version in 1..=SEED_VERSION {
            let temp = fixture();
            fs::create_dir(temp.path().join(".grafium")).unwrap();
            fs::write(
                temp.path()
                    .join(format!(".grafium/tutorial-seeded-v{version}")),
                "original marker",
            )
            .unwrap();
            let before = snapshot(temp.path());
            assert_eq!(seed(temp.path()), Ok(false), "marker v{version}");
            assert_eq!(snapshot(temp.path()), before);

            fs::create_dir(temp.path().join("pages")).unwrap();
            fs::write(
                temp.path().join("pages/Welcome To Grafium.md"),
                "My edited older tutorial",
            )
            .unwrap();
            let before = snapshot(temp.path());
            assert_eq!(seed(temp.path()), Ok(false));
            assert_eq!(snapshot(temp.path()), before);
        }
    }

    #[test]
    fn current_marker_prevents_restoring_removed_or_rearranged_samples() {
        let temp = fixture();
        assert_eq!(seed(temp.path()), Ok(true));
        fs::rename(
            temp.path().join("pages/Space.md"),
            temp.path().join("pages/My subject.md"),
        )
        .unwrap();
        fs::remove_file(temp.path().join("pages/Welcome To Grafium.md")).unwrap();
        let before = snapshot(temp.path());
        assert_eq!(seed(temp.path()), Ok(false));
        assert_eq!(snapshot(temp.path()), before);
    }

    #[test]
    fn preflights_assets_and_parent_conflicts_before_writing() {
        for relative in [
            "assets/welcome/orbit.svg",
            "assets/welcome",
            "pages/Space",
            "journals",
            ".grafium",
        ] {
            let temp = fixture();
            let path = temp.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "existing content").unwrap();
            let before = snapshot(temp.path());
            assert!(seed(temp.path()).is_err(), "{relative}");
            assert_eq!(snapshot(temp.path()), before);
        }
    }

    #[test]
    fn exclusive_creation_preserves_a_file_that_appeared_after_preflight() {
        let temp = fixture();
        let relative = Path::new("assets/welcome/orbit.svg");
        preflight_destination(temp.path(), relative).unwrap();
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "arrived after preflight").unwrap();
        assert!(write_new_file(&path, "sample content").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "arrived after preflight");
    }

    #[test]
    fn invalid_root_and_metadata_are_explicit_errors() {
        let temp = fixture();
        let root = temp.path().join("not-a-directory");
        fs::write(&root, "keep me").unwrap();
        assert!(seed(&root).is_err());
        for metadata in ["", "..", "../outside", "/outside", "nested/metadata"] {
            assert!(seed_tutorial_graph(temp.path(), metadata).is_err());
        }
        assert_eq!(fs::read_to_string(root).unwrap(), "keep me");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_or_dangling_destinations_are_never_followed() {
        use std::os::unix::fs::symlink;
        for relative in ["pages", "assets/welcome/orbit.svg", ".grafium"] {
            let temp = fixture();
            let root = temp.path().join("graph");
            let outside = temp.path().join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::write(outside.join("sentinel"), "untouched").unwrap();
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(&outside, &path).unwrap();
            assert!(seed(&root).is_err(), "{relative}");
            assert_eq!(snapshot(&outside).len(), 1);
            assert_eq!(
                fs::read_to_string(outside.join("sentinel")).unwrap(),
                "untouched"
            );
            fs::remove_file(&path).unwrap();
            symlink(outside.join("missing"), path).unwrap();
            assert!(seed(&root).is_err());
            assert!(!root.join("pages/Welcome To Grafium.md").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_does_not_look_empty() {
        use std::os::unix::fs::PermissionsExt;
        let temp = fixture();
        let directory = temp.path().join("pages");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();
        // Privileged test runners may still read mode-000 directories.
        let cannot_read = fs::read_dir(&directory).is_err();
        let result = if cannot_read {
            Some(seed(temp.path()))
        } else {
            None
        };
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        if let Some(result) = result {
            assert!(result.is_err());
        }
        assert!(!temp.path().join(".grafium/tutorial-seeded-v12").exists());
        assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
    }

    #[test]
    fn reindex_resolves_all_pages_links_namespaces_cards_assets_and_collection() {
        let temp = fixture();
        let graph = Graph::open(temp.path()).unwrap();
        assert_eq!(seed(temp.path()), Ok(true));
        graph.reindex_all().unwrap();

        let markdown: Vec<_> = RESOURCES
            .iter()
            .filter(|r| r.path.ends_with(".md"))
            .collect();
        assert_eq!(markdown.len(), 36);
        let titles: HashSet<String> = markdown
            .iter()
            .map(|resource| {
                if resource.path.starts_with("journals/") {
                    "1969-07-20".to_string()
                } else {
                    resource
                        .path
                        .strip_prefix("pages/")
                        .unwrap()
                        .trim_end_matches(".md")
                        .to_string()
                }
            })
            .collect();
        let indexed = graph.db.run_raw_select("SELECT title FROM pages").unwrap();
        assert_eq!(indexed.len(), titles.len(), "No implicit placeholder pages");
        for title in &titles {
            let page = graph.db.get_page_by_title(title).unwrap();
            let file = page.file_path.expect("Every page must have an actual file");
            assert!(temp.path().join(file).is_file(), "{title}");
        }
        assert!(graph.db.get_page_by_title("1969-07-20").unwrap().is_journal);
        for resource in markdown {
            for link in extract_links(resource.content) {
                match link {
                    ExtractedLink::Page(title) | ExtractedLink::Tag(title) => {
                        assert!(titles.contains(&title), "{} -> {title}", resource.path);
                    }
                    ExtractedLink::BlockRef(id) => {
                        assert!(graph.db.get_block(&id).is_ok(), "{} -> {id}", resource.path);
                    }
                }
            }
            for reference in resource.content.split("](").skip(1) {
                let target = reference.split(')').next().unwrap();
                // The Markdown renderer treats legacy ../assets as graph-relative.
                let relative = target.strip_prefix("../").unwrap_or(target);
                let asset = grafium_core::graph::resolve_asset_path(temp.path(), relative)
                    .expect("Demo media must be local graph assets");
                assert!(asset.is_file(), "{} -> {target}", resource.path);
            }
        }
        let gravity = graph.db.get_page_by_title("Space/Orbits/Gravity").unwrap();
        assert_eq!(
            gravity.file_path.as_deref(),
            Some("pages/Space/Orbits/Gravity.md")
        );
        let cards = graph.db.list_flashcards(100, 0).unwrap();
        assert_eq!(cards.len(), 4, "Only the four intended examples are cards");
        assert!(cards
            .iter()
            .any(|card| card.back.contains("assets/welcome/orbit.svg")));
        assert_eq!(
            graph
                .db
                .list_flashcards_due(Some("Space"), 100)
                .unwrap()
                .len(),
            4
        );
        let collections = graph.db.list_collections().unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].0.title, "Learning/Reading Shelf");
        assert_eq!(collections[0].1, 4);
        let nested = graph.db.run_raw_select(
            "SELECT b.id FROM blocks b JOIN pages p ON p.id = b.page_id WHERE p.title = 'Projects/Observatory Night/Plan' AND b.parent_id IS NOT NULL",
        ).unwrap();
        assert!(
            !nested.is_empty(),
            "Nested example must index as child blocks"
        );
    }

    #[test]
    fn seeded_task_query_is_live_and_read_only() {
        let temp = fixture();
        let graph = Graph::open(temp.path()).unwrap();
        assert_eq!(seed(temp.path()), Ok(true));
        graph.reindex_all().unwrap();
        let content = RESOURCES
            .iter()
            .find(|resource| resource.path == "pages/Projects/Task Dashboard.md")
            .unwrap()
            .content;
        let parsed = parse_page(content, "Task Dashboard");
        let query = parsed
            .blocks
            .iter()
            .find(|block| block.content.starts_with("{{query "))
            .expect("Dashboard must be an actual query block");
        let sql = query
            .content
            .strip_prefix("{{query ")
            .unwrap()
            .strip_suffix("}}")
            .unwrap();
        let rows = graph.db.run_raw_select(sql).unwrap();
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert!(row
                .iter()
                .any(|(key, value)| key == "page" && value == "Projects/Observatory Night/Plan"));
            assert!(row
                .iter()
                .any(|(key, value)| key == "scheduled" && value.is_null()));
            assert!(row
                .iter()
                .any(|(key, value)| key == "deadline" && value.is_null()));
        }
        let block_id = rows[0]
            .iter()
            .find(|(key, _)| key == "_block_id")
            .unwrap()
            .1
            .as_str()
            .unwrap();
        graph
            .set_task_date(block_id, "scheduled", Some("1969-07-20"))
            .unwrap();
        assert!(graph.db.run_raw_select(sql).unwrap().iter().any(|row| {
            row.iter()
                .any(|(key, value)| key == "scheduled" && value == "1969-07-20")
        }));
        graph.set_task_date(block_id, "scheduled", None).unwrap();
        graph
            .update_task_state(block_id, &grafium_core::models::TaskState::Done)
            .unwrap();
        assert_eq!(graph.db.run_raw_select(sql).unwrap().len(), 2);
        for mutation in [
            "DELETE FROM tasks",
            "SELECT * FROM tasks; DELETE FROM tasks",
            "PRAGMA writable_schema = ON",
        ] {
            assert!(graph.db.run_raw_select(mutation).is_err());
        }
        assert_eq!(graph.db.run_raw_select(sql).unwrap().len(), 2);
    }
}
