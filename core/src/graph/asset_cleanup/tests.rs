use super::*;
use tempfile::TempDir;

fn setup() -> (TempDir, Graph) {
    let dir = TempDir::new().unwrap();
    let graph = Graph::open(dir.path()).unwrap();
    (dir, graph)
}

fn put(graph: &Graph, path: &str, content: &[u8]) {
    let path = graph.root_dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn names(graph: &Graph) -> Vec<String> {
    graph
        .scan_unused_assets()
        .unwrap()
        .assets
        .into_iter()
        .map(|a| a.filename)
        .collect()
}

#[test]
fn scans_zip_and_nested_attachments_but_not_sources_or_hidden_files() {
    let (_dir, graph) = setup();
    for path in [
        "assets/joplinoled.zip",
        "assets/anki/deck/sound.mp3",
        "pages/book/assets/file.bin",
        "assets/.hidden",
        "assets/notes.md",
        "assets/annotation.jsonld",
        ".grafium/asset-trash/old/assets/old.zip",
    ] {
        put(&graph, path, b"content");
    }
    assert_eq!(
        names(&graph),
        vec![
            "assets/anki/deck/sound.mp3",
            "assets/joplinoled.zip",
            "pages/book/assets/file.bin"
        ]
    );
}

#[test]
fn protects_disk_references_without_reindexing_and_encoded_names() {
    let (_dir, graph) = setup();
    for path in [
        "assets/theme zip.zip",
        "assets/a&b.png",
        "assets/figure(1).png",
        "assets/unicode.png",
        "assets/config.bin",
        "pages/book/assets/local.mp3",
        "assets/ref.zip",
    ] {
        put(&graph, path, b"binary");
    }
    put(&graph, "pages/external.md", b"[zip](../assets/theme%20zip.zip)\n<img src='../assets/a&amp;b.png'>\n![x](../assets/figure\\(1\\).png)\n[attachment][r]\n[r]: ../assets/ref.zip");
    put(
        &graph,
        "books/1/original.jsonld",
        br#"{"body":{"value":"assets/\u0075nicode.png"},"audio":"local.mp3"}"#,
    );
    put(
        &graph,
        ".grafium/settings.json",
        br#"{"icon":"assets/config.bin"}"#,
    );
    assert!(names(&graph).is_empty());
}

#[test]
fn conservatively_retains_duplicate_names_and_asset_dependencies() {
    let (_dir, graph) = setup();
    put(&graph, "assets/shared.png", b"image");
    put(&graph, "pages/book/assets/shared.png", b"image");
    put(
        &graph,
        "assets/theme.css",
        b"body { background: url(shared.png); }",
    );
    put(&graph, "pages/use.md", b"[theme](../assets/theme.css)");
    assert!(names(&graph).is_empty());
}

#[test]
fn protects_indexed_notes_and_properties_but_ignores_chat_snapshots() {
    let (_dir, graph) = setup();
    for name in ["block.zip", "prop.png", "chat.zip", "chat-source.zip", "chat-context.zip"] {
        put(&graph, &format!("assets/{name}"), b"data");
    }
    let page = graph.db.create_page("Indexed", false).unwrap();
    graph
        .db
        .create_block(
            &page.id,
            None,
            0,
            "[zip](../assets/block.zip)",
            BlockType::Text,
            serde_json::json!({"cover": "../assets/prop.png"}),
        )
        .unwrap();
    let conn = graph.db.conn().unwrap();
    conn.execute(
        "INSERT INTO chat_threads (id,title,context_json,created_at,updated_at)
         VALUES ('t','Chat','{\"asset\":\"assets/chat-context.zip\"}',0,0)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO chat_messages (id,thread_id,position,role,content,sources_json,created_at)
        VALUES ('m','t',0,'user','[archive](../assets/chat.zip)','[{\"path\":\"assets/chat-source.zip\"}]',0)", []).unwrap();
    conn.execute("INSERT INTO chat_messages (id,thread_id,position,role,content,created_at)
        VALUES ('reply','t',1,'assistant','Quoted: [archive](../assets/chat.zip)',0)", []).unwrap();
    assert_eq!(names(&graph), vec!["assets/chat-context.zip", "assets/chat-source.zip", "assets/chat.zip"]);
    let scan = graph.scan_unused_assets().unwrap();
    let moved = graph.trash_unused_assets(&scan.graph_path, &scan.assets).unwrap();
    assert!(moved.errors.is_empty());
    assert_eq!(moved.moved.len(), 3);
    let trash = graph.list_asset_trash().unwrap();
    let purged = graph.purge_trashed_assets(&trash.graph_path, &trash.assets).unwrap();
    assert!(purged.errors.is_empty());
    assert_eq!(purged.purged.len(), 3);
    assert!(graph.db.load_chat_thread("t").unwrap().is_some());
    assert!(graph.root_dir.join("assets/block.zip").exists());
    assert!(graph.root_dir.join("assets/prop.png").exists());
}

#[test]
fn chat_excerpt_saved_as_a_note_protects_its_attachment() {
    let (_dir, graph) = setup();
    put(&graph, "assets/chat.zip", b"archive");
    put(&graph, "pages/Saved conversation.md", b"- Quoted answer: [archive](../assets/chat.zip)\n");
    assert!(names(&graph).is_empty());
}

#[test]
fn asset_lifetime_checks_never_query_chat_storage() {
    let (_dir, graph) = setup();
    let page = graph.create_page_with_content("Only note", false, "- [file](../assets/archive.zip)\n").unwrap();
    put(&graph, "assets/archive.zip", b"archive");
    let block = graph.db.list_blocks_for_page(&page.id).unwrap().remove(0);
    // In this isolated fixture, any attempted chat query must fail.
    graph.db.conn().unwrap().execute_batch("DROP TABLE chat_messages; DROP TABLE chat_threads;").unwrap();
    assert!(graph.scan_unused_assets().unwrap().assets.is_empty());
    graph.delete_block(&block.id).unwrap();
    assert!(!graph.root_dir.join("assets/archive.zip").exists());
    let trash = graph.list_asset_trash().unwrap();
    assert_eq!(trash.assets.len(), 1);
    let purged = graph.purge_trashed_assets(&trash.graph_path, &trash.assets).unwrap();
    assert!(purged.errors.is_empty());
    assert_eq!(purged.purged.len(), 1);
}

#[test]
fn moves_selected_files_with_verified_recovery_and_never_rescans_trash() {
    let (_dir, graph) = setup();
    put(&graph, "assets/theme.zip", b"zip bytes");
    put(&graph, "pages/book/assets/keep.bin", b"keep");
    let scan = graph.scan_unused_assets().unwrap();
    let result = graph
        .trash_unused_assets(&scan.graph_path, &scan.assets[..1])
        .unwrap();
    assert!(result.errors.is_empty());
    assert_eq!(result.moved, vec!["assets/theme.zip"]);
    assert!(!graph.root_dir.join("assets/theme.zip").exists());
    assert_eq!(
        fs::read(Path::new(result.trash_path.as_ref().unwrap()).join("assets/theme.zip")).unwrap(),
        b"zip bytes"
    );
    assert_eq!(names(&graph), vec!["pages/book/assets/keep.bin"]);
    let again = graph
        .trash_unused_assets(&scan.graph_path, &scan.assets[..1])
        .unwrap();
    assert!(again.moved.is_empty());
    assert_eq!(again.errors.len(), 1);
}

#[test]
fn refuses_changed_referenced_missing_and_foreign_graph_candidates() {
    let (_dir, graph) = setup();
    for name in ["changed.zip", "linked.zip", "missing.zip", "valid.zip"] {
        put(&graph, &format!("assets/{name}"), b"before");
    }
    let scan = graph.scan_unused_assets().unwrap();
    put(&graph, "assets/changed.zip", b"after!");
    put(
        &graph,
        "journals/external.md",
        b"[attachment](../assets/linked.zip)",
    );
    fs::remove_file(graph.root_dir.join("assets/missing.zip")).unwrap();
    assert!(graph
        .trash_unused_assets("another graph", &scan.assets)
        .is_err());
    let result = graph
        .trash_unused_assets(&scan.graph_path, &scan.assets)
        .unwrap();
    assert_eq!(result.moved, vec!["assets/valid.zip"]);
    assert_eq!(result.errors.len(), 3);
    assert_eq!(
        fs::read(graph.root_dir.join("assets/changed.zip")).unwrap(),
        b"after!"
    );
    assert!(graph.root_dir.join("assets/linked.zip").exists());
}

#[test]
fn rejects_crafted_paths_and_protects_source_documents() {
    let (_dir, graph) = setup();
    put(&graph, "pages/note.md", b"note");
    put(&graph, "assets/file.zip", b"zip");
    let scan = graph.scan_unused_assets().unwrap();
    let mut requested = Vec::new();
    for path in [
        "../outside",
        "/assets/file.zip",
        "assets/../pages/note.md",
        "pages/note.md",
        ".grafium/index.db",
    ] {
        let mut asset = scan.assets[0].clone();
        asset.filename = path.into();
        requested.push(asset);
    }
    let result = graph
        .trash_unused_assets(&scan.graph_path, &requested)
        .unwrap();
    assert!(result.moved.is_empty());
    assert_eq!(result.errors.len(), requested.len());
    assert!(graph.root_dir.join("pages/note.md").exists());
}

#[test]
fn invalid_source_encoding_fails_closed() {
    let (_dir, graph) = setup();
    put(&graph, "assets/file.zip", b"zip");
    put(&graph, "pages/broken.md", &[0xff]);
    assert!(graph
        .scan_unused_assets()
        .unwrap_err()
        .to_string()
        .contains("broken.md"));
}

#[test]
fn hidden_source_and_html_entities_still_protect_assets() {
    let (_dir, graph) = setup();
    put(&graph, "assets/copyright\u{a9}.png", b"image");
    put(&graph, "assets/a b.zip", b"zip");
    put(
        &graph,
        ".notes/hidden.html",
        b"<img src='../assets/copyright&copy;.png'><a href='../assets/a&#x20;b.zip'>file</a>",
    );
    assert!(names(&graph).is_empty());
}

#[test]
fn failed_recovery_directory_leaves_original_and_reports_error() {
    let (_dir, graph) = setup();
    put(&graph, "assets/file.zip", b"zip");
    let scan = graph.scan_unused_assets().unwrap();
    put(&graph, ".grafium/asset-trash", b"not a directory");
    let result = graph
        .trash_unused_assets(&scan.graph_path, &scan.assets)
        .unwrap();
    assert!(result.moved.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert_eq!(
        fs::read(graph.root_dir.join("assets/file.zip")).unwrap(),
        b"zip"
    );
}

#[test]
fn cleanup_fences_in_flight_sync_and_deduplicates_selection() {
    let (_dir, graph) = setup();
    put(&graph, "assets/file.zip", b"zip");
    let fence = crate::fsutil::source_mutation_fence(&graph.root_dir, Path::new("assets/file.zip"))
        .unwrap();
    let scan = graph.scan_unused_assets().unwrap();
    let requested = vec![scan.assets[0].clone(), scan.assets[0].clone()];
    let result = graph
        .trash_unused_assets(&scan.graph_path, &requested)
        .unwrap();
    assert_eq!(result.moved, vec!["assets/file.zip"]);
    assert!(result.errors.is_empty());
    assert!(!fence.is_current());
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_sources_assets_and_trash() {
    use std::os::unix::fs::symlink;
    for link in [
        "pages/linked.md",
        "assets/linked.zip",
        ".grafium/asset-trash",
    ] {
        let (_dir, graph) = setup();
        let external = TempDir::new().unwrap();
        put(&graph, "assets/file.zip", b"zip");
        let scan = graph.scan_unused_assets().unwrap();
        symlink(external.path(), graph.root_dir.join(link)).unwrap();
        if link == ".grafium/asset-trash" {
            let result = graph
                .trash_unused_assets(&scan.graph_path, &scan.assets)
                .unwrap();
            assert!(result.moved.is_empty());
            assert_eq!(result.errors.len(), 1);
        } else {
            assert!(graph.scan_unused_assets().is_err());
        }
        assert!(graph.root_dir.join("assets/file.zip").exists());
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
    }
}
