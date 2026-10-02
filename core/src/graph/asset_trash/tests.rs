use super::*;
use tempfile::TempDir;

fn setup() -> (TempDir, Graph) {
    let dir = TempDir::new().unwrap();
    let graph = Graph::open(dir.path()).unwrap();
    (dir, graph)
}

fn put(graph: &Graph, name: &str, bytes: &[u8]) {
    let path = graph.root_dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn spec(block: &Block) -> BlockCreateSpec {
    BlockCreateSpec {
        id: Some(block.id.clone()),
        parent: block
            .parent_id
            .clone()
            .map_or(BlockCreateParent::Root, BlockCreateParent::Existing),
        order_index: block.order_index,
        content: block.content.clone(),
        block_type: block.block_type.clone(),
        properties: block.properties.clone(),
    }
}

#[test]
fn trash_containing_folder_resolves_each_copy_without_changing_bytes() -> Result<()> {
    let (_dir, graph) = setup();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        put(&graph, "assets/nested/archive file.zip", bytes);
        let scan = graph.scan_unused_assets()?;
        assert_eq!(graph.trash_unused_assets(&scan.graph_path, &scan.assets)?.moved.len(), 1);
    }
    let scan = graph.list_asset_trash()?;
    assert_eq!(scan.assets.len(), 2);
    let mut folders = HashSet::new();
    for entry in &scan.assets {
        let file = graph.root_dir.canonicalize()?.join(&entry.trash_filename);
        let before = fs::read(&file)?;
        let folder = graph.asset_trash_containing_folder(&scan.graph_path, entry)?;
        assert_eq!(folder, file.parent().unwrap());
        assert!(folders.insert(folder));
        assert_eq!(fs::read(file)?, before);
        assert!(!graph.root_dir.join(&entry.filename).exists());
    }
    assert_eq!(graph.list_asset_trash()?.assets.len(), 2);
    Ok(())
}

#[test]
fn trash_containing_folder_rejects_stale_and_invalid_previews() -> Result<()> {
    let (_dir, graph) = setup();
    put(&graph, "assets/archive.zip", b"archive");
    let scan = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&scan.graph_path, &scan.assets)?;
    let scan = graph.list_asset_trash()?;
    let entry = &scan.assets[0];
    for graph_path in ["", "/different/graph"] {
        assert!(graph.asset_trash_containing_folder(graph_path, entry)
            .unwrap_err().to_string().contains("active graph changed"));
    }
    for path in [
        "../assets/archive.zip",
        "assets/archive.zip",
        ".grafium/asset-trash/.purged/archive.zip",
        ".grafium/asset-trash/not-a-batch/assets/archive.zip",
        &format!("/{}", entry.trash_filename),
        &entry.trash_filename.replace("/assets/", "/../assets/"),
    ] {
        let mut invalid = entry.clone();
        invalid.trash_filename = path.into();
        assert!(graph.asset_trash_containing_folder(&scan.graph_path, &invalid).is_err(), "{path}");
    }
    let mut mismatched = entry.clone();
    mismatched.filename = "assets/other.zip".into();
    assert!(graph.asset_trash_containing_folder(&scan.graph_path, &mismatched).is_err());
    graph.restore_trashed_assets(&scan.graph_path, &scan.assets)?;
    assert!(graph.asset_trash_containing_folder(&scan.graph_path, entry).is_err());
    assert_eq!(fs::read(graph.root_dir.join("assets/archive.zip"))?, b"archive");
    Ok(())
}

#[cfg(unix)]
#[test]
fn trash_containing_folder_rejects_symlinked_batches() -> Result<()> {
    let (_dir, graph) = setup();
    let outside = TempDir::new()?;
    fs::create_dir(outside.path().join("assets"))?;
    fs::write(outside.path().join("assets/archive.zip"), b"outside")?;
    let batch = Uuid::new_v4();
    fs::create_dir_all(graph.root_dir.join(TRASH))?;
    std::os::unix::fs::symlink(outside.path(), graph.root_dir.join(TRASH).join(batch.to_string()))?;
    let entry = AssetTrashEntry {
        filename: "assets/archive.zip".into(),
        trash_filename: format!("{TRASH}/{batch}/assets/archive.zip"),
        size: 7,
        sha256: String::new(),
    };
    assert!(graph.asset_trash_containing_folder(
        graph.root_dir.canonicalize()?.to_str().unwrap(), &entry
    ).unwrap_err().to_string().contains("Symbolic link"));
    assert_eq!(fs::read(outside.path().join("assets/archive.zip"))?, b"outside");
    Ok(())
}

#[test]
fn deleting_subtree_restoring_and_redoing_preserves_all_attachment_bytes() -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page_with_content("Attachments", false,
        "- ![image](../assets/photo.png)\n  - [archive](../assets/files.zip)\n  - <audio src=\"../assets/sound.mp3\"></audio>\n- Keep\n")?;
    for name in ["photo.png", "files.zip", "sound.mp3"] {
        put(&graph, &format!("assets/{name}"), name.as_bytes());
    }
    let blocks = graph.db.list_blocks_for_page(&page.id)?;
    let removed = graph.delete_blocks(&page.id, &[blocks[0].id.clone()])?;
    assert_eq!(removed.len(), 3);
    assert_eq!(graph.list_asset_trash()?.assets.len(), 3);
    for name in ["photo.png", "files.zip", "sound.mp3"] {
        assert!(!graph.root_dir.join("assets").join(name).exists());
    }
    graph.create_blocks(&page.id, removed.iter().map(spec).collect())?;
    for name in ["photo.png", "files.zip", "sound.mp3"] {
        assert_eq!(
            fs::read(graph.root_dir.join("assets").join(name))?,
            name.as_bytes()
        );
    }
    assert!(graph.list_asset_trash()?.assets.is_empty());
    graph.delete_blocks(&page.id, &[blocks[0].id.clone()])?;
    assert_eq!(graph.list_asset_trash()?.assets.len(), 3);
    Ok(())
}

#[test]
fn text_delete_undo_and_restart_restore_encoded_page_local_attachment() -> Result<()> {
    let (dir, graph) = setup();
    let before = "![figure](assets/a%20b.png)";
    let page = graph.create_page_with_content("Book/Chapter", false, &format!("- {before}\n"))?;
    put(&graph, "pages/Book/assets/a b.png", b"picture");
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    graph.update_block(&block.id, "Remaining text", None)?;
    assert!(!graph.root_dir.join("pages/Book/assets/a b.png").exists());
    let scan = graph.list_asset_trash()?;
    assert_eq!(scan.assets.len(), 1);
    assert_eq!(
        fs::read(graph.root_dir.join(&scan.assets[0].trash_filename))?,
        b"picture"
    );
    drop(graph);
    let graph = Graph::open(dir.path())?;
    graph.update_block(&block.id, before, None)?;
    assert_eq!(
        fs::read(graph.root_dir.join("pages/Book/assets/a b.png"))?,
        b"picture"
    );
    assert!(graph.list_asset_trash()?.assets.is_empty());
    Ok(())
}

#[test]
fn shared_files_survive_until_last_reference_is_deleted() -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page_with_content(
        "Shared",
        false,
        "- [one](../assets/shared.zip)\n- [two](../assets/shared.zip)\n",
    )?;
    put(&graph, "assets/shared.zip", b"archive");
    let blocks = graph.db.list_blocks_for_page(&page.id)?;
    graph.delete_block(&blocks[0].id)?;
    assert!(graph.root_dir.join("assets/shared.zip").exists());
    graph.delete_block(&blocks[1].id)?;
    assert!(!graph.root_dir.join("assets/shared.zip").exists());
    assert_eq!(graph.list_asset_trash()?.assets.len(), 1);
    Ok(())
}

#[test]
fn page_delete_protects_notes_and_indexed_references_but_not_chat() -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page_with_content("Remove", false,
        "- [note](assets/note.zip) [chat](assets/chat.zip) [indexed](assets/indexed.zip) [unused](assets/unused.zip)\n")?;
    for name in ["note.zip", "chat.zip", "indexed.zip", "unused.zip"] {
        put(&graph, &format!("pages/assets/{name}"), b"bytes");
    }
    put(
        &graph,
        "books/book/original.jsonld",
        br#"{"body":{"value":"[note](../../pages/assets/note.zip)"}}"#,
    );
    let other = graph.db.create_page("Pending", false)?;
    graph.db.create_block(
        &other.id,
        None,
        0,
        "[file](assets/indexed.zip)",
        BlockType::Text,
        serde_json::json!({}),
    )?;
    let conn = graph.db.conn()?;
    conn.execute(
        "INSERT INTO chat_threads(id,title,created_at,updated_at) VALUES('t','Chat',0,0)",
        [],
    )?;
    conn.execute("INSERT INTO chat_messages(id,thread_id,position,role,content,created_at) VALUES('m','t',0,'user','assets/chat.zip',0)", [])?;
    let deleted = graph.delete_page(&page.id)?;
    assert_eq!(deleted.deleted_assets, 2);
    for name in ["note.zip", "indexed.zip"] {
        assert!(graph.root_dir.join("pages/assets").join(name).exists());
    }
    assert_eq!(
        graph.list_asset_trash()?.assets.into_iter().map(|asset| asset.filename).collect::<Vec<_>>(),
        vec!["pages/assets/chat.zip", "pages/assets/unused.zip"]
    );
    assert!(!graph.root_dir.join("pages/assets/chat.zip").exists());
    assert!(graph.db.load_chat_thread("t")?.is_some());
    Ok(())
}

#[test]
fn removing_last_note_reference_trashes_chat_quoted_zip_and_undo_restores_it() -> Result<()> {
    for operation in ["edit", "single", "batch"] {
        let (_dir, graph) = setup();
        let page = graph.create_page_with_content(
            "Notes", false, "- [[Ephemeral]] [archive](../assets/archive.zip)\n",
        )?;
        put(&graph, "assets/archive.zip", b"archive bytes");
        let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
        let target = graph.db.get_page_by_title("Ephemeral")?;
        graph.db.conn()?.execute(
            "INSERT INTO chat_threads(id,title,source_page_id,source_page_title,context_json,created_at,updated_at)
             VALUES('t','Discussion',?1,'Ephemeral',?2,0,0)",
            rusqlite::params![target.id, serde_json::json!({"pageId": target.id, "excerpt": block.content}).to_string()],
        )?;
        graph.db.conn()?.execute(
            "INSERT INTO chat_messages(id,thread_id,position,role,content,sources_json,created_at)
             VALUES('m','t',0,'assistant',?1,?2,0)",
            rusqlite::params![block.content, serde_json::json!([{"pageId": target.id, "path": "../assets/archive.zip"}]).to_string()],
        )?;
        match operation {
            "edit" => graph.update_block(&block.id, "No attachment", None)?,
            "single" => graph.delete_block(&block.id)?,
            _ => { graph.delete_blocks(&page.id, &[block.id.clone()])?; }
        }
        assert!(!graph.root_dir.join("assets/archive.zip").exists(), "{operation}");
        assert_eq!(graph.list_asset_trash()?.assets.len(), 1, "{operation}");
        assert!(graph.db.find_page_by_title("Ephemeral")?.is_none(), "{operation}");
        assert_eq!(graph.db.load_chat_thread("t")?.unwrap().messages[0].content, block.content);
        if operation == "edit" {
            graph.update_block(&block.id, &block.content, None)?;
        } else {
            graph.create_blocks(&page.id, vec![spec(&block)])?;
        }
        assert_eq!(fs::read(graph.root_dir.join("assets/archive.zip"))?, b"archive bytes");
        assert!(graph.list_asset_trash()?.assets.is_empty());
        assert!(graph.db.get_page_by_title("Ephemeral").is_ok());
    }
    Ok(())
}

#[test]
fn purge_frees_attachment_bytes_and_old_undo_reports_missing_file_without_mutating_text(
) -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page_with_content("Purge", false, "- [file](../assets/file.zip)\n")?;
    put(&graph, "assets/file.zip", b"archive bytes");
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    graph.update_block(&block.id, "No attachment", None)?;
    let scan = graph.list_asset_trash()?;
    let result = graph.purge_trashed_assets(&scan.graph_path, &scan.assets)?;
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.purged, vec![scan.assets[0].trash_filename.clone()]);
    assert!(!graph.root_dir.join(&scan.assets[0].trash_filename).exists());
    assert!(graph.list_asset_trash()?.assets.is_empty());
    let failure = graph
        .update_block(&block.id, &block.content, None)
        .unwrap_err();
    assert!(failure.to_string().contains("permanently deleted"));
    assert_eq!(
        graph.db.get_block_by_id(&block.id)?.content,
        "No attachment"
    );
    assert!(
        !fs::read_to_string(graph.root_dir.join(page.file_path.unwrap()))?.contains("file.zip")
    );
    Ok(())
}

#[test]
fn purge_rechecks_new_references_changes_and_graph_identity() -> Result<()> {
    let (_dir, graph) = setup();
    for name in ["changed.zip", "referenced.zip", "valid.zip"] {
        put(&graph, &format!("assets/{name}"), b"before");
    }
    let unused = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&unused.graph_path, &unused.assets)?;
    let scan = graph.list_asset_trash()?;
    let changed = scan
        .assets
        .iter()
        .find(|e| e.filename == "assets/changed.zip")
        .unwrap();
    put(&graph, &changed.trash_filename, b"after");
    put(
        &graph,
        "pages/external.md",
        b"- [file](../assets/referenced.zip)",
    );
    assert!(graph
        .purge_trashed_assets("wrong graph", &scan.assets)
        .is_err());
    let result = graph.purge_trashed_assets(&scan.graph_path, &scan.assets)?;
    assert_eq!(result.purged.len(), 1);
    assert_eq!(result.errors.len(), 2);
    assert!(graph.root_dir.join(&changed.trash_filename).exists());
    Ok(())
}

#[test]
fn restore_never_overwrites_an_existing_file_or_accepts_crafted_paths() -> Result<()> {
    let (_dir, graph) = setup();
    put(&graph, "assets/file.zip", b"old");
    let unused = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&unused.graph_path, &unused.assets)?;
    let scan = graph.list_asset_trash()?;
    put(&graph, "assets/file.zip", b"new");
    let result = graph.restore_trashed_assets(&scan.graph_path, &scan.assets)?;
    assert!(result.restored.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert_eq!(fs::read(graph.root_dir.join("assets/file.zip"))?, b"new");
    assert_eq!(
        fs::read(graph.root_dir.join(&scan.assets[0].trash_filename))?,
        b"old"
    );
    let mut crafted = scan.assets[0].clone();
    crafted.filename = "pages/note.md".into();
    assert_eq!(
        graph
            .purge_trashed_assets(&scan.graph_path, &[crafted])?
            .errors
            .len(),
        1
    );
    assert!(graph
        .restore_trashed_assets("different graph", &scan.assets)
        .is_err());
    Ok(())
}

#[test]
fn cleanup_failure_keeps_saved_edit_and_surfaces_warning_without_losing_asset() -> Result<()> {
    let (_dir, graph) = setup();
    let page =
        graph.create_page_with_content("Failure", false, "- [file](../assets/file.zip)\n")?;
    put(&graph, "assets/file.zip", b"archive");
    put(&graph, ".grafium/asset-trash", b"not a directory");
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    graph.update_block(&block.id, "Saved edit", None)?;
    assert_eq!(graph.db.get_block_by_id(&block.id)?.content, "Saved edit");
    assert!(graph.root_dir.join("assets/file.zip").exists());
    assert!(!graph.take_asset_cleanup_warnings().is_empty());
    Ok(())
}

#[test]
fn restoration_fences_in_flight_sync() -> Result<()> {
    let (_dir, graph) = setup();
    put(&graph, "assets/file.zip", b"archive");
    let unused = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&unused.graph_path, &unused.assets)?;
    let fence =
        crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("assets/file.zip"))?;
    let scan = graph.list_asset_trash()?;
    assert_eq!(
        graph
            .restore_trashed_assets(&scan.graph_path, &scan.assets)?
            .restored
            .len(),
        1
    );
    assert!(!fence.is_current());
    Ok(())
}

#[test]
fn deleting_and_restoring_property_attachment_and_refusing_purged_batch_are_safe() -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page("Properties", false)?;
    put(&graph, "pages/assets/cover.png", b"cover");
    let block = graph.create_block(
        &page.id,
        None,
        0,
        "Title",
        BlockType::Text,
        serde_json::json!({"cover": "assets/cover.png"}),
    )?;
    graph.delete_block(&block.id)?;
    assert!(!graph.root_dir.join("pages/assets/cover.png").exists());
    graph.create_blocks(&page.id, vec![spec(&block)])?;
    assert_eq!(
        fs::read(graph.root_dir.join("pages/assets/cover.png"))?,
        b"cover"
    );
    graph.delete_block(&block.id)?;
    let scan = graph.list_asset_trash()?;
    assert_eq!(
        graph
            .purge_trashed_assets(&scan.graph_path, &scan.assets)?
            .purged
            .len(),
        1
    );
    let before = serde_json::to_value(graph.db.list_blocks_for_page(&page.id)?)?;
    assert!(graph
        .create_blocks(&page.id, vec![spec(&block)])
        .unwrap_err()
        .to_string()
        .contains("permanently deleted"));
    assert_eq!(
        serde_json::to_value(graph.db.list_blocks_for_page(&page.id)?)?,
        before
    );
    assert!(graph.db.get_block_by_id(&block.id).is_err());
    Ok(())
}

#[test]
fn undo_refuses_conflicting_original_without_overwriting_either_version() -> Result<()> {
    let (_dir, graph) = setup();
    let page =
        graph.create_page_with_content("Conflict", false, "- [file](../assets/file.zip)\n")?;
    put(&graph, "assets/file.zip", b"before");
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    graph.update_block(&block.id, "After", None)?;
    put(&graph, "assets/file.zip", b"external replacement");
    assert!(graph
        .update_block(&block.id, &block.content, None)
        .unwrap_err()
        .to_string()
        .contains("different file"));
    assert_eq!(graph.db.get_block_by_id(&block.id)?.content, "After");
    assert_eq!(
        fs::read(graph.root_dir.join("assets/file.zip"))?,
        b"external replacement"
    );
    let trash = graph.list_asset_trash()?;
    assert_eq!(
        fs::read(graph.root_dir.join(&trash.assets[0].trash_filename))?,
        b"before"
    );
    Ok(())
}

#[test]
fn manual_cleanup_preserves_undo_recovery_and_never_touches_external_links() -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page("Manual", false)?;
    put(&graph, "assets/file.zip", b"manual");
    let unused = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&unused.graph_path, &unused.assets)?;
    let block = graph.create_block(
        &page.id,
        None,
        0,
        "[file](../assets/file.zip)",
        BlockType::Text,
        serde_json::json!({}),
    )?;
    assert_eq!(fs::read(graph.root_dir.join("assets/file.zip"))?, b"manual");
    graph.update_block(
        &block.id,
        "![remote](https://example.test/assets/image.png)",
        None,
    )?;
    graph.update_block(&block.id, "No remote link", None)?;
    assert_eq!(graph.list_asset_trash()?.assets.len(), 1);
    assert!(graph.take_asset_cleanup_warnings().is_empty());
    Ok(())
}

#[test]
fn markdown_attachment_in_properties_is_preflighted_before_restoring_rows() -> Result<()> {
    let (_dir, graph) = setup();
    let page = graph.create_page("Property links", false)?;
    put(&graph, "assets/file.zip", b"bytes");
    let block = graph.create_block(
        &page.id,
        None,
        0,
        "Title",
        BlockType::Text,
        serde_json::json!({"attachment": "[file](../assets/file.zip)"}),
    )?;
    graph.delete_block(&block.id)?;
    let scan = graph.list_asset_trash()?;
    assert_eq!(scan.assets.len(), 1);
    assert_eq!(
        graph
            .purge_trashed_assets(&scan.graph_path, &scan.assets)?
            .purged
            .len(),
        1
    );
    let before = serde_json::to_value(graph.db.list_blocks_for_page(&page.id)?)?;
    assert!(graph
        .create_blocks(&page.id, vec![spec(&block)])
        .unwrap_err()
        .to_string()
        .contains("permanently deleted"));
    assert_eq!(
        serde_json::to_value(graph.db.list_blocks_for_page(&page.id)?)?,
        before
    );
    Ok(())
}

fn two_versions() -> Result<(TempDir, Graph, Block, AssetTrashScan)> {
    let (dir, graph) = setup();
    let page =
        graph.create_page_with_content("Versions", false, "- [file](../assets/file.zip)\n")?;
    put(&graph, "assets/file.zip", b"version A");
    let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);
    graph.update_block(&block.id, "Deleted attachment", None)?;
    put(&graph, "assets/file.zip", b"version B");
    let unused = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&unused.graph_path, &unused.assets)?;
    let scan = graph.list_asset_trash()?;
    assert_eq!(scan.assets.len(), 2);
    Ok((dir, graph, block, scan))
}

#[test]
fn explicitly_selected_version_resolves_undo_conflict_and_survives_restart_and_redo() -> Result<()>
{
    let (dir, graph, block, scan) = two_versions()?;
    assert!(graph
        .update_block(&block.id, &block.content, None)
        .unwrap_err()
        .to_string()
        .contains("Multiple versions"));
    let intended = scan
        .assets
        .iter()
        .find(|entry| entry.sha256 == format!("{:x}", Sha256::digest(b"version A")))
        .unwrap();
    assert_eq!(
        graph
            .restore_trashed_assets(&scan.graph_path, std::slice::from_ref(intended))?
            .restored
            .len(),
        1
    );
    drop(graph);
    let graph = Graph::open(dir.path())?;
    graph.update_block(&block.id, &block.content, None)?;
    assert_eq!(
        fs::read(graph.root_dir.join("assets/file.zip"))?,
        b"version A"
    );
    graph.update_block(&block.id, "Deleted again", None)?;
    graph.update_block(&block.id, &block.content, None)?;
    assert_eq!(
        fs::read(graph.root_dir.join("assets/file.zip"))?,
        b"version A"
    );
    assert_eq!(graph.list_asset_trash()?.assets.len(), 1);
    Ok(())
}

#[test]
fn purged_version_cannot_be_silently_replaced_by_different_surviving_bytes() -> Result<()> {
    let (_dir, graph, block, scan) = two_versions()?;
    let purged = scan
        .assets
        .iter()
        .find(|entry| entry.sha256 == format!("{:x}", Sha256::digest(b"version A")))
        .unwrap();
    assert_eq!(
        graph
            .purge_trashed_assets(&scan.graph_path, std::slice::from_ref(purged))?
            .purged
            .len(),
        1
    );
    assert!(graph
        .update_block(&block.id, &block.content, None)
        .unwrap_err()
        .to_string()
        .contains("different version was permanently deleted"));
    assert!(!graph.root_dir.join("assets/file.zip").exists());
    assert_eq!(
        graph.db.get_block_by_id(&block.id)?.content,
        "Deleted attachment"
    );
    let surviving = graph.list_asset_trash()?;
    assert_eq!(
        graph
            .restore_trashed_assets(&surviving.graph_path, &surviving.assets)?
            .restored
            .len(),
        1
    );
    graph.update_block(&block.id, &block.content, None)?;
    assert_eq!(
        fs::read(graph.root_dir.join("assets/file.zip"))?,
        b"version B"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn restore_rejects_symlinked_destination_and_purge_rejects_symlinked_trash() -> Result<()> {
    use std::os::unix::fs::symlink;
    let (_dir, graph) = setup();
    put(&graph, "pages/Book/assets/file.zip", b"archive");
    let unused = graph.scan_unused_assets()?;
    graph.trash_unused_assets(&unused.graph_path, &unused.assets)?;
    let scan = graph.list_asset_trash()?;
    fs::remove_dir(graph.root_dir.join("pages/Book/assets"))?;
    let outside = TempDir::new()?;
    symlink(outside.path(), graph.root_dir.join("pages/Book/assets"))?;
    assert_eq!(
        graph
            .restore_trashed_assets(&scan.graph_path, &scan.assets)?
            .errors
            .len(),
        1
    );
    assert!(!outside.path().join("file.zip").exists());
    let entry = &scan.assets[0];
    fs::remove_file(graph.root_dir.join(&entry.trash_filename))?;
    put(&graph, "assets/other.zip", b"archive");
    symlink(
        graph.root_dir.join("assets/other.zip"),
        graph.root_dir.join(&entry.trash_filename),
    )?;
    assert_eq!(
        graph
            .purge_trashed_assets(&scan.graph_path, &scan.assets)?
            .errors
            .len(),
        1
    );
    assert!(graph.root_dir.join("assets/other.zip").exists());
    Ok(())
}
