use super::*;

fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Graph, BookInfo) {
    let source = tempfile::tempdir_in(".").unwrap();
    let root = tempfile::tempdir_in(".").unwrap();
    fs::write(source.path().join("Synthetic.fb2"),
        "<FictionBook><description><book-title>Synthetic</book-title></description><body><section><p>TODO author's cobalt prose #science</p><p>Question :: Answer</p><p>[[Astronomy]] describes the stars.</p></section></body></FictionBook>").unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let info = graph
        .import_original_book(&source.path().join("Synthetic.fb2"))
        .unwrap();
    (source, root, graph, info)
}

fn legacy_note(graph: &Graph, info: &BookInfo, id: &str, body: &str) -> BookNote {
    let path = graph.note_path(&info.id, id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let now = Utc::now().to_rfc3339();
    let metadata = NoteMetadata {
        version: 1, id: id.into(), book_id: info.id.clone(),
        source_sha256: info.source_sha256.clone(), locator: None,
        quote: String::new(), created_at: now.clone(), updated_at: now,
    };
    fs::write(&path, format!("book-note:: {}\n\n{body}", serde_json::to_string(&metadata).unwrap())).unwrap();
    graph.index_file(&path).unwrap();
    graph.read_book_note(&info.id, id).unwrap()
}

#[test]
fn original_text_is_searchable_without_markdown_tasks_or_flashcards() {
    let (source, root, graph, info) = fixture();
    assert_eq!(
        graph.book_read_bytes(&info.id).unwrap(),
        fs::read(source.path().join("Synthetic.fb2")).unwrap()
    );
    assert!(root.path().join(&info.file_path).exists());
    assert_eq!(graph.markdown_file_count().unwrap(), 0);
    assert!(!graph.db.search_fts("cobalt", 10).unwrap().is_empty());
    let conn = graph.db.conn().unwrap();
    for table in ["tasks", "flashcards"] {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert!(graph.db.find_page_by_title("Astronomy").unwrap().is_some());
    let page = graph.db.get_page_by_id(&info.page_id).unwrap();
    assert_eq!(page.properties["book-source-sha256"], info.source_sha256);
}

#[test]
fn reimport_never_substitutes_an_externally_changed_library_copy() {
    let (source, root, graph, info) = fixture();
    let replacement = b"<FictionBook><body><section><p>A different book now occupies this source.</p></section></body></FictionBook>";
    let path = root.path().join(&info.file_path);
    fs::write(&path, replacement).unwrap();
    graph.index_original_book(&info.id).unwrap();
    let error = graph.import_original_book(&source.path().join("Synthetic.fb2")).unwrap_err();
    assert!(error.to_string().contains("changed externally"));
    assert_eq!(fs::read(path).unwrap(), replacement);
    assert_ne!(graph.book_info(&info.id).unwrap().source_sha256, info.source_sha256);
}

#[test]
fn explicit_reimport_restores_a_missing_copy_without_replacing_its_notes() {
    let (source, root, graph, info) = fixture();
    let note = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Keep this annotation when restoring the source.", "", None, &info.source_sha256).unwrap();
    fs::remove_file(root.path().join(&info.file_path)).unwrap();
    graph.reconcile_original_books().unwrap();
    let restored = graph.import_original_book(&source.path().join("Synthetic.fb2")).unwrap();
    assert_eq!(restored.id, info.id);
    assert_eq!(restored.source_sha256, info.source_sha256);
    let notes = graph.book_notes_list(&info.id).unwrap();
    assert_eq!(notes[0].revision, note.revision);
    assert_eq!(notes[0].status, "attached");
}

#[test]
fn source_and_annotation_titles_reclaim_link_placeholders_after_full_reindex() {
    let (_source, _root, graph, info) = fixture();
    let note_id = Uuid::new_v4().to_string();
    let note_title = format!("Reading Notes/Books/{}/{note_id}", info.id);
    let links = graph
        .create_page_with_content(
            "Links",
            false,
            &format!("- [[{}]] [[{note_title}]]", info.title),
        )
        .unwrap();
    let note = graph
        .book_note_save(
            &info.id,
            &note_id,
            None,
            "My note",
            "",
            None,
            &info.source_sha256,
        )
        .unwrap();
    graph.reindex_all().unwrap();
    assert_eq!(graph.db.get_page_by_title(&info.title).unwrap().id, info.id);
    assert_eq!(
        graph.db.get_page_by_title(&note_title).unwrap().id,
        note.note_page_id
    );
    let links_page = graph.db.get_page_by_title(&links.title).unwrap();
    let targets: Vec<_> = graph
        .db
        .get_links_from_page(&links_page.id)
        .unwrap()
        .into_iter()
        .map(|link| link.to_page_id)
        .collect();
    assert!(targets.contains(&info.id));
    assert!(targets.contains(&note_id));
    let context = crate::knowledge::assistant_scope::assistant_context_info(
        &graph.db,
        &graph.root_dir,
        &info.id,
        None,
    )
    .unwrap();
    assert_eq!(context.book.unwrap().page_id, info.id);
}

#[test]
fn epub_original_uses_existing_extractor_and_rejects_unsafe_archives() {
    use std::io::Write;
    let source = tempfile::tempdir_in(".").unwrap();
    let root = tempfile::tempdir_in(".").unwrap();
    let epub = source.path().join("Synthetic.epub");
    let mut zip = zip::ZipWriter::new(fs::File::create(&epub).unwrap());
    for (name, text) in [
        (
            "META-INF/container.xml",
            r#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#,
        ),
        (
            "book.opf",
            r#"<package><metadata><dc:title>Synthetic EPUB</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
        ),
        (
            "chapter.xhtml",
            "<html><body><h1>Actual chapter</h1><p>Neodymium source evidence.</p></body></html>",
        ),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let info = graph.import_original_book(&epub).unwrap();
    assert!(!graph.db.search_fts("Neodymium", 10).unwrap().is_empty());
    assert_eq!(
        graph.book_read_bytes(&info.id).unwrap(),
        fs::read(&epub).unwrap()
    );
    let unsafe_epub = source.path().join("Unsafe.epub");
    let mut zip = zip::ZipWriter::new(fs::File::create(&unsafe_epub).unwrap());
    zip.start_file("../escape", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"unsafe").unwrap();
    zip.finish().unwrap();
    assert!(graph.import_original_book(&unsafe_epub).is_err());
}

#[test]
fn notes_preserve_external_body_metadata_and_validate_revisions() {
    let (_source, root, graph, info) = fixture();
    let note = legacy_note(&graph, &info, &Uuid::new_v4().to_string(), "First body");
    let path = root.path().join(&note.file_path);
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!(
            "custom:: retained\n{}",
            original.replace("First body", "External body")
        )
        .replace('\n', "\r\n"),
    )
    .unwrap();
    assert!(graph
        .book_note_save(
            &info.id,
            &note.id,
            Some(&note.revision),
            "Oops",
            "",
            None,
            &info.source_sha256
        )
        .is_err());
    let current = graph.book_notes_list(&info.id).unwrap().remove(0);
    assert_eq!(current.body, "External body");
    let updated = graph
        .book_note_save(
            &info.id,
            &note.id,
            Some(&current.revision),
            "Final body",
            "",
            None,
            &info.source_sha256,
        )
        .unwrap();
    assert_eq!(updated.body, "Final body");
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("custom:: retained"));
    assert!(graph.rename_page(&note.note_page_id, "Moved").is_err());
    assert!(graph
        .update_page_properties(&note.note_page_id, serde_json::json!({}))
        .is_err());
}

#[test]
fn original_text_blocks_are_bounded_even_for_multiline_prose() {
    let source = tempfile::tempdir_in(".").unwrap();
    let root = tempfile::tempdir_in(".").unwrap();
    let long = "Long author's prose\n".repeat(1000);
    fs::write(
        source.path().join("Long.fb2"),
        format!("<FictionBook><body><p>{long}</p></body></FictionBook>"),
    )
    .unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let info = graph
        .import_original_book(&source.path().join("Long.fb2"))
        .unwrap();
    assert!(graph
        .db
        .list_blocks_for_page(&info.id)
        .unwrap()
        .iter()
        .all(|block| block.content.chars().count() <= 2000));
}
#[test]
fn sidecar_index_failure_retries_and_delete_retains_tombstone() {
    let (_source, root, graph, info) = fixture();
    let id = Uuid::new_v4().to_string();
    graph.db.conn().unwrap().execute_batch(
        "CREATE TRIGGER fail_note_index BEFORE INSERT ON pages
         WHEN NEW.title LIKE 'Reading Notes/Books/%' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;"
    ).unwrap();
    assert!(graph
        .book_note_save(
            &info.id,
            &id,
            None,
            "Durable note",
            "",
            None,
            &info.source_sha256
        )
        .is_err());
    let path = root
        .path()
        .join(format!("books/{}/original.jsonld", info.id));
    assert!(path.exists());
    graph
        .db
        .conn()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_note_index")
        .unwrap();
    let note = graph
        .book_note_save(
            &info.id,
            &id,
            None,
            "Durable note",
            "",
            None,
            &info.source_sha256,
        )
        .unwrap();
    assert_eq!(note.note_page_id, id);
    graph
        .book_note_delete(&info.id, &id, &note.revision)
        .unwrap();
    assert!(path.exists());
    assert!(graph.book_notes_list(&info.id).unwrap().is_empty());
}

#[test]
fn corrupt_external_identity_invalidates_old_author_evidence() {
    let (_source, root, graph, info) = fixture();
    fs::write(
        root.path().join(format!("books/{}/book.json", info.id)),
        "{}",
    )
    .unwrap();
    assert!(graph.reconcile_original_books().is_err());
    assert!(graph.db.search_fts("cobalt", 10).unwrap().is_empty());
    assert!(graph.db.get_page_by_id(&info.id).is_err());
    assert!(root.path().join(info.file_path).exists());
}

#[test]
fn book_identity_and_block_ids_survive_reindex_restart_and_source_change() {
    let (source, root, graph, info) = fixture();
    let ids: Vec<_> = graph
        .db
        .list_blocks_for_page(&info.page_id)
        .unwrap()
        .into_iter()
        .map(|b| b.id)
        .collect();
    graph.reindex_all().unwrap();
    assert_eq!(
        ids,
        graph
            .db
            .list_blocks_for_page(&info.page_id)
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        graph
            .import_original_book(&source.path().join("Synthetic.fb2"))
            .unwrap()
            .id,
        info.id
    );
    drop(graph);
    let graph = Graph::open(root.path()).unwrap();
    assert!(graph.needs_startup_reindex().unwrap());
    graph.reconcile_files_from_disk().unwrap();
    assert_eq!(graph.book_open(&info.page_id).unwrap().id, info.id);
    fs::write(
        root.path().join(&info.file_path),
        "<FictionBook><body><p>New zirconium evidence.</p></body></FictionBook>",
    )
    .unwrap();
    graph.reconcile_files_from_disk().unwrap();
    let updated = graph.book_open(&info.page_id).unwrap();
    assert_ne!(updated.source_sha256, info.source_sha256);
    assert!(graph.db.search_fts("cobalt", 10).unwrap().is_empty());
    assert!(!graph.db.search_fts("zirconium", 10).unwrap().is_empty());
    let metadata = graph.read_book_metadata(&info.id).unwrap();
    assert_eq!(metadata.source_sha256, updated.source_sha256);
}

#[test]
fn all_original_edits_fail_before_changing_source_or_index() {
    let (_source, root, graph, info) = fixture();
    let before = fs::read(root.path().join(&info.file_path)).unwrap();
    let blocks = graph.db.list_blocks_for_page(&info.page_id).unwrap();
    let block = &blocks[0];
    let properties = graph.db.get_page_by_id(&info.page_id).unwrap().properties;
    assert!(graph.update_block(&block.id, "changed", None).is_err());
    assert!(graph
        .create_block(
            &info.page_id,
            None,
            0,
            "changed",
            BlockType::Text,
            serde_json::json!({})
        )
        .is_err());
    assert!(graph.delete_block(&block.id).is_err());
    assert!(graph
        .delete_blocks(&info.page_id, &[block.id.clone()])
        .is_err());
    assert!(graph.move_block(&block.id, None, 99).is_err());
    assert!(graph
        .reorder_blocks(&info.page_id, &[block.id.clone()])
        .is_err());
    assert!(graph.rename_page(&info.page_id, "Oops").is_err());
    assert!(graph.bulk_rename_pages("Books", "Oops", false).is_err());
    assert!(graph
        .update_page_properties(&info.page_id, serde_json::json!({"x":"y"}))
        .is_err());
    assert!(graph.append_content_to_page(&info.page_id, "oops").is_err());
    assert!(graph.insert_block_at_top(&info.page_id, "summary").is_err());
    assert!(graph.update_page_source(&info.page_id, "oops").is_err());
    assert!(graph
        .update_page_source_guarded(&info.page_id, "", "oops")
        .is_err());
    assert!(graph
        .set_task_date(&block.id, "scheduled", Some("2026-01-01"))
        .is_err());
    assert!(graph.cycle_task_state(&block.id).is_err());
    let other = graph.create_page("Other", false).unwrap();
    assert!(graph
        .create_blocks(
            &other.id,
            vec![BlockCreateSpec {
                id: Some(block.id.clone()),
                parent: BlockCreateParent::Root,
                order_index: 0,
                content: "stolen ID".into(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
            }]
        )
        .is_err());
    assert!(graph
        .create_block(
            &other.id,
            Some(&block.id),
            0,
            "child",
            BlockType::Text,
            serde_json::json!({})
        )
        .is_err());
    assert!(graph
        .reorder_blocks(&other.id, &[block.id.clone()])
        .is_err());
    let collision = root.path().join("pages/Collision.md");
    fs::write(&collision, format!("- changed\n  id:: {}\n", block.id)).unwrap();
    assert!(graph.index_file(&collision).is_err());
    assert!(graph.merge_page(&other.id, &info.page_id).is_err());
    assert!(graph.merge_page(&info.page_id, &other.id).is_err());
    assert_eq!(
        graph.db.get_page_by_id(&info.page_id).unwrap().properties,
        properties
    );
    assert_eq!(
        serde_json::to_value(graph.db.list_blocks_for_page(&info.page_id).unwrap()).unwrap(),
        serde_json::to_value(blocks).unwrap()
    );
    assert_eq!(fs::read(root.path().join(&info.file_path)).unwrap(), before);
    // Rewriting a linked normal page must not rewrite the author's words.
    let target = graph.db.get_page_by_title("Astronomy").unwrap();
    graph.rename_page(&target.id, "Astrophysics").unwrap();
    assert_eq!(graph.book_read_bytes(&info.id).unwrap(), before);
}

#[test]
fn annotations_are_portable_revision_checked_and_survive_original_deletion() {
    let (_source, root, graph, info) = fixture();
    let id = Uuid::new_v4().to_string();
    let note = graph
        .book_note_save(
            &info.id,
            &id,
            None,
            "My iridium observation [[Notebook]]",
            "author's cobalt",
            None,
            &info.source_sha256,
        )
        .unwrap();
    assert_eq!(note.status, "attached");
    assert_eq!(note.note_page_id, id);
    assert_eq!(
        graph
            .book_note_save(
                &info.id,
                &id,
                None,
                &note.body,
                &note.quote,
                None,
                &info.source_sha256
            )
            .unwrap()
            .revision,
        note.revision
    );
    assert!(graph
        .book_note_save(
            &info.id,
            &id,
            Some("old"),
            "oops",
            "",
            None,
            &info.source_sha256
        )
        .is_err());
    assert!(graph.book_note_delete(&info.id, &id, "old").is_err());
    graph.reindex_all().unwrap();
    let notes = graph.book_notes_list(&info.id).unwrap();
    assert_eq!(notes[0].body, note.body);
    assert_eq!(notes[0].note_page_id, id);
    assert!(graph
        .db
        .list_blocks_for_page(&info.page_id)
        .unwrap()
        .iter()
        .all(|b| !b.content.contains("iridium")));
    graph.delete_page(&info.page_id).unwrap();
    assert!(root.path().join(&note.file_path).exists());
    assert!(!root.path().join(&info.file_path).exists());
    assert_eq!(
        graph.book_notes_list(&info.id).unwrap()[0].status,
        "orphaned"
    );
    let edited = graph
        .book_note_save(
            &info.id,
            &id,
            Some(&note.revision),
            "Edited orphan",
            &note.quote,
            None,
            &info.source_sha256,
        )
        .unwrap();
    assert_eq!(edited.status, "orphaned");
    assert!(graph
        .book_note_delete(&info.id, &id, &note.revision)
        .is_err());
    graph
        .book_note_delete(&info.id, &id, &edited.revision)
        .unwrap();
    assert!(root.path().join(&note.file_path).exists());
    assert!(graph.book_notes_list(&info.id).unwrap().is_empty());
    assert!(graph.db.get_page_by_id(&id).is_err());
    assert!(graph
        .db
        .list_pending_reindex_due(0, 100)
        .unwrap()
        .iter()
        .any(|(id, _)| id == &info.page_id));
}

#[test]
fn external_annotation_directory_deletion_deindexes_without_touching_original() {
    let (_source, root, graph, info) = fixture();
    let note = graph
        .book_note_save(
            &info.id,
            &Uuid::new_v4().to_string(),
            None,
            "Erbium personal note",
            "",
            None,
            &info.source_sha256,
        )
        .unwrap();
    let path = root.path().join(&note.file_path);
    fs::remove_file(&path).unwrap();
    graph.reconcile_book_notes().unwrap();
    assert!(graph.db.get_page_by_id(&note.note_page_id).is_err());
    assert!(graph.db.search_fts("erbium", 10).unwrap().is_empty());
    assert!(graph.book_read_bytes(&info.id).is_ok());
}

#[test]
fn external_source_deletion_deindexes_and_queues_vector_removal() {
    let (_source, root, graph, info) = fixture();
    graph
        .db
        .conn()
        .unwrap()
        .execute("DELETE FROM pending_reindex", [])
        .unwrap();
    fs::remove_file(root.path().join(&info.file_path)).unwrap();
    graph.reconcile_files_from_disk().unwrap();
    assert!(graph.db.get_page_by_id(&info.page_id).is_err());
    assert!(graph.db.search_fts("cobalt", 10).unwrap().is_empty());
    assert!(graph
        .db
        .list_pending_reindex_due(0, 100)
        .unwrap()
        .iter()
        .any(|(id, _)| id == &info.page_id));
}

#[test]
fn locations_validate_and_position_does_not_change_original() {
    let (_source, _root, graph, info) = fixture();
    let location = BookLocation::Epub {
        cfi: "epubcfi(/6/2!/4/2:0)".into(),
        renderer_version: "foliate-js".into(),
    };
    let original = graph.book_read_bytes(&info.id).unwrap();
    graph
        .book_save_position(&info.id, &info.source_sha256, location.clone())
        .unwrap();
    assert_eq!(
        graph.book_open(&info.page_id).unwrap().reading_location,
        Some(location.clone())
    );
    assert!(graph
        .book_save_position(&info.id, "stale", location)
        .is_err());
    assert_eq!(graph.book_read_bytes(&info.id).unwrap(), original);
    assert!(BookLocation::Pdf {
        page: 0,
        rects: None
    }
    .validate()
    .is_err());
    assert!(BookLocation::Pdf {
        page: 1,
        rects: Some(vec![BookRect {
            x: f64::NAN,
            y: 0.0,
            width: 0.5,
            height: 0.5
        }])
    }
    .validate()
    .is_err());
    assert!(BookLocation::Pdf {
        page: 1,
        rects: Some(vec![BookRect {
            x: 0.8,
            y: 0.0,
            width: 0.5,
            height: 0.5
        }])
    }
    .validate()
    .is_err());
    assert!(graph.book_read_bytes("../outside").is_err());
    let wire = serde_json::to_value(&info).unwrap();
    assert!(wire["pageId"].is_string());
    assert!(wire["sourceSha256"].is_string());
}

#[test]
fn unavailable_extraction_preserves_original_and_exposes_warning() {
    let source = tempfile::tempdir_in(".").unwrap();
    let root = tempfile::tempdir_in(".").unwrap();
    let bytes = b"%PDF-1.4\ninvalid synthetic test";
    fs::write(source.path().join("broken.pdf"), bytes).unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let book = graph
        .import_original_book(&source.path().join("broken.pdf"))
        .unwrap();
    assert!(book.indexing_warning.unwrap().contains("not indexed"));
    assert_eq!(graph.book_read_bytes(&book.id).unwrap(), bytes);
    assert!(graph
        .db
        .list_blocks_for_page(&book.page_id)
        .unwrap()
        .is_empty());
}

#[test]
fn filesystem_sync_contains_only_portable_original_book_files() {
    use crate::sync::{filesystem::FilesystemBackend, SyncBackend};
    let (_source, root, graph, info) = fixture();
    let directory = root.path().join("books").join(&info.id);
    fs::create_dir(directory.join(".extract-cache")).unwrap();
    fs::write(directory.join(".extract-cache/converted.epub"), b"cache").unwrap();
    fs::write(directory.join("index-cache.txt"), b"cache").unwrap();
    let note = graph
        .book_note_save(
            &info.id,
            &Uuid::new_v4().to_string(),
            None,
            "Note",
            "",
            None,
            &info.source_sha256,
        )
        .unwrap();
    let backend = FilesystemBackend::new(root.path().to_path_buf(), "fixture".into());
    let paths = backend
        .list_files()
        .unwrap()
        .into_iter()
        .map(|f| f.rel_path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&info.file_path));
    assert!(paths.contains(&format!("books/{}/book.json", info.id)));
    assert!(paths.contains(&note.file_path));
    assert!(!paths.iter().any(|p| p.contains("cache")));
}

#[cfg(unix)]
#[test]
fn symlink_originals_and_note_directories_are_rejected() {
    use std::os::unix::fs::symlink;
    let (source, root, graph, info) = fixture();
    let external = source.path().join("Synthetic.fb2").canonicalize().unwrap();
    fs::remove_file(root.path().join(&info.file_path)).unwrap();
    symlink(&external, root.path().join(&info.file_path)).unwrap();
    assert!(graph.book_read_bytes(&info.id).is_err());
    assert!(graph.index_original_book(&info.id).is_err());
    fs::create_dir_all(root.path().join("pages/Reading Notes")).unwrap();
    symlink(
        source.path().canonicalize().unwrap(),
        root.path().join("pages/Reading Notes/Books"),
    )
    .unwrap();
    assert!(graph.book_notes_list(&info.id).is_err());
}

#[test]
fn adjacent_sidecar_is_authoritative_searchable_read_only_and_portable() {
    let (_source, root, graph, info) = fixture();
    let original = graph.book_read_bytes(&info.id).unwrap();
    let first = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Gadolinium [[Notebook]]", "cobalt", None, &info.source_sha256).unwrap();
    let second = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Europium", "", None, &info.source_sha256).unwrap();
    assert_eq!(first.file_path, format!("books/{}/original.jsonld", info.id));
    assert_eq!(first.file_path, second.file_path);
    assert!(!root.path().join("pages/Reading Notes/Books").exists());
    assert!(first.conflicts.is_empty());
    let indexed = graph.db.get_page_by_id(&first.id).unwrap();
    assert_eq!(indexed.properties["book-note-id"], first.id);
    assert_eq!(indexed.properties["book-note-book-page-id"], info.id);
    assert!(!graph.db.search_fts("Gadolinium", 10).unwrap().is_empty());
    assert!(!graph.db.get_links_from_page(&first.id).unwrap().is_empty());
    let block = graph.db.list_blocks_for_page(&first.id).unwrap().remove(0);
    assert!(graph.update_block(&block.id, "overwrite", None).is_err());
    assert!(graph.update_page_source(&first.id, "overwrite").is_err());
    assert!(graph.delete_page(&first.id).is_err());
    let edited = graph.book_note_save(&info.id, &first.id, Some(&first.revision),
        "Samarium", &first.quote, None, &info.source_sha256).unwrap();
    graph.book_note_delete(&info.id, &second.id, &second.revision).unwrap();
    assert_eq!(graph.book_read_bytes(&info.id).unwrap(), original);
    let sidecar = root.path().join(&first.file_path);
    let bytes = fs::read(&sidecar).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["type"], "AnnotationPage");
    assert_eq!(json["items"].as_array().unwrap().len(), 4);
    assert!(json["items"].as_array().unwrap().iter().any(|r| r["grafium:deleted"] == true));
    assert_eq!(merge_annotation_sidecars(&bytes,&bytes).unwrap(), bytes);
    validate_annotation_sidecar(&first.file_path,&bytes).unwrap();
    assert!(!is_annotation_sidecar("books/../original.jsonld"));
    assert!(!is_annotation_sidecar(&format!("books/{}/original.annotations.jsonld",info.id)));
    assert!(!is_annotation_sidecar(&format!("books/{}/original.jsonld.conflict_123",info.id)));

    // A fresh index needs neither the old database nor companion Markdown.
    let copy = tempfile::tempdir_in(".").unwrap();
    let folder = copy.path().join(format!("books/{}",info.id));
    fs::create_dir_all(&folder).unwrap();
    for name in ["original.fb2","book.json","original.jsonld"] {
        fs::copy(sidecar.parent().unwrap().join(name),folder.join(name)).unwrap();
    }
    let copied = Graph::open(copy.path()).unwrap();
    copied.reindex_all().unwrap();
    let notes = copied.book_notes_list(&info.id).unwrap();
    assert_eq!(notes.len(),1);
    assert_eq!(notes[0].body,"Samarium");
    assert_eq!(notes[0].revision,edited.revision);
    assert!(!copied.db.search_fts("Samarium",10).unwrap().is_empty());
    copied.delete_page(&info.id).unwrap();
    copied.reindex_all().unwrap();
    assert_eq!(copied.book_notes_list(&info.id).unwrap()[0].status,"orphaned");
}

#[test]
fn sidecar_concurrent_edits_deletes_and_resolutions_preserve_every_head() {
    let (_source, root, graph, info) = fixture();
    let note = graph.book_note_save(&info.id,&Uuid::new_v4().to_string(),None,
        "Initial","",None,&info.source_sha256).unwrap();
    let path = root.path().join(&note.file_path);
    let base = fs::read(&path).unwrap();
    let left_note = graph.book_note_save(&info.id,&note.id,Some(&note.revision),
        "Left","",None,&info.source_sha256).unwrap();
    let left = fs::read(&path).unwrap();
    fs::write(&path,&base).unwrap();
    graph.book_note_save(&info.id,&note.id,Some(&note.revision),
        "Right","",None,&info.source_sha256).unwrap();
    let right = fs::read(&path).unwrap();
    let merged = merge_annotation_sidecars(&left,&right).unwrap();
    assert_eq!(merge_annotation_sidecars(&right,&left).unwrap(),merged);
    assert_eq!(merge_annotation_sidecars(&merged,&left).unwrap(),merged);
    fs::write(&path,&merged).unwrap();
    let conflict = graph.book_notes_list(&info.id).unwrap().remove(0);
    assert_eq!(conflict.status,"conflicted");
    assert!(conflict.body.is_empty());
    assert_eq!(conflict.conflicts.len(),2);
    assert_eq!(annotation_conflict_count(&merged).unwrap(),1);
    assert!(graph.book_note_save(&info.id,&note.id,Some(&conflict.revision),
        "Implicit winner","",None,&info.source_sha256).is_err());
    assert!(graph.book_note_delete(&info.id,&note.id,&conflict.revision).is_err());
    assert!(graph.book_note_resolve(&info.id,&note.id,&left_note.revision,
        "Stale","",None,&info.source_sha256,false).is_err());

    // Third offline writer deletes the common ancestor.
    fs::write(&path,&base).unwrap();
    graph.book_note_delete(&info.id,&note.id,&note.revision).unwrap();
    let deletion = fs::read(&path).unwrap();
    let three = merge_annotation_sidecars(&merged,&deletion).unwrap();
    assert_eq!(merge_annotation_sidecars(&left,&merge_annotation_sidecars(&right,&deletion).unwrap()).unwrap(),three);
    fs::write(&path,&three).unwrap();
    assert!(graph.book_note_resolve(&info.id,&note.id,&conflict.revision,
        "Missed deletion","",None,&info.source_sha256,false).is_err());
    let conflict = graph.book_notes_list(&info.id).unwrap().remove(0);
    assert_eq!(conflict.conflicts.len(),3);
    assert_eq!(conflict.conflicts.iter().filter(|c|c.deleted).count(),1);
    graph.book_note_resolve(&info.id,&note.id,&conflict.revision,
        "Explicit merged result","",None,&info.source_sha256,false).unwrap();
    let resolved = fs::read(&path).unwrap();
    assert_eq!(merge_annotation_sidecars(&resolved,&three).unwrap(),resolved);
    let note = graph.book_notes_list(&info.id).unwrap().remove(0);
    assert!(note.conflicts.is_empty());
    assert_eq!(note.body,"Explicit merged result");
    graph.book_note_delete(&info.id,&note.id,&note.revision).unwrap();
    let deleted = fs::read(&path).unwrap();
    fs::write(&path,merge_annotation_sidecars(&deleted,&base).unwrap()).unwrap();
    assert!(graph.book_notes_list(&info.id).unwrap().is_empty());

    // An explicit delete resolution also consumes every concurrent candidate.
    fs::write(&path,&three).unwrap();
    let conflict = graph.book_notes_list(&info.id).unwrap().remove(0);
    graph.book_note_resolve(&info.id,&note.id,&conflict.revision,
        "User chose deletion","",None,&info.source_sha256,true).unwrap();
    let deleted_resolution = fs::read(&path).unwrap();
    assert_eq!(merge_annotation_sidecars(&deleted_resolution,&three).unwrap(),deleted_resolution);
    assert!(graph.book_notes_list(&info.id).unwrap().is_empty());
}

#[test]
fn independent_sidecar_additions_union_and_corruption_is_never_overwritten() {
    let (_source, root, graph, info) = fixture();
    let first = graph.book_note_save(&info.id,&Uuid::new_v4().to_string(),None,
        "One","",None,&info.source_sha256).unwrap();
    let path = root.path().join(&first.file_path);
    let left = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    graph.reconcile_book_notes().unwrap();
    graph.book_note_save(&info.id,&Uuid::new_v4().to_string(),None,
        "Two","",None,&info.source_sha256).unwrap();
    let right = fs::read(&path).unwrap();
    let merged = merge_annotation_sidecars(&left,&right).unwrap();
    fs::write(&path,&merged).unwrap();
    assert_eq!(graph.book_notes_list(&info.id).unwrap().len(),2);
    let mut duplicate: serde_json::Value = serde_json::from_slice(&left).unwrap();
    duplicate["items"][0]["body"]["value"] = serde_json::json!("Different payload");
    assert!(merge_annotation_sidecars(&left,&serde_json::to_vec(&duplicate).unwrap()).is_err());
    let mut wrong_book: serde_json::Value = serde_json::from_slice(&left).unwrap();
    wrong_book["grafium:bookId"] = serde_json::json!(format!("urn:uuid:{}",Uuid::new_v4()));
    assert!(merge_annotation_sidecars(&left,&serde_json::to_vec(&wrong_book).unwrap()).is_err());
    let mut cyclic: serde_json::Value = serde_json::from_slice(&left).unwrap();
    cyclic["items"][0]["grafium:parents"] = serde_json::json!([cyclic["items"][0]["id"].clone()]);
    assert!(merge_annotation_sidecars(&left,&serde_json::to_vec(&cyclic).unwrap()).is_err());
    let mut missing: serde_json::Value = serde_json::from_slice(&left).unwrap();
    missing["items"][0]["grafium:parents"] = serde_json::json!([format!("urn:uuid:{}",Uuid::new_v4())]);
    assert!(merge_annotation_sidecars(&left,&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut future: serde_json::Value = serde_json::from_slice(&left).unwrap();
    future["grafium:version"] = serde_json::json!(99);
    for bytes in [b"not JSON".to_vec(),serde_json::to_vec(&future).unwrap()] {
        fs::write(&path,&bytes).unwrap();
        assert!(graph.book_notes_list(&info.id).is_err());
        assert!(graph.book_note_save(&info.id,&first.id,Some(&first.revision),
            "Overwrite","",None,&info.source_sha256).is_err());
        assert!(graph.book_note_delete(&info.id,&first.id,&first.revision).is_err());
        assert_eq!(fs::read(&path).unwrap(),bytes);
    }
}

#[test]
fn legacy_files_coexist_unchanged_and_sidecar_watcher_reindexes_notes() {
    let (_source, root, graph, info) = fixture();
    let old = legacy_note(&graph,&info,&Uuid::new_v4().to_string(),"Legacy terbium");
    let legacy_path = root.path().join(&old.file_path);
    let legacy_bytes = fs::read(&legacy_path).unwrap();
    let note = graph.book_note_save(&info.id,&Uuid::new_v4().to_string(),None,
        "Modern thulium","",None,&info.source_sha256).unwrap();
    let path = root.path().join(&note.file_path);
    graph.reindex_all().unwrap();
    assert_eq!(graph.book_notes_list(&info.id).unwrap().len(),2);
    assert_eq!(fs::read(&legacy_path).unwrap(),legacy_bytes);
    let before = fs::read(&path).unwrap();
    graph.book_note_save(&info.id,&note.id,Some(&note.revision),
        "Watcher lutetium","",None,&info.source_sha256).unwrap();
    let after = fs::read(&path).unwrap();
    fs::write(&path,&before).unwrap();
    graph.reconcile_book_notes().unwrap();
    fs::write(&path,&after).unwrap();
    let event = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any)).add_path(path.clone());
    assert!(crate::source_events::should_process_event(&event,&graph.pages_dir,&graph.journals_dir,&graph.knowledge_dir));
    crate::source_events::reconcile_watched_paths(&graph,&HashSet::from([path.clone()]),false).unwrap();
    assert!(!graph.db.search_fts("lutetium",10).unwrap().is_empty());
    assert!(graph.db.search_fts("thulium",10).unwrap().is_empty());
    let current = graph.book_notes_list(&info.id).unwrap().into_iter().find(|n|n.id==note.id).unwrap();
    graph.book_note_delete(&info.id,&note.id,&current.revision).unwrap();
    assert_eq!(fs::read(&legacy_path).unwrap(),legacy_bytes);
    assert_eq!(graph.book_notes_list(&info.id).unwrap().len(),1);
    let changed = graph.book_note_save(&info.id,&old.id,Some(&old.revision),
        "Edited legacy","",None,&info.source_sha256).unwrap();
    assert!(changed.file_path.ends_with(".md"));
    assert!(graph.book_notes_list(&info.id).unwrap()[0].conflicts.is_empty());
}

#[test]
fn importing_copied_original_and_adjacent_sidecar_preserves_and_unions_history() {
    let (source, root, graph, info) = fixture();
    let note = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Portable annotation", "", None, &info.source_sha256).unwrap();
    let external = source.path().join("Exported.fb2");
    fs::copy(root.path().join(&info.file_path), &external).unwrap();
    fs::copy(root.path().join(&note.file_path), external.with_extension("jsonld")).unwrap();
    let copy = tempfile::tempdir_in(".").unwrap();
    let copied = Graph::open(copy.path()).unwrap();
    let imported = copied.import_original_book(&external).unwrap();
    assert_eq!(imported.id, info.id);
    let notes = copied.book_notes_list(&info.id).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].body, "Portable annotation");
    assert_eq!(notes[0].revision, note.revision);

    graph.book_note_save(&info.id, &note.id, Some(&note.revision),
        "Exported edit", "", None, &info.source_sha256).unwrap();
    copied.book_note_save(&info.id, &note.id, Some(&note.revision),
        "Local edit", "", None, &info.source_sha256).unwrap();
    fs::copy(root.path().join(&note.file_path), external.with_extension("jsonld")).unwrap();
    copied.import_original_book(&external).unwrap();
    let conflict = copied.book_notes_list(&info.id).unwrap().remove(0);
    assert_eq!(conflict.conflicts.len(), 2);
    let sidecar = copy.path().join(&note.file_path);
    let merged = fs::read(&sidecar).unwrap();
    copied.import_original_book(&external).unwrap();
    assert_eq!(fs::read(&sidecar).unwrap(), merged);
    copied.book_note_resolve(&info.id, &note.id, &conflict.revision, "", "", None,
        &info.source_sha256, true).unwrap();
    copied.import_original_book(&external).unwrap();
    assert!(copied.book_notes_list(&info.id).unwrap().is_empty());
    assert_eq!(copied.book_read_bytes(&info.id).unwrap(), graph.book_read_bytes(&info.id).unwrap());
}

#[test]
fn adjacent_import_rejects_corruption_identity_and_hash_mismatch_before_writing() {
    let (source, root, graph, info) = fixture();
    let note = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Keep original annotations", "", None, &info.source_sha256).unwrap();
    let external = source.path().join("Synthetic.fb2");
    let sidecar = external.with_extension("jsonld");
    let original = fs::read(root.path().join(&note.file_path)).unwrap();
    let mut wrong_hash: serde_json::Value = serde_json::from_slice(&original).unwrap();
    wrong_hash["items"][0]["target"]["grafium:sourceSha256"] = serde_json::json!("f".repeat(64));
    let mut wrong_book: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let other_id = Uuid::new_v4();
    wrong_book["grafium:bookId"] = serde_json::json!(format!("urn:uuid:{other_id}"));
    wrong_book["id"] = serde_json::json!(format!("urn:uuid:{other_id}:annotations"));
    for bytes in [b"{corrupt".to_vec(), serde_json::to_vec(&wrong_hash).unwrap(),
        serde_json::to_vec(&wrong_book).unwrap()] {
        fs::write(&sidecar, &bytes).unwrap();
        let empty = tempfile::tempdir_in(".").unwrap();
        let destination = Graph::open(empty.path()).unwrap();
        assert!(destination.import_original_book(&external).is_err());
        assert_eq!(fs::read_dir(empty.path().join("books")).unwrap().count(), 0);
        assert_eq!(fs::read(&sidecar).unwrap(), bytes);
        assert!(graph.import_original_book(&external).is_err());
        assert_eq!(fs::read(root.path().join(&note.file_path)).unwrap(), original);
    }
    fs::remove_file(root.path().join(&info.file_path)).unwrap();
    assert!(graph.import_original_book(&external).is_err());
    assert!(!root.path().join(&info.file_path).exists());
    assert_eq!(graph.book_notes_list(&info.id).unwrap()[0].status, "orphaned");
}

#[test]
fn synthetic_non_epub_cfi_is_custom_locator_not_an_original_resource_fragment() {
    let (_source, root, graph, info) = fixture();
    let location = BookLocation::Epub {
        cfi: "epubcfi(/6/2!/4/2:0)".into(), renderer_version: "foliate-js".into(),
    };
    let note = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Note", "author's cobalt", Some(location.clone()), &info.source_sha256).unwrap();
    let bytes = fs::read(root.path().join(&note.file_path)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let target = &doc["items"][0]["target"];
    assert_eq!(target["grafium:locator"], serde_json::to_value(location).unwrap());
    assert_eq!(target["selector"].as_array().unwrap().len(), 1);
    assert_eq!(target["selector"][0]["type"], "TextQuoteSelector");
    validate_annotation_sidecar(&note.file_path, &bytes).unwrap();
}

#[test]
fn annotation_context_survives_missing_original_and_deleted_manifest() {
    let (source, root, graph, info) = fixture();
    let note = graph.book_note_save(&info.id, &Uuid::new_v4().to_string(), None,
        "Orphan annotation", "cobalt", None, &info.source_sha256).unwrap();
    let attached = graph.book_notes_context(&info.id).unwrap();
    assert!(attached.source_available);
    assert_eq!(attached.source_sha256, info.source_sha256);
    fs::remove_file(root.path().join(&info.file_path)).unwrap();
    let missing = graph.book_notes_context(&info.id).unwrap();
    assert!(!missing.source_available);
    assert_eq!(missing.title, info.title);
    assert_eq!(missing.source_sha256, info.source_sha256);
    assert!(missing.indexing_warning.unwrap().contains("unavailable"));
    graph.import_original_book(&source.path().join("Synthetic.fb2")).unwrap();
    graph.delete_page(&info.id).unwrap();
    graph.reindex_all().unwrap();
    let orphan = graph.book_notes_context(&info.id).unwrap();
    assert!(!orphan.source_available);
    assert_eq!(orphan.id, info.id);
    assert_eq!(orphan.format, "fb2");
    assert_eq!(orphan.file_path, info.file_path);
    assert_eq!(orphan.source_sha256, info.source_sha256);
    graph.book_note_save(&info.id, &note.id, Some(&note.revision),
        "Still editable", &note.quote, note.locator, &note.source_sha256).unwrap();
    assert_eq!(graph.book_notes_list(&info.id).unwrap()[0].body, "Still editable");
}
