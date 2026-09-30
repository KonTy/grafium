use super::*;
use rusqlite::params;

fn graph() -> (tempfile::TempDir, Graph) {
    let directory = tempfile::tempdir_in(".").unwrap();
    let graph = Graph::open(directory.path()).unwrap();
    (directory, graph)
}

#[test]
fn file_identity_survives_external_title_changes_without_duplicate_sources() -> Result<()> {
    let (_directory, graph) = graph();
    let path = graph.pages_dir.join("A.md");
    fs::write(
        &path,
        "title:: Before\n- Question :: Answer\n  id:: stable-card\n",
    )?;
    graph.index_file(&path)?;
    let before = graph.db.get_page_by_title("Before")?;
    let reader = graph.create_page_with_content("Reader", false, "- [[Before]]\n")?;
    let reader_path = graph.root_dir.join(reader.file_path.unwrap());
    let reader_bytes = fs::read(&reader_path)?;
    let card = graph.db.list_flashcards(10, 0)?.remove(0);
    graph.db.grade_flashcard(&card.id, 5)?;
    fs::write(
        &path,
        "title:: After\n- Question :: Answer\n  id:: stable-card\n",
    )?;
    graph.index_file(&path)?;
    assert_eq!(graph.db.get_page_by_title("After")?.id, before.id);
    assert_eq!(graph.db.count_file_backed_pages()?, 2);
    assert_eq!(graph.db.list_flashcards(10, 0)?[0].review_count, 1);
    assert_eq!(graph.db.get_backlinks(&before.id)?.len(), 1);
    assert_eq!(fs::read(reader_path)?, reader_bytes);
    Ok(())
}

#[test]
fn legacy_duplicate_paths_preserve_identities_favorites_and_review_recovery() -> Result<()> {
    let (_directory, graph) = graph();
    let page = graph.create_page_with_content("Current", false, "- Question :: Answer\n")?;
    graph
        .db
        .conn()?
        .execute_batch("DROP INDEX idx_pages_unique_source_path")?;
    let legacy = graph.db.create_page("Legacy", false)?;
    let block = graph.db.create_block(
        &legacy.id,
        None,
        0,
        "Question :: Answer",
        BlockType::Flashcard,
        serde_json::json!({}),
    )?;
    let card = graph
        .db
        .upsert_flashcard(&block.id, "Question", "Answer", &[])?;
    graph.db.grade_flashcard(&card.id, 5)?;
    graph.db.conn()?.execute(
        "UPDATE pages SET file_path=?1,properties='{\"owned\":\"retain\"}' WHERE id=?2",
        params![page.file_path, legacy.id],
    )?;
    graph.db.conn()?.execute(
        "INSERT INTO favorites(id,page_id,created_at) VALUES('fav',?1,0)",
        [&legacy.id],
    )?;
    let reader = graph.create_page_with_content("Reader", false, "- [[Legacy]]\n")?;
    let before = fs::read(graph.root_dir.join(reader.file_path.as_ref().unwrap()))?;
    graph.index_file(&graph.root_dir.join(page.file_path.as_ref().unwrap()))?;
    let detached = graph.db.get_page_by_id(&legacy.id)?;
    assert!(detached.file_path.is_none());
    assert_eq!(detached.properties["owned"], "retain");
    assert_eq!(graph.db.get_backlinks(&legacy.id)?.len(), 1);
    assert!(!graph.db.has_duplicate_source_paths()?);
    assert_eq!(graph.db.list_flashcards(10, 0)?[0].review_count, 1);
    assert_eq!(graph.db.list_flashcards(10, 0)?[0].block_id, block.id);
    let saved: i64 =
        graph
            .db
            .conn()?
            .query_row("SELECT count(*) FROM source_identity_recovery", [], |r| {
                r.get(0)
            })?;
    assert_eq!(saved, 2);
    assert_eq!(
        fs::read(graph.root_dir.join(reader.file_path.unwrap()))?,
        before
    );
    Ok(())
}

#[test]
fn deindex_retires_every_legacy_row_for_the_deleted_file() -> Result<()> {
    let (_directory, graph) = graph();
    let page = graph.create_page_with_content("First", false, "- Cobalt source\n")?;
    graph
        .db
        .conn()?
        .execute_batch("DROP INDEX idx_pages_unique_source_path")?;
    let duplicate = graph.db.create_page("Duplicate", false)?;
    graph.db.conn()?.execute(
        "UPDATE pages SET file_path=?1 WHERE id=?2",
        params![page.file_path, duplicate.id],
    )?;
    graph.db.create_block(
        &duplicate.id,
        None,
        0,
        "Zirconium source",
        BlockType::Text,
        serde_json::json!({}),
    )?;
    let path = graph.root_dir.join(page.file_path.unwrap());
    fs::remove_file(&path)?;
    graph.deindex_file(&path)?;
    assert!(graph.db.get_page_by_id(&page.id).is_err());
    assert!(graph.db.get_page_by_id(&duplicate.id).is_err());
    assert!(graph.db.search_fts("cobalt OR zirconium", 10)?.is_empty());
    Ok(())
}

#[test]
fn deleted_referenced_source_retains_target_identity_and_reconnects() -> Result<()> {
    let (_directory, graph) = graph();
    let target =
        graph.create_page_with_content("Target", false, "kind:: source\n- Old content\n")?;
    let reader = graph.create_page_with_content("Reader", false, "- [[Target]]\n")?;
    let path = graph.root_dir.join(reader.file_path.unwrap());
    let bytes = fs::read(&path)?;
    graph.delete_page(&target.id)?;
    let placeholder = graph.db.get_page_by_id(&target.id)?;
    assert!(placeholder.file_path.is_none());
    assert_eq!(placeholder.properties, serde_json::json!({}));
    assert!(graph.db.list_blocks_for_page(&target.id)?.is_empty());
    assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);
    assert_eq!(fs::read(&path)?, bytes);
    assert_eq!(
        graph
            .create_page_with_content("Target", false, "- New content\n")?
            .id,
        target.id
    );
    Ok(())
}

#[test]
fn generated_tag_gc_preserves_shared_authored_and_favorited_pages() -> Result<()> {
    let (_directory, graph) = graph();
    let owned = graph.db.create_page("Authored empty", false)?;
    let first =
        graph.create_page_with_content("First", false, "- #unused/leaf #shared #starred\n")?;
    let second = graph.create_page_with_content("Second", false, "- #shared\n")?;
    let starred = graph.db.get_page_by_title("starred")?;
    graph.db.conn()?.execute(
        "INSERT INTO favorites(id,page_id,created_at) VALUES('star',?1,0)",
        [&starred.id],
    )?;
    graph.delete_page(&first.id)?;
    assert!(graph.db.find_page_by_title("unused/leaf")?.is_none());
    assert!(graph.db.find_page_by_title("unused")?.is_none());
    assert!(graph.db.get_page_by_title("shared").is_ok());
    assert!(graph.db.get_page_by_id(&starred.id).is_ok());
    assert!(graph.db.get_page_by_id(&owned.id).is_ok());
    let block = graph.db.list_blocks_for_page(&second.id)?.remove(0);
    graph.update_block(&block.id, "No tags remain", None)?;
    assert!(graph.db.find_page_by_title("shared")?.is_none());
    Ok(())
}

#[test]
fn deleting_a_virtual_title_never_synthesizes_another_source_path() -> Result<()> {
    let (_directory, graph) = graph();
    let path = graph.pages_dir.join("A.md");
    let content = "title:: B\n- Actual B source [[A]]\n";
    fs::write(&path, content)?;
    graph.index_file(&path)?;
    let virtual_page = graph.db.get_page_by_title("A")?;
    assert!(virtual_page.file_path.is_none());
    graph.delete_page(&virtual_page.id)?;
    assert_eq!(fs::read_to_string(path)?, content);
    assert!(graph.db.get_page_by_title("B")?.file_path.is_some());
    Ok(())
}

#[test]
fn media_cleanup_preserves_indexed_original_pdf_sources() -> Result<()> {
    let (_directory, graph) = graph();
    let external = tempfile::tempdir_in(".")?;
    let original = external.path().join("Synthetic.pdf");
    fs::write(&original, b"%PDF-1.4\nsynthetic unsupported PDF")?;
    let book = graph.import_original_book(&original)?;
    let page = graph.create_page_with_content(
        "Link",
        false,
        &format!("- [PDF](../{})\n", book.file_path),
    )?;
    graph.delete_page(&page.id)?;
    assert_eq!(graph.book_read_bytes(&book.id)?, fs::read(original)?);
    Ok(())
}

#[test]
fn imported_folder_cleanup_preserves_relative_shared_assets_and_dotted_siblings() -> Result<()> {
    let (_directory, graph) = graph();
    let sibling = graph.create_page_with_content("Books/Volume", false, "- Sibling source\n")?;
    let book = graph.create_page_with_content("Books/Volume.v2", false, "- Book root\n")?;
    graph.create_page_with_content(
        "Books/Volume.v2/Chapter",
        false,
        "- ![picture](assets/shared.png)\n",
    )?;
    let asset = graph.pages_dir.join("Books/Volume.v2/assets/shared.png");
    fs::create_dir_all(asset.parent().unwrap())?;
    fs::write(&asset, b"shared picture")?;
    graph.create_page_with_content(
        "Other/Keep",
        false,
        "- ![picture](../Books/Volume.v2/assets/sh%61red.png)\n",
    )?;
    graph.delete_imported_book_folder(&book.title)?;
    assert!(graph.root_dir.join(sibling.file_path.unwrap()).exists());
    assert!(asset.exists());
    assert!(!graph.root_dir.join(book.file_path.unwrap()).exists());
    Ok(())
}

#[test]
fn graph_handles_share_reentrant_publication_lock_and_cannot_resurrect_deleted_sources(
) -> Result<()> {
    let (directory, graph) = graph();
    let page = graph.create_page_with_content("Race", false, "- Before deletion\n")?;
    let second = Graph::open(directory.path())?;
    let operations = graph.source_operation_lock();
    assert!(Arc::ptr_eq(&operations, &second.source_operation_lock()));
    let lock = operations.lock();
    let nested = operations.lock();
    drop(nested);
    let path = graph.root_dir.join(page.file_path.unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        second.index_file(&path)
    });
    started_rx.recv().unwrap();
    graph.delete_page(&page.id)?;
    drop(lock);
    assert!(worker.join().unwrap().is_err());
    assert!(graph.db.get_page_by_id(&page.id).is_err());
    assert!(graph.db.search_fts("deletion", 10)?.is_empty());
    Ok(())
}

#[test]
fn source_revision_revalidation_rejects_external_deletion_before_index_commit() -> Result<()> {
    let (_directory, graph) = graph();
    let page = graph.create_page_with_content("Race", false, "- Old source\n")?;
    let path = graph.root_dir.join(page.file_path.unwrap());
    fs::write(&path, "- Stale bytes must not publish\n")?;
    assert!(graph
        .index_file_impl_with_hook(&path, || {
            fs::remove_file(&path)?;
            Ok(())
        })
        .is_err());
    assert!(graph.db.get_page_by_id(&page.id).is_err());
    assert!(graph.db.search_fts("stale", 10)?.is_empty());
    Ok(())
}

#[test]
fn merge_failure_restores_source_destination_and_database() -> Result<()> {
    let (_directory, graph) = graph();
    let source = graph.create_page_with_content("Source", false, "- Source words\n")?;
    let dest = graph.create_page_with_content("Destination", false, "- Destination words\n")?;
    let reader = graph.create_page_with_content("Reader", false, "- [[Source]]\n")?;
    let snapshot = vec![source.clone(), dest.clone(), reader.clone()]
        .into_iter()
        .map(|page| {
            let path = graph.root_dir.join(page.file_path.as_ref().unwrap());
            (
                page.id.clone(),
                path.clone(),
                fs::read(path).unwrap(),
                graph.db.list_blocks_for_page(&page.id).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(graph
        .merge_pages_with_hook(&source.id, &dest.id, || Err(CoreError::Other(
            "injected publication failure".into()
        )))
        .is_err());
    for (id, path, bytes, blocks) in snapshot {
        assert_eq!(fs::read(path)?, bytes);
        assert_eq!(
            serde_json::to_value(graph.db.list_blocks_for_page(&id)?)?,
            serde_json::to_value(blocks)?
        );
    }
    graph.merge_page(&source.id, &dest.id)?;
    let combined = fs::read_to_string(graph.root_dir.join(dest.file_path.unwrap()))?;
    assert!(combined.contains("Source words") && combined.contains("Destination words"));
    assert!(
        fs::read_to_string(graph.root_dir.join(reader.file_path.unwrap()))?
            .contains("[[Destination]]")
    );
    Ok(())
}

#[test]
fn bulk_merge_collision_failure_keeps_source_and_referring_documents() -> Result<()> {
    let (_directory, graph) = graph();
    let source = graph.create_page_with_content("Old/Target", false, "- Source words\n")?;
    let destination = graph.create_page_with_content("Target", false, "- Destination\n")?;
    let reference = graph.create_page_with_content("Reference", false, "- [[Old/Target]]\n")?;
    let destination_path = graph.root_dir.join(destination.file_path.unwrap());
    fs::remove_file(&destination_path)?;
    fs::create_dir(&destination_path)?;
    let result = graph.bulk_rename_pages("Old/", "", false)?;
    assert!(result.merged.is_empty());
    assert!(!result.skipped.is_empty());
    assert!(graph.root_dir.join(source.file_path.unwrap()).exists());
    assert!(
        fs::read_to_string(graph.root_dir.join(reference.file_path.unwrap()))?
            .contains("[[Old/Target]]")
    );
    Ok(())
}

#[test]
fn ordinary_and_writing_edits_reconcile_cards_properties_without_resetting_reviews() -> Result<()> {
    let (_directory, graph) = graph();
    let page =
        graph.create_page_with_content("Cards", false, "- Question :: Answer\n  owner:: old\n")?;
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    let card = graph.db.list_flashcards(10, 0)?.remove(0);
    graph.db.grade_flashcard(&card.id, 5)?;
    graph.update_block(
        &block.id,
        "Improved question :: Improved answer",
        Some(&serde_json::json!({"owner":"new"})),
    )?;
    let updated = graph.db.list_flashcards(10, 0)?.remove(0);
    assert_eq!(updated.id, card.id);
    assert_eq!(updated.review_count, 1);
    let owner: String = graph.db.conn()?.query_row(
        "SELECT value FROM block_properties WHERE block_id=?1 AND key='owner'",
        [&block.id],
        |r| r.get(0),
    )?;
    assert_eq!(owner, "new");
    let before = graph.db.list_blocks_for_page(&page.id)?;
    graph.apply_writing_changes(
        &page.id,
        &[WritingContentChange {
            block_id: block.id.clone(),
            before_content: before[0].content.clone(),
            after_content: "Ordinary prose".into(),
        }],
        Some(&before),
    )?;
    assert!(graph.db.list_flashcards(10, 0)?.is_empty());
    graph.update_block(&block.id, "Ordinary prose", Some(&serde_json::json!({})))?;
    let properties: i64 = graph.db.conn()?.query_row(
        "SELECT count(*) FROM block_properties WHERE block_id=?1",
        [&block.id],
        |r| r.get(0),
    )?;
    assert_eq!(properties, 0);
    Ok(())
}

#[test]
fn ink_fts_is_removed_by_direct_cascade_and_full_rebuild() -> Result<()> {
    let (_directory, graph) = graph();
    let page = graph.create_page("Ink", false)?;
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    let conn = graph.db.conn()?;
    conn.execute("INSERT INTO ink_pages(id,block_id,file_path,recognized_text,created_at) VALUES('ink',?1,'assets/ink.svg','cobalt',0)",[&block.id])?;
    conn.execute(
        "INSERT INTO fts_ink(ink_id,recognized_text) VALUES('ink','cobalt')",
        [],
    )?;
    conn.execute("DELETE FROM blocks WHERE id=?1", [&block.id])?;
    let rows: i64 = conn.query_row("SELECT count(*) FROM fts_ink", [], |r| r.get(0))?;
    assert_eq!(rows, 0);
    conn.execute(
        "INSERT INTO fts_ink(ink_id,recognized_text) VALUES('legacy-orphan','zirconium')",
        [],
    )?;
    drop(conn);
    graph.reindex_all()?;
    assert!(graph.db.search_ink_fts("zirconium", 10)?.is_empty());
    Ok(())
}

#[test]
fn conflict_backups_are_never_authoritative_pages_or_duplicate_book_notes() -> Result<()> {
    let (_directory, graph) = graph();
    let source = tempfile::tempdir_in(".")?;
    fs::write(
        source.path().join("Book.fb2"),
        "<FictionBook><body><p>Author text</p></body></FictionBook>",
    )?;
    let book = graph.import_original_book(&source.path().join("Book.fb2"))?;
    let note = graph.book_note_save(
        &book.id,
        &Uuid::new_v4().to_string(),
        None,
        "My note",
        "",
        None,
        &book.source_sha256,
    )?;
    let path = graph.root_dir.join(&note.file_path);
    let backup = path.with_file_name(format!("{}.conflict_123.md", note.id));
    fs::copy(&path, &backup)?;
    fs::write(
        graph.pages_dir.join("Ordinary.conflict_123.md"),
        "- Invisible recovery evidence\n",
    )?;
    graph.reindex_all()?;
    assert_eq!(graph.book_notes_list(&book.id)?.len(), 1);
    assert!(graph.db.search_fts("invisible", 10)?.is_empty());
    assert!(graph
        .db
        .find_page_by_file_path(&graph.relative_graph_path(&backup))?
        .is_none());
    assert!(backup.exists());
    Ok(())
}

#[test]
fn referenced_originals_and_notes_restore_their_retired_identities() -> Result<()> {
    let (_directory, graph) = graph();
    let external = tempfile::tempdir_in(".")?;
    let source = external.path().join("Book.fb2");
    fs::write(
        &source,
        "<FictionBook><body><p>Original evidence</p></body></FictionBook>",
    )?;
    let book = graph.import_original_book(&source)?;
    let note = graph.book_note_save(
        &book.id,
        &Uuid::new_v4().to_string(),
        None,
        "Own note",
        "",
        None,
        &book.source_sha256,
    )?;
    let note_page = graph.db.get_page_by_id(&note.note_page_id)?;
    let reader = graph.create_page_with_content(
        "Reader",
        false,
        &format!("- [[{}]] [[{}]]\n", book.title, note_page.title),
    )?;
    let before = fs::read(graph.root_dir.join(reader.file_path.as_ref().unwrap()))?;
    graph.delete_page(&book.page_id)?;
    assert!(graph.db.get_page_by_id(&book.page_id)?.file_path.is_none());
    assert_eq!(graph.db.get_backlinks(&book.page_id)?.len(),1);
    assert_eq!(graph.import_original_book(&source)?.page_id, book.page_id);
    assert_eq!(graph.db.get_backlinks(&book.page_id)?.len(),1);
    // A missing file retires only the rebuildable index. Restoring the same
    // authoritative sidecar must restore its identities and incoming links.
    let sidecar_path = graph.root_dir.join(&note.file_path);
    let sidecar = fs::read(&sidecar_path)?;
    fs::remove_file(&sidecar_path)?;
    graph.reconcile_book_notes()?;
    assert!(graph
        .db
        .get_page_by_id(&note.note_page_id)?
        .file_path
        .is_none());
    assert_eq!(graph.db.get_backlinks(&note.note_page_id)?.len(),1);
    fs::write(&sidecar_path, &sidecar)?;
    graph.reconcile_book_notes()?;
    assert_eq!(graph.book_notes_list(&book.id)?[0].note_page_id, note.note_page_id);
    assert_eq!(graph.db.get_backlinks(&note.note_page_id)?.len(),1);
    // API deletion is different: its persistent tombstone must reject stale
    // creates instead of silently resurrecting the deleted annotation.
    graph.book_note_delete(&book.id, &note.id, &note.revision)?;
    let deleted = fs::read(&sidecar_path)?;
    assert!(graph.book_note_save(
        &book.id, &note.id, None, "Restored note", "", None, &book.source_sha256
    ).is_err());
    assert_eq!(fs::read(&sidecar_path)?, deleted);
    assert!(graph.db.get_page_by_id(&note.note_page_id)?.file_path.is_none());
    assert_eq!(graph.db.get_backlinks(&note.note_page_id)?.len(),1);
    assert_eq!(
        fs::read(graph.root_dir.join(reader.file_path.unwrap()))?,
        before
    );
    Ok(())
}

#[test]
fn application_deletion_retires_all_legacy_alias_rows_for_one_source() -> Result<()> {
    let (_directory, graph) = graph();
    let source = graph.create_page_with_content("Source", false, "- Primary evidence\n")?;
    graph
        .db
        .conn()?
        .execute_batch("DROP INDEX idx_pages_unique_source_path")?;
    let duplicate = graph.db.create_page("Old title", false)?;
    graph.db.conn()?.execute(
        "UPDATE pages SET file_path=?1 WHERE id=?2",
        params![source.file_path, duplicate.id],
    )?;
    graph.db.create_block(
        &duplicate.id,
        None,
        0,
        "Stale duplicate evidence",
        BlockType::Text,
        serde_json::json!({}),
    )?;
    graph.create_page_with_content("Reader", false, "- [[Old title]]\n")?;
    graph.delete_page(&source.id)?;
    assert!(graph.db.get_page_by_id(&duplicate.id)?.file_path.is_none());
    assert!(graph.db.list_blocks_for_page(&duplicate.id)?.is_empty());
    assert!(graph.db.search_fts("evidence", 10)?.is_empty());
    assert!(!graph.root_dir.join(source.file_path.unwrap()).exists());
    Ok(())
}

#[test]
fn virtual_edits_cannot_overwrite_an_existing_differently_titled_file() -> Result<()> {
    let (_directory, graph) = graph();
    let path = graph.pages_dir.join("A.md");
    let content = "title:: B\n- B source [[A]]\n";
    fs::write(&path, content)?;
    graph.index_file(&path)?;
    let virtual_page = graph.db.get_page_by_title("A")?;
    assert!(graph
        .create_page_with_content("A", false, "- Replacement\n")
        .is_err());
    assert!(graph
        .create_block(
            &virtual_page.id,
            None,
            0,
            "Replacement",
            BlockType::Text,
            serde_json::json!({})
        )
        .is_err());
    assert_eq!(fs::read_to_string(path)?, content);
    assert!(graph.db.list_blocks_for_page(&virtual_page.id)?.is_empty());
    Ok(())
}

#[test]
fn live_block_creation_and_text_property_removal_share_derived_state() -> Result<()> {
    let (_directory, graph) = graph();
    let page = graph.create_page("Derived", false)?;
    let block = graph.create_block(
        &page.id,
        None,
        1,
        "Question :: Answer",
        BlockType::Text,
        serde_json::json!({"owner":"kept"}),
    )?;
    assert_eq!(graph.db.list_flashcards(10, 0)?.len(), 1);
    graph.update_block(&block.id, "Prose\nstatus:: temporary", None)?;
    let count: i64 = graph.db.conn()?.query_row(
        "SELECT count(*) FROM block_properties WHERE block_id=?1 AND key='status'",
        [&block.id],
        |r| r.get(0),
    )?;
    assert_eq!(count, 1);
    graph.update_block(&block.id, "Prose", None)?;
    let count: i64 = graph.db.conn()?.query_row(
        "SELECT count(*) FROM block_properties WHERE block_id=?1 AND key='status'",
        [&block.id],
        |r| r.get(0),
    )?;
    assert_eq!(count, 0);
    Ok(())
}

#[test]
fn reference_style_media_and_unowned_graph_files_survive_cleanup() -> Result<()> {
    let (_directory, graph) = graph();
    fs::create_dir_all(graph.root_dir.join("assets"))?;
    let asset = graph.root_dir.join("assets/shared image.png");
    fs::write(&asset, b"shared")?;
    let metadata = graph.root_dir.join(".grafium/own-data.json");
    fs::write(&metadata, b"owned graph metadata")?;
    let source = graph.create_page_with_content(
        "Delete",
        false,
        "- ![image](../assets/shared%20image.png)\n- [metadata](../.grafium/own-data.json)\n",
    )?;
    graph.create_page_with_content(
        ".Private",
        false,
        "- ![reference][image]\n\n[image]: ../assets/shared%20image.png\n",
    )?;
    graph.delete_page(&source.id)?;
    assert!(asset.exists());
    assert_eq!(fs::read(metadata)?, b"owned graph metadata");
    assert!(graph.db.get_page_by_title(".Private")?.file_path.is_some());
    assert_eq!(crate::import::books::percent_decode_lossy("%😀"), "%😀");
    Ok(())
}

#[test]
fn reopening_normalizes_legacy_cache_paths_without_losing_page_state() -> Result<()> {
    let (directory, graph) = graph();
    let page = graph.create_page_with_content("Portable", false, "- Portable source\n")?;
    graph.db.conn()?.execute(
        "UPDATE pages SET file_path=?1 WHERE id=?2",
        params!["pages\\Portable.md", page.id],
    )?;
    drop(graph);
    let reopened = Graph::open(directory.path())?;
    assert_eq!(
        reopened.db.get_page_by_id(&page.id)?.file_path.as_deref(),
        Some("pages/Portable.md")
    );
    reopened.delete_page(&page.id)?;
    assert!(!directory.path().join("pages/Portable.md").exists());
    Ok(())
}

#[test]
fn repeated_merges_preserve_every_sources_properties_in_durable_history() -> Result<()> {
    let (_directory, graph) = graph();
    let dest = graph.create_page_with_content(
        "Destination",
        false,
        "owner:: destination\n- Destination\n",
    )?;
    let first = graph.create_page_with_content("First", false, "owner:: first\n- First body\n")?;
    let second =
        graph.create_page_with_content("Second", false, "owner:: second\n- Second body\n")?;
    graph.merge_page(&first.id, &dest.id)?;
    graph.merge_page(&second.id, &dest.id)?;
    graph.reconcile_files_from_disk()?;
    let page = graph.db.get_page_by_id(&dest.id)?;
    let history: serde_json::Value = serde_json::from_str(
        page.properties["merged-source-properties"]
            .as_str()
            .unwrap(),
    )?;
    assert_eq!(history[0]["properties"]["owner"], "first");
    assert_eq!(history[1]["properties"]["owner"], "second");
    assert_eq!(page.properties["owner"], "destination");
    Ok(())
}

#[test]
fn mutation_fence_detects_initially_absent_create_delete_without_global_false_conflicts(
) -> Result<()> {
    let (_directory, graph) = graph();
    let fence = crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("pages/Race.md"))?;
    let unrelated = graph.create_page("Unrelated", false)?;
    graph.delete_page(&unrelated.id)?;
    assert!(fence.is_current());
    {
        let _publication = fence.lock();
        let page = graph.create_page_with_content(
            "Race",
            false,
            "- Local author chose to delete this\n",
        )?;
        graph.delete_page(&page.id)?;
    }
    assert!(!graph.pages_dir.join("Race.md").exists());
    assert!(
        !fence.is_current(),
        "absence alone must not authorize resurrecting an intentionally deleted source"
    );
    Ok(())
}

#[test]
fn mutation_fence_detects_directory_intent_and_same_byte_rewrites() -> Result<()> {
    let (_directory, graph) = graph();
    let file =
        crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("pages/Folder/A.md"))?;
    let directory =
        crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("pages/Folder"))?;
    crate::fsutil::record_source_mutation(&graph.root_dir, Path::new("pages/Folder/B.md"))?;
    assert!(file.is_current());
    assert!(!directory.is_current());
    crate::fsutil::record_source_mutation(&graph.root_dir, Path::new("pages/Folder"))?;
    assert!(!file.is_current());
    let page = graph.create_page_with_content("Identical", false, "- Same body\n")?;
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    graph.update_block(&block.id, "Same body", None)?;
    let path = graph.root_dir.join(page.file_path.as_ref().unwrap());
    let bytes = fs::read(&path)?;
    let fence = crate::fsutil::source_mutation_fence(
        &graph.root_dir,
        Path::new(page.file_path.as_ref().unwrap()),
    )?;
    graph.update_block(&block.id, "Same body", None)?;
    assert_eq!(fs::read(path)?, bytes);
    assert!(!fence.is_current());
    Ok(())
}

#[test]
fn mutation_fence_keeps_its_domain_alive_across_graph_handles() -> Result<()> {
    let (directory, graph) = graph();
    let fence = crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("pages/Later.md"))?;
    drop(graph);
    let graph = Graph::open(directory.path())?;
    let page = graph.create_page("Later", false)?;
    graph.delete_page(&page.id)?;
    assert!(!fence.is_current());
    assert!(
        crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("../outside")).is_err()
    );
    Ok(())
}

#[test]
fn job_epoch_protects_paths_discovered_after_remote_listing() -> Result<()> {
    let (_directory, graph) = graph();
    let epoch = crate::fsutil::graph_mutation_epoch(&graph.root_dir)?;
    let page = graph.create_page("Discovered later", false)?;
    graph.delete_page(&page.id)?;
    let relative = Path::new("pages/Discovered later.md");
    assert!(!epoch.for_path(relative)?.is_current());
    assert!(crate::fsutil::source_mutation_fence(&graph.root_dir, relative)?.is_current());
    assert!(epoch
        .for_path(Path::new("pages/Untouched.md"))?
        .is_current());
    Ok(())
}

#[test]
fn reconciliation_preserves_unambiguous_moved_source_ids_and_card_reviews_after_restart(
) -> Result<()> {
    let (directory, graph) = graph();
    let page = graph.create_page_with_content(
        "Old/Deck",
        false,
        "- Question :: Answer\n  id:: moved-card\n",
    )?;
    let card = graph.db.list_flashcards(10, 0)?.remove(0);
    graph.db.grade_flashcard(&card.id, 5)?;
    graph.update_block("moved-card", "Updated question :: Updated answer", None)?;
    graph.db.conn()?.execute(
        "INSERT INTO favorites(id,page_id,created_at) VALUES('moved-favorite',?1,0)",
        [&page.id],
    )?;
    let other = graph.create_page_with_content("Other", false, "- Other document\n")?;
    drop(graph);
    fs::rename(
        directory.path().join("pages/Old"),
        directory.path().join("pages/New"),
    )?;
    let reopened = Graph::open(directory.path())?;
    reopened.reconcile_files_from_disk()?;
    let moved = reopened.db.get_page_by_title("New/Deck")?;
    assert_eq!(moved.id, page.id);
    assert_eq!(moved.file_path.as_deref(), Some("pages/New/Deck.md"));
    assert_eq!(reopened.db.get_page_by_id(&other.id)?.title, "Other");
    let card = reopened.db.list_flashcards(10, 0)?.remove(0);
    assert_eq!(card.review_count, 1);
    assert_eq!(card.block_id, "moved-card");
    let favorites: i64 = reopened.db.conn()?.query_row(
        "SELECT count(*) FROM favorites WHERE page_id=?1",
        [&page.id],
        |r| r.get(0),
    )?;
    assert_eq!(favorites, 1);
    Ok(())
}

#[test]
fn reconciliation_clears_missing_title_owner_before_indexing_replacement() -> Result<()> {
    let (_directory, graph) = graph();
    let path = graph.pages_dir.join("Old.md");
    fs::write(&path, "title:: Shared title\n- Deleted old evidence\n")?;
    graph.index_file(&path)?;
    fs::remove_file(path)?;
    fs::write(
        graph.pages_dir.join("Incoming.md"),
        "title:: Shared title\n- Different incoming evidence\n",
    )?;
    graph.reconcile_files_from_disk()?;
    let replacement = graph.db.get_page_by_title("Shared title")?;
    assert_eq!(replacement.file_path.as_deref(), Some("pages/Incoming.md"));
    assert!(graph.db.search_fts("deleted", 10)?.is_empty());
    assert!(!graph.db.search_fts("incoming", 10)?.is_empty());
    Ok(())
}

#[test]
fn ambiguous_identical_folder_moves_are_not_guessed() -> Result<()> {
    let (_directory, graph) = graph();
    let first = graph.create_page_with_content("Old/First", false, "- Identical body\n")?;
    let second = graph.create_page_with_content("Old/Second", false, "- Identical body\n")?;
    fs::rename(graph.pages_dir.join("Old"), graph.pages_dir.join("New"))?;
    graph.reconcile_files_from_disk()?;
    let moved_first = graph.db.get_page_by_title("New/First")?;
    let moved_second = graph.db.get_page_by_title("New/Second")?;
    assert_ne!(moved_first.id, first.id);
    assert_ne!(moved_second.id, second.id);
    assert!(!graph.db.has_duplicate_source_paths()?);
    Ok(())
}

#[test]
fn malformed_arrival_does_not_prevent_retiring_missing_sources() -> Result<()> {
    let (_directory, graph) = graph();
    let missing = graph.create_page_with_content("Missing", false, "- Old evidence\n")?;
    fs::remove_file(graph.root_dir.join(missing.file_path.unwrap()))?;
    fs::write(
        graph.pages_dir.join("Invalid.md"),
        "book-note:: {}\n\nInvalid note",
    )?;
    assert!(graph.reconcile_files_from_disk().is_err());
    assert!(graph.db.get_page_by_id(&missing.id).is_err());
    assert!(graph.db.search_fts("evidence", 10)?.is_empty());
    Ok(())
}

#[test]
fn scan_errors_and_malformed_books_do_not_block_unrelated_source_deletion_cleanup() -> Result<()> {
    let (_directory, graph) = graph();
    let source = tempfile::tempdir_in(".")?;
    fs::write(
        source.path().join("Original.fb2"),
        "<FictionBook><body><p>Rubidium old author evidence</p></body></FictionBook>",
    )?;
    let book = graph.import_original_book(&source.path().join("Original.fb2"))?;
    let deleted =
        graph.create_page_with_content("Deleted", false, "- Tungsten deleted evidence\n")?;
    fs::remove_file(graph.root_dir.join(deleted.file_path.unwrap()))?;
    fs::write(
        graph.root_dir.join(format!("books/{}/book.json", book.id)),
        "invalid JSON",
    )?;
    fs::remove_dir(&graph.knowledge_dir)?;
    fs::write(&graph.knowledge_dir, "not a directory")?;
    fs::write(
        graph.pages_dir.join("Invalid.md"),
        "book-note:: {}\n\nInvalid note",
    )?;
    let error = graph.reconcile_files_from_disk().unwrap_err().to_string();
    assert!(error.contains("knowledge"));
    assert!(error.contains("Invalid.md"));
    assert!(error.contains(&book.id));
    assert!(graph.db.search_fts("tungsten OR rubidium", 10)?.is_empty());
    assert!(graph.db.get_page_by_id(&book.id).is_err());
    assert!(graph.db.get_page_by_id(&deleted.id).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_replacement_of_books_root_clears_old_author_evidence_without_reading_it() -> Result<()> {
    let (_directory, graph) = graph();
    let source = tempfile::tempdir_in(".")?;
    fs::write(
        source.path().join("Original.fb2"),
        "<FictionBook><body><p>Rubidium author evidence</p></body></FictionBook>",
    )?;
    let book = graph.import_original_book(&source.path().join("Original.fb2"))?;
    fs::rename(
        graph.root_dir.join("books"),
        graph.root_dir.join("displaced-books"),
    )?;
    std::os::unix::fs::symlink(source.path().canonicalize()?, graph.root_dir.join("books"))?;
    assert!(graph.reconcile_original_books().is_err());
    assert!(graph.db.get_page_by_id(&book.id).is_err());
    assert!(graph.db.search_fts("rubidium", 10)?.is_empty());
    assert!(graph
        .root_dir
        .join("displaced-books")
        .join(book.id)
        .join("original.fb2")
        .exists());
    Ok(())
}
