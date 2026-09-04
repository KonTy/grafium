mod commands;

// Android-only JNI bridge: exposes grafium_core::assistant::handle_command as
// `Java_com_grafium_app_AssistantReceiver_nativeHandleCommand` so the Kotlin
// receiver can share the same NLU as the desktop Tauri command above.
#[cfg(target_os = "android")]
mod android_jni;

use commands::graph::GraphConfig;
use grafium_core::Graph;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

pub struct AppState {
    pub graph: Arc<Mutex<Graph>>,
    watcher: Mutex<Option<GraphWatcherHandle>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GraphRuntimeSnapshot {
    pub root_dir: PathBuf,
    pub db_path: PathBuf,
    pub metadata_dir_name: String,
}

struct GraphWatcherHandle {
    stop_tx: mpsc::Sender<()>,
    join_handle: thread::JoinHandle<()>,
}

impl GraphWatcherHandle {
    fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join_handle.join();
    }
}

impl AppState {
    pub fn restart_graph_watcher(&self) -> Result<(), String> {
        if let Ok(mut guard) = self.watcher.lock() {
            if let Some(existing) = guard.take() {
                existing.stop();
            }
        }

        let (pages_dir, journals_dir, self_writes) = {
            let graph = self.graph.lock().map_err(|e| e.to_string())?;
            (
                graph.pages_dir.clone(),
                graph.journals_dir.clone(),
                graph.self_write_tracker(),
            )
        };

        let handle = start_graph_watcher(self.graph.clone(), pages_dir, journals_dir, self_writes)?;
        let mut guard = self.watcher.lock().map_err(|e| e.to_string())?;
        *guard = Some(handle);
        Ok(())
    }
}

pub(crate) fn snapshot_then<T, S, R>(
    lock: &Mutex<T>,
    snapshot: impl FnOnce(&T) -> Result<S, String>,
    run: impl FnOnce(S) -> Result<R, String>,
) -> Result<R, String> {
    let snapshot = {
        let guard = lock.lock().map_err(|e| e.to_string())?;
        snapshot(&guard)?
    };
    run(snapshot)
}

pub(crate) fn current_graph_snapshot(
    app: &tauri::AppHandle,
    graph: &Mutex<Graph>,
) -> Result<GraphRuntimeSnapshot, String> {
    snapshot_then(
        graph,
        |graph| {
            Ok(GraphRuntimeSnapshot {
                root_dir: graph.root_dir.clone(),
                db_path: platform_db_path(app, &graph.root_dir),
                metadata_dir_name: metadata_dir_name(app),
            })
        },
        |snapshot| Ok(snapshot),
    )
}

pub(crate) fn open_graph_snapshot(snapshot: &GraphRuntimeSnapshot) -> Result<Graph, String> {
    Graph::open_with_db_path_and_metadata_dir(
        &snapshot.root_dir,
        &snapshot.db_path,
        &snapshot.metadata_dir_name,
    )
    .map_err(|e| e.to_string())
}

fn should_process_event(
    event: &Event,
    pages_dir: &std::path::Path,
    journals_dir: &std::path::Path,
) -> bool {
    event.paths.iter().any(|p| {
        p.extension().and_then(|e| e.to_str()) == Some("md")
            && (p.starts_with(pages_dir) || p.starts_with(journals_dir))
    })
}

/// Returns true if `path` was written by the app itself within the last few
/// seconds. Used to ignore self-inflicted filesystem events so a normal block
/// save doesn't get mistaken for an external edit.
fn was_recent_self_write(self_writes: &Arc<Mutex<HashMap<PathBuf, Instant>>>, path: &Path) -> bool {
    if let Ok(mut map) = self_writes.lock() {
        let now = Instant::now();
        map.retain(|_, t| now.duration_since(*t).as_secs() < 30);
        if let Some(t) = map.get(path) {
            return now.duration_since(*t).as_secs() < 10;
        }
    }
    false
}

fn start_graph_watcher(
    graph: Arc<Mutex<Graph>>,
    pages_dir: PathBuf,
    journals_dir: PathBuf,
    self_writes: Arc<Mutex<HashMap<PathBuf, Instant>>>,
) -> Result<GraphWatcherHandle, String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();

    let join_handle = thread::spawn(move || {
        let mut watcher = match RecommendedWatcher::new(
            move |res| {
                let _ = event_tx.send(res);
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("watcher init failed: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&pages_dir, RecursiveMode::Recursive) {
            eprintln!("watch pages dir failed: {}", e);
            return;
        }
        if let Err(e) = watcher.watch(&journals_dir, RecursiveMode::Recursive) {
            eprintln!("watch journals dir failed: {}", e);
            return;
        }

        let debounce = Duration::from_millis(400);
        let mut pending_files = std::collections::HashSet::<PathBuf>::new();
        let mut last_event_at: Option<Instant> = None;

        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(event)) => {
                    if !should_process_event(&event, &pages_dir, &journals_dir) {
                        continue;
                    }

                    for path in event.paths {
                        if path.extension().and_then(|e| e.to_str()) != Some("md") {
                            continue;
                        }
                        // Ignore writes the app just made itself. Without this,
                        // every block save (which rewrites the page's .md file)
                        // would be re-processed as an external change.
                        if was_recent_self_write(&self_writes, &path) {
                            continue;
                        }
                        pending_files.insert(path);
                    }

                    if !pending_files.is_empty() {
                        last_event_at = Some(Instant::now());
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("watch event error: {}", e);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let ready = last_event_at
                .map(|t| t.elapsed() >= debounce)
                .unwrap_or(false);

            if !ready {
                continue;
            }

            let g = match graph.lock() {
                Ok(guard) => guard,
                Err(e) => {
                    eprintln!("watch lock error: {}", e);
                    pending_files.clear();
                    last_event_at = None;
                    continue;
                }
            };

            // Incremental indexing only. We deliberately NEVER call
            // `reindex_all()` from the watcher: it runs `clear_all()` (wiping
            // the entire index) followed by a full disk rescan. On a large
            // graph that both freezes the app for a long time and — if the
            // on-disk .md files are not a complete mirror of the index — can
            // destroy data. Index only the changed files. Deletions are left
            // alone (a stale entry is harmless; an explicit re-index fixes it).
            for path in pending_files.drain() {
                if path.exists() {
                    if let Err(e) = g.index_file(&path) {
                        eprintln!("watch index file failed ({}): {}", path.display(), e);
                    }
                }
            }

            drop(g);
            pending_files.clear();
            last_event_at = None;
        }
    });

    Ok(GraphWatcherHandle {
        stop_tx,
        join_handle,
    })
}

/// Watch ~/.config/smplos/current/theme.name for changes and emit event to frontend
fn start_smplos_theme_watcher(app_handle: tauri::AppHandle) {
    let theme_path = match dirs::config_dir() {
        Some(d) => d.join("smplos/current/theme.name"),
        None => return,
    };

    if !theme_path.exists() {
        return;
    }

    thread::spawn(move || {
        use std::fs;

        let mut last_content = fs::read_to_string(&theme_path)
            .unwrap_or_default()
            .trim()
            .to_string();

        loop {
            thread::sleep(Duration::from_secs(2));

            let current = match fs::read_to_string(&theme_path) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };

            if current != last_content && !current.is_empty() {
                eprintln!(
                    "[theme-watcher] smplos theme changed: {} -> {}",
                    last_content, current
                );
                last_content = current.clone();
                let _ = app_handle.emit(
                    "smplos-theme-changed",
                    serde_json::json!({
                        "theme": current,
                    }),
                );
            }
        }
    });
}

/// Background thread that periodically checks if sync targets become available.
/// When a target that was unavailable becomes available, it emits a Tauri event
/// and optionally triggers auto-sync.
fn start_sync_monitor(app_handle: tauri::AppHandle, graph: Arc<Mutex<Graph>>) {
    use grafium_core::sync::{
        filesystem::FilesystemBackend,
        state::{BackendConfig, SyncConfigs},
        webdav::WebDavBackend,
        SyncEngine,
    };

    thread::spawn(move || {
        let mut was_available: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        let check_interval = Duration::from_secs(5);

        loop {
            thread::sleep(check_interval);

            let snapshot = match current_graph_snapshot(&app_handle, graph.as_ref()) {
                Ok(snapshot) => snapshot,
                Err(_) => continue,
            };

            let config_path = snapshot
                .root_dir
                .join(&snapshot.metadata_dir_name)
                .join("sync-config.json");
            let configs = SyncConfigs::load(&config_path);

            for target in &configs.targets {
                let backend: Box<dyn grafium_core::sync::SyncBackend> = match &target.config {
                    BackendConfig::Filesystem { path } => {
                        Box::new(FilesystemBackend::new(path.clone(), target.name.clone()))
                    }
                    BackendConfig::WebDav {
                        url,
                        username,
                        password,
                    } => match WebDavBackend::new(
                        url.clone(),
                        username.clone(),
                        password.clone(),
                        target.name.clone(),
                    ) {
                        Ok(backend) => Box::new(backend),
                        Err(err) => {
                            eprintln!(
                                "[sync-monitor] Skipping target '{}' because WebDAV backend initialization failed: {}",
                                target.name,
                                err
                            );
                            continue;
                        }
                    },
                };

                let now_available = backend.is_available();
                let previously_available = was_available.get(&target.id).copied().unwrap_or(false);

                if now_available && !previously_available {
                    // Target just became available!
                    eprintln!("[sync-monitor] Target '{}' is now available", target.name);

                    // Emit event to frontend
                    let _ = app_handle.emit(
                        "sync-target-available",
                        serde_json::json!({
                            "target_id": target.id,
                            "target_name": target.name,
                        }),
                    );

                    // Auto-sync if enabled
                    if target.auto_sync {
                        let engine = SyncEngine::new_with_metadata_dir(
                            snapshot.root_dir.clone(),
                            &snapshot.metadata_dir_name,
                        );
                        match engine.sync(backend.as_ref()) {
                            Ok(result) => {
                                eprintln!(
                                    "[sync-monitor] Auto-sync '{}': {}",
                                    target.name,
                                    result.summary()
                                );

                                // Reindex if we pulled files
                                if !result.pulled.is_empty()
                                    || !result.conflicts.is_empty()
                                    || !result.deleted_local.is_empty()
                                {
                                    if let Ok(detached_graph) = open_graph_snapshot(&snapshot) {
                                        let _ = detached_graph.reindex_all();
                                    }
                                    // Notify frontend to refresh
                                    let _ = app_handle.emit(
                                        "sync-completed",
                                        serde_json::json!({
                                            "target_name": target.name,
                                            "pushed": result.pushed.len(),
                                            "pulled": result.pulled.len(),
                                            "conflicts": result.conflicts.len(),
                                        }),
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "[sync-monitor] Auto-sync '{}' failed: {}",
                                    target.name, e
                                );
                                let _ = app_handle.emit(
                                    "sync-error",
                                    serde_json::json!({
                                        "target_name": target.name,
                                        "error": e.to_string(),
                                    }),
                                );
                            }
                        }
                    }
                } else if !now_available && previously_available {
                    // Target disconnected
                    let _ = app_handle.emit(
                        "sync-target-disconnected",
                        serde_json::json!({
                            "target_id": target.id,
                            "target_name": target.name,
                        }),
                    );
                }

                was_available.insert(target.id.clone(), now_available);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::snapshot_then;

    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn snapshot_then_releases_lock_before_running_work() {
        let shared = Arc::new(Mutex::new(41usize));
        let worker_shared = Arc::clone(&shared);
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            snapshot_then(
                worker_shared.as_ref(),
                |value| Ok(*value + 1),
                |snapshot| {
                    started_tx.send(snapshot).unwrap();
                    finish_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                    Ok(())
                },
            )
            .unwrap();
        });

        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 42);
        assert!(
            shared.try_lock().is_ok(),
            "expensive work should not run while holding the shared graph mutex"
        );

        finish_tx.send(()).unwrap();
        worker.join().unwrap();
    }
}

#[cfg(target_os = "android")]
fn stable_path_id(path: &std::path::Path) -> String {
    // Deterministic FNV-1a hash for filesystem-safe DB directory names.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn metadata_dir_name(app: &tauri::AppHandle) -> String {
    let raw = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| app.package_info().name.clone());

    let slug = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let normalized = if slug.is_empty() {
        "grafium".to_string()
    } else {
        slug
    };
    format!(".{}", normalized)
}

fn has_any_markdown(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                return true;
            }
        }
    }
    false
}

fn write_text_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, content).map_err(|e| e.to_string())
}

fn seed_tutorial_graph(graph_root: &Path, metadata_dir: &str) -> Result<bool, String> {
    let marker_v1 = graph_root.join(metadata_dir).join("tutorial-seeded-v1");
    let marker_v2 = graph_root.join(metadata_dir).join("tutorial-seeded-v2");
    let marker_v3 = graph_root.join(metadata_dir).join("tutorial-seeded-v3");
    let marker_v4 = graph_root.join(metadata_dir).join("tutorial-seeded-v4");
    let marker_v5 = graph_root.join(metadata_dir).join("tutorial-seeded-v5");
    let marker_v6 = graph_root.join(metadata_dir).join("tutorial-seeded-v6");
    let marker_v7 = graph_root.join(metadata_dir).join("tutorial-seeded-v7");
    let marker_v8 = graph_root.join(metadata_dir).join("tutorial-seeded-v8");
    let marker_v9 = graph_root.join(metadata_dir).join("tutorial-seeded-v9");
    let marker_v10 = graph_root.join(metadata_dir).join("tutorial-seeded-v10");
    let marker = graph_root.join(metadata_dir).join("tutorial-seeded-v11");
    if marker.exists() {
        return Ok(false);
    }

    let pages_dir = graph_root.join("pages");
    let journals_dir = graph_root.join("journals");
    let has_markdown = has_any_markdown(&pages_dir) || has_any_markdown(&journals_dir);
    // Allow one-time in-place refresh of the built-in tutorial graph from v1..v10 -> v11.
    if has_markdown
        && !marker_v1.exists()
        && !marker_v2.exists()
        && !marker_v3.exists()
        && !marker_v4.exists()
        && !marker_v5.exists()
        && !marker_v6.exists()
        && !marker_v7.exists()
        && !marker_v8.exists()
        && !marker_v9.exists()
        && !marker_v10.exists()
    {
        return Ok(false);
    }

    let start_here = r##"# Welcome To Grafium

Grafium is a **second brain** — a place to capture what you learn, connect it, and remember it for good. This is a safe tutorial graph, so you can practice without touching your real notes.

## ⭐ Start Here: Learn Anything, Remember It Forever

Grafium isn't just note storage — it's a system for **studying and remembering** any book, video, or lecture. Read these three pages in order:

1. [[The Grafium Study Method]] — why it works (CODE + PACER)
2. [[PACER - Tag What You Read]] — label each note so you know how to use it
3. [[The Study Loop]] — the exact steps: capture, digest, review
4. [[How To Study a Book]] — a full worked example (studying an economics chapter)

## Learn The App

- [[Try Block Editing]] — blocks, links, tags, and tasks
- [[How To Create A Flashcard]] — text, image, audio & video cards (+ import Anki decks)
- [[Create Your Own Graph]] — make your real graph in Documents

## Quick Editor Tips

> **Tip:**
> Click any block to edit it.
> Press `Enter` to create a new block.
> Press `Shift+Enter` to create a new line inside the same block.
> Type `/` to show commands.

## Important

This tutorial graph stays active until you create/switch to your own graph.
Your personal notes should live in your own graph folder.
"##;

    let create_graph_page = r##"# Create Your Own Graph

## Recommended Location

Use a folder under:

`~/Documents/grafium/`

Example:

`~/Documents/grafium/my-notes`

## Steps

1. Open the graph menu (top-left graph button).
2. Click **New Graph**.
3. Enter a graph name.
4. Pick a location in `Documents/grafium`.
5. Grafium creates structure automatically (`pages/`, `journals/`, and metadata).

## Switching Graphs

- Use graph menu to switch back/forth between tutorial and your graph.
- Once switched, Grafium remembers your choice.
"##;

    let block_editing_page = r##"# Try Block Editing

Practice these directly in this page:

- Press **Enter** to create a new block
- Press **Tab** to indent, **Shift+Tab** to outdent
- Write `TODO` at line start to create tasks
- Create a page link like `[[Ideas]]`
- Add a tag like `#project`

## Journal Tip

Open Journal view and right-click the date title to delete a journal page.

## Search Tip

Use search in sidebar to jump by page name or block content.
"##;

    let study_method_page = r##"# The Grafium Study Method

Most people try to learn by **consuming more** — reading faster, watching at 2x. But we forget up to **90%** of what we read. The fix isn't consuming more; it's **digesting** what you consume.

Grafium combines two proven ideas:

- **CODE / Second Brain** (Tiago Forte) — *where* notes live and how they link: Capture, Organize, Distill, Express.
- **PACER** (Justin Sung) — *how* to process each note so it sticks: Procedural, Analogous, Conceptual, Evidence, Reference.

> **The core rule:** Learning = **consume + digest**, and the two must stay **balanced**. If you can't digest what you're reading, slow down.

## The Two Stages

1. **Consume** — capture ideas fast in your daily [[Journal]], tagging each one.
2. **Digest** — later, turn the important notes into permanent, linked [[concept page]]s. That is what builds your second brain (and your graph).

## Watch The Source

This method is based on Justin Sung's video on how to remember what you read. Search YouTube for **Justin Sung — "How to remember everything you read"** to watch his full explanation of the PACER system.

## Next

- [[PACER - Tag What You Read]]
- [[The Study Loop]]
"##;

    let pacer_page = r##"# PACER - Tag What You Read

Not all information is equal. As you read, decide which of the **five PACER types** each note is, and tag it. That single decision *is* active learning — and it tells you how to digest the note later.

## The Five Types

- **P — Procedural** `#proc` — how to *do* something (a technique, steps, code).
  - Digest by: **practice it** as soon as you can.
- **A — Analogous** `#analogy` — it reminds you of something you already know.
  - Digest by: **critique the analogy** — where does it fit, where does it break?
- **C — Conceptual** `#concept` — the *what* and *why* (facts, theories, links).
  - Digest by: **map it** — create/link a concept page. This grows your graph.
- **E — Evidence** `#evidence` — an example, stat, or case that proves a concept.
  - Digest by: **store** it under its concept, then **rehearse**.
- **R — Reference** `#ref` — a nitty-gritty fact you may need later.
  - Digest by: **store** it as a flashcard — write it as `Question :: Answer`.

## Workflow Tags

Use these to find notes again:

- `#inbox` — captured, not yet digested
- `#rehearse` — needs active-recall practice
- `#flashcard` — should become a spaced-repetition card
- `#digested` — done; it now lives in a concept page

> **Tip:** Click any tag to see every note with it. That is how you build your review list.

## Next

- [[The Study Loop]]
"##;

    let study_loop_page = r##"# The Study Loop

Here is the exact step-by-step. Example: you're watching a video on learning.

## 1. Before (10 seconds)

Open today's **Journal** and add the source, then indent notes under it:

- [[Source - Justin Sung: How to Remember]]  #inbox
  - (your notes go here, indented)

## 2. Consume (while watching)

Capture each idea in your own words and **tag its PACER type**. Do not stop to memorize — just capture and tag:

- Learning = consume + [[digest]], keep it balanced  #concept
- Muscle contraction is like my swimming stroke  #analogy
- Up to 90% forgotten without digestion  #evidence
- Kim Peek had FG syndrome  #ref
- Draw a mind-map while reading conceptual info  #proc

> **Balance rule:** if you can't keep tagging, you're consuming too fast.

## 3. Digest (that evening)

Go back through today's journal and process each tag:

- `#concept` → open/create the [[concept page]] and link it to related ideas.
- `#analogy` → write *why* it fits and where it breaks.
- `#proc` → add a `TODO` to practice it.
- `#evidence` → move it under its concept, then tag `#rehearse`.
- `#ref` → turn it into a flashcard: write `Question :: Answer` (for example `Capital of France :: Paris`). Review it later in **Flashcards** (sidebar).

Change each note from `#inbox` to `#digested` as you finish.

## 4. Review (ongoing)

- **Flashcards (sidebar):** spaced-repetition review of every `Question :: Answer` card. Grade each recall (Again / Hard / Good / Easy) and Grafium schedules when you should see it next.
- **Rehearse list:** search the `#rehearse` tag to actively recall evidence notes.
- **Graph view:** find **orphan** notes (captured but never linked = not learned yet) and connect them; practice recall from your dense **hub** topics.

## That's The Whole System

Capture fast → tag with PACER → digest into linked concept pages → review by tag and graph. Do this for any topic and your knowledge compounds forever.

## See It In Action

- [[How To Study a Book]] — the same loop applied end-to-end to a real chapter.
"##;

    let study_book_page = r##"# How To Study a Book

This is a full worked example: studying the first chapter of an intro **economics** textbook using the Grafium study loop. It adapts the classic *"summarize in the margins"* reading method (progressive summarization) to your graph.

## The One Rule: Summarize, Never Copy

You only remember what you force your brain to process. Copying a sentence is passive — you can do it without understanding. **Summarizing in your own words is active** — you can only compress six sentences into one if you actually understood them. So for every chunk you read, you write **one sentence in your own words**. In Grafium, each summary is a block, and that block *is* your margin note.

## The Progressive Summary Trick

- Paragraph 1 → one block: a one-sentence summary of paragraph 1.
- Paragraph 2 → one block: a one-sentence summary of paragraph 2.
- Paragraph 3 and onward → **two** blocks: first a **rolling summary of everything so far**, then a summary of the new paragraph.

The rolling summary is where the magic is: it forces you to connect and compress every idea so far into one line, every few paragraphs. That act of synthesis is what actually builds memory.

## Step 1 — Set Up The Source (Journal)

Open today's **Journal**, add the source, tag it `#inbox`, and indent your notes under it:

- [[Source - Bernanke: Principles of Economics, Ch.1]]  #inbox
  - (your one-sentence summaries go here, indented)

## Step 2 — Read & Summarize (Consume)

Read one paragraph, then write one sentence in your own words and **tag its PACER type**. Every third block or so, make it a *rolling* summary instead:

- Economics = the study of how scarce resources get allocated  #concept
- "Scarce" just means finite — money, sand, and time are all scarce  #concept
- Rolling summary: economics studies who gets limited resources, and at what cost  #concept
- Opportunity cost = the value of the next-best thing you gave up  #concept
- Rolling summary: scarcity forces choices, and every choice has an opportunity cost  #concept

Notice the two `Rolling summary` lines — that is the paragraph-3 move: compress everything so far into one line *before* adding the new idea.

## Step 3 — Make Key Facts Stick (Flashcards)

Turn the definitions worth memorizing into flashcards right inside your notes, using the `Question :: Answer` syntax:

- Economics :: the study of the allocation of scarce resources  #economics
- Opportunity cost :: the value of the next-best alternative you gave up  #economics

Grafium turns these into spaced-repetition cards automatically — review them later in **Flashcards** (sidebar).

Tip: the `#economics` tag turns these cards into a **study topic**. In **Flashcards** you can drill just one topic (e.g. `#economics` or `#chinese`) or study **Mixed** — pulling due cards from every topic at once.

## Step 4 — Digest (That Evening)

Go back through the journal and turn your **rolling summaries** into a permanent [[concept page]]. Your last, best rolling summary basically *is* the distilled page:

- Open or create [[Economics]] and make your best rolling summary its opening line.
- Link the concepts it touches: [[Scarcity]], [[Opportunity Cost]], [[Allocation]].
- Change the source from `#inbox` to `#digested`.

## Step 5 — Review (Later)

- You **never reread the whole chapter** — you reread your one-line summaries.
- **Flashcards (sidebar):** grade recall on each `Question :: Answer` card.
- Search the `#rehearse` tag to actively recall the evidence you flagged.

## Why This Works

Every single step forces you to think through *meaning* — summarizing, connecting, and recalling — instead of passively passing your eyes over the page. That is the whole secret: no thinking, no memory.

## Watch The Source

This reading method comes from a well-known video by a philosophy professor on how to remember what you read. Search YouTube for **"how to remember what you read — summarize in the margins"** to watch the full explanation.

## Next

- [[The Study Loop]]
- [[PACER - Tag What You Read]]
"##;

    let flashcard_page = r##"# How To Create A Flashcard

Flashcards in Grafium are just blocks. Write a question and an answer on one line separated by ` :: ` and Grafium turns it into a spaced-repetition card automatically. Review your due cards any time from **Flashcards** in the sidebar.

## The Syntax

Write `Front :: Answer` in any block. The part before `::` is the front (the prompt); the part after is the back (the answer). Add a `#tag` to file the card into a study topic. Here are three real, reviewable cards:

- What is the capital of France? :: Paris  #geography
- Photosynthesis :: how plants convert light into chemical energy  #biology
- 7 × 8 :: 56  #math

Open **Flashcards** in the sidebar and you will see these appear under the `#geography`, `#biology`, and `#math` topics. Study one topic at a time, or pick **Mixed** to pull due cards from every topic at once.

## Text Cards With Rich Answers

The answer can contain normal markdown — **bold**, *italics*, `code`, and math. This is one single card (keep the whole card on one line):

- What does $E = mc^2$ describe? :: the equivalence of **energy** and **mass**, where $c$ is the speed of light  #physics

## Image Cards

Add a picture to a card with image syntax: `![](path)`. Point it at a file in your graph's `assets` folder. This card shows a diagram on the back:

- What shape is this? :: A rounded card sample → ![](../assets/tutorial/flashcard-demo.svg)  #demo

Any local image works — PNG, JPG, GIF, WebP, or SVG. Drop the file into your graph's `assets` folder and reference it as `../assets/<your-file>`.

## Audio Cards

Use the same `![](path)` syntax with an audio file (`.mp3`, `.wav`, `.ogg`, `.m4a`, `.opus`, `.flac`). Grafium renders a little audio player right on the card — perfect for language pronunciation or ear training:

`- How do you say "hello" in Mandarin? :: 你好 (nǐ hǎo) ![](../assets/audio/nihao.mp3)  #chinese`

(The line above is shown as code because this tutorial graph doesn't ship an audio file — but the syntax is exactly that. Imported Anki language decks bring their pronunciation audio automatically.)

## Video Cards

Video works too (`.mp4`, `.webm`, `.mov`, `.mkv`). Great for a golf swing, a chemistry reaction, or a sign-language sign:

`- Show the correct kettlebell swing :: ![](../assets/video/swing.mp4)  #fitness`

## Import A Whole Anki Deck

Already have an Anki deck? In **Flashcards** (sidebar), click **Import Anki deck** and pick a `.apkg` file. Grafium converts every note into a `Front :: Back` card on a new page, files them under a topic named after the deck, and copies the deck's audio and images into your graph so they play right on the card.

## Tips

- Keep each card on **one physical line** — a new line starts a new block (and a new card).
- The `::` must have a space on each side: `Front :: Back`, not `Front::Back`.
- Cards become reviewable as soon as the page is saved.
- Tag every card (`#topic`) so you can study by subject.

## Next

- [[The Study Loop]]
- [[How To Study a Book]]
"##;

    write_text_file(&graph_root.join("pages/Welcome To Grafium.md"), start_here)?;
    write_text_file(
        &graph_root.join("pages/Create Your Own Graph.md"),
        create_graph_page,
    )?;
    write_text_file(
        &graph_root.join("pages/Try Block Editing.md"),
        block_editing_page,
    )?;
    write_text_file(
        &graph_root.join("pages/The Grafium Study Method.md"),
        study_method_page,
    )?;
    write_text_file(
        &graph_root.join("pages/PACER - Tag What You Read.md"),
        pacer_page,
    )?;
    write_text_file(&graph_root.join("pages/The Study Loop.md"), study_loop_page)?;
    write_text_file(
        &graph_root.join("pages/How To Study a Book.md"),
        study_book_page,
    )?;
    write_text_file(
        &graph_root.join("pages/How To Create A Flashcard.md"),
        flashcard_page,
    )?;

    // Seed a tiny self-contained SVG so the image-card demo renders out of the box.
    let demo_svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="140" viewBox="0 0 240 140"><rect x="6" y="6" width="228" height="128" rx="16" fill="#1e293b" stroke="#7dd3fc" stroke-width="3"/><text x="120" y="66" font-family="sans-serif" font-size="20" fill="#7dd3fc" text-anchor="middle">Flashcard</text><text x="120" y="96" font-family="sans-serif" font-size="13" fill="#94a3b8" text-anchor="middle">image demo</text></svg>"##;
    write_text_file(
        &graph_root.join("assets/tutorial/flashcard-demo.svg"),
        demo_svg,
    )?;

    write_text_file(&marker, "seeded_v11")?;
    Ok(true)
}

fn platform_db_path(app: &tauri::AppHandle, graph_root: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "android")]
    {
        let app_data = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("/data/local/tmp"));
        let id = stable_path_id(graph_root);
        return app_data.join("graph_indexes").join(id).join("index.db");
    }

    #[cfg(not(target_os = "android"))]
    {
        graph_root.join(metadata_dir_name(app)).join("index.db")
    }
}

/// Serves local graph assets (images, audio, video) to the webview through the
/// custom `grafium-asset://localhost/<relative-path>` scheme.
///
/// The path is resolved against the active graph's root directory. Requests are
/// confined to that directory (path-traversal attempts are rejected) so the
/// scheme can only read files inside the current graph.
fn asset_scheme_handler(
    app: &tauri::AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    // Extract and percent-decode the request path (strip leading '/').
    let raw_path = request.uri().path().trim_start_matches('/');
    let decoded = match urlencoding::decode(raw_path) {
        Ok(d) => d.into_owned(),
        Err(_) => return not_found(),
    };

    // Reject absolute paths and any traversal component up-front.
    if decoded.is_empty() || decoded.starts_with('/') || decoded.split('/').any(|c| c == "..") {
        return not_found();
    }

    // Resolve against the active graph root.
    let root = match app.try_state::<AppState>() {
        Some(state) => match state.graph.lock() {
            Ok(g) => g.root_dir.clone(),
            Err(_) => return not_found(),
        },
        None => return not_found(),
    };

    let candidate = root.join(&decoded);

    // Canonicalize both and confirm the target stays within the graph root.
    let (canon_root, canon_target) = match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(r), Ok(t)) => (r, t),
        _ => return not_found(),
    };
    if !canon_target.starts_with(&canon_root) {
        return not_found();
    }

    let bytes = match std::fs::read(&canon_target) {
        Ok(b) => b,
        Err(_) => return not_found(),
    };

    let mime = mime_for_path(&canon_target);
    let total = bytes.len() as u64;

    // WebKitGTK's media backend requires Range support for <audio>/<video>
    // playback — without it the element fails to load ("error"). Honor a single
    // byte-range request and always advertise Accept-Ranges.
    let range = request
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| parse_byte_range(h, total));

    if let Some((start, end)) = range {
        let slice = bytes[start as usize..=end as usize].to_vec();
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header("Content-Type", mime)
            .header("Accept-Ranges", "bytes")
            .header("Content-Range", format!("bytes {start}-{end}/{total}"))
            .header("Content-Length", (end - start + 1).to_string())
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "public, max-age=31536000, immutable")
            .body(slice)
            .unwrap_or_else(|_| not_found());
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", total.to_string())
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "public, max-age=31536000, immutable")
        .body(bytes)
        .unwrap_or_else(|_| not_found())
}

/// Parse a single `Range: bytes=START-END` header into an inclusive, clamped
/// (start, end) pair. Supports open-ended (`bytes=500-`) and suffix
/// (`bytes=-500`) forms. Returns None for an empty file or an unsatisfiable range.
fn parse_byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (s, e) = spec.split_once('-')?;
    let (start, end) = if s.is_empty() {
        // Suffix range: the last N bytes.
        let n: u64 = e.parse().ok()?;
        if n == 0 {
            return None;
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let start: u64 = s.parse().ok()?;
        let end: u64 = if e.is_empty() {
            total - 1
        } else {
            e.parse::<u64>().ok()?.min(total - 1)
        };
        (start, end)
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}

/// Best-effort MIME type from a file extension for the asset scheme.
pub(crate) fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        // Video
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Without this, every `tracing::info!`/`tracing::warn!` call throughout
    // the codebase (including grafium-core) silently went nowhere -- a real
    // observability gap that hampered debugging the OOM-crash investigation.
    // `RUST_LOG` still overrides the default if set; otherwise `info` is a
    // reasonable default for a desktop app (not so verbose it drowns out
    // the signal, but enough to see lifecycle/AI-provider/page-load events).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // WebKitGTK on Wayland aborts with "Error 71 (Protocol error)" on some
    // GPU/compositor setups when the DMABUF renderer / accelerated compositing
    // is active. Disable them before the webview initializes so the app launches
    // reliably from any entry point (start menu, terminal, packaged binary)
    // without depending on an external wrapper script to set these.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("grafium-asset", |ctx, request| {
            asset_scheme_handler(ctx.app_handle(), request)
        })
        .setup(|app| {
            let app_dir = app.path().app_data_dir().expect("Failed to get app data dir");
            let config_path = app_dir.join("graphs.json");
            let config = GraphConfig::load(&config_path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            // Use last-used graph, or fall back to default graph dir.
            // Validate the saved path before trusting it — if it no longer has a proper
            // graph structure, fall back to the default directory rather than creating
            // subdirectories inside an arbitrary user folder.
            let default_graph_dir = app_dir.join("tutorial-graph");
            let graph_dir = if let Some(ref current) = config.current {
                let candidate = PathBuf::from(current);
                let validation = Graph::validate_structure_with_metadata_dir(
                    &candidate,
                    &metadata_dir_name(app.handle()),
                );
                if validation.is_valid {
                    candidate
                } else {
                    eprintln!(
                        "Warning: saved graph path '{}' is no longer valid ({}), falling back to default",
                        current,
                        validation.error_message.as_deref().unwrap_or("unknown error")
                    );
                    default_graph_dir.clone()
                }
            } else {
                default_graph_dir.clone()
            };

            let db_path = platform_db_path(app.handle(), &graph_dir);
            let metadata_dir = metadata_dir_name(app.handle());
            let graph = Graph::open_with_db_path_and_metadata_dir(
                &graph_dir,
                &db_path,
                &metadata_dir,
            )
                .expect("Failed to initialize graph");

            let should_seed_tutorial = graph_dir == default_graph_dir;
            if should_seed_tutorial {
                if let Ok(true) = seed_tutorial_graph(&graph_dir, &metadata_dir) {
                    let _ = graph.reindex_all();
                }
            }

            // Keep startup responsive. If DB is empty (first run or recovered),
            // rebuild in the background instead of blocking app initialization.
            // Use a cheap existence probe — a full page listing here would scan
            // the whole table and freeze the UI thread on very large graphs.
            let page_count = if graph.db.has_any_page().unwrap_or(false) { 1 } else { 0 };
            if page_count == 0 {
                let graph_dir_clone = graph_dir.clone();
                let db_path_clone = db_path.clone();
                let metadata_dir_clone = metadata_dir.clone();
                thread::spawn(move || {
                    match Graph::open_with_db_path_and_metadata_dir(
                        &graph_dir_clone,
                        &db_path_clone,
                        &metadata_dir_clone,
                    ) {
                        Ok(g) => {
                            if let Err(e) = g.reindex_all() {
                                eprintln!(
                                    "Warning: background startup reindex failed for '{}': {}",
                                    graph_dir_clone.display(),
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: background startup reindex could not open '{}': {}",
                                graph_dir_clone.display(),
                                e
                            );
                        }
                    }
                });
            }

            // One-time (per graph): map existing FTS rows to their rowids.
            // `fts_blocks.block_id` is UNINDEXED, so deleting a block's FTS row
            // by block_id full-scans the whole index — seconds per edit on a
            // large graph, which froze the UI. `fts_block_rowid` lets edits
            // delete by rowid instead. Backfill runs in the background so it
            // never blocks startup; edits work meanwhile (only not-yet-mapped
            // legacy blocks use the slow path until the backfill reaches them).
            {
                let db_path_str = db_path.to_string_lossy().to_string();
                thread::spawn(move || match grafium_core::Database::new(&db_path_str) {
                    Ok(db) => match db.backfill_fts_rowid_map() {
                        Ok(0) => {}
                        Ok(n) => eprintln!("fts rowid map backfill: mapped {n} blocks"),
                        Err(e) => eprintln!("Warning: fts rowid map backfill failed: {}", e),
                    },
                    Err(e) => {
                        eprintln!("Warning: fts rowid map backfill could not open db: {}", e)
                    }
                });
            }

            // Register default graph in config if not present
            let mut config = config;
            let path_str = graph_dir.to_string_lossy().to_string();
            let name = if should_seed_tutorial {
                "Tutorial Graph".to_string()
            } else {
                graph_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("graph")
                    .to_string()
            };

            if let Some(existing) = config.graphs.iter_mut().find(|g| g.path == path_str) {
                existing.name = name.clone();
            } else {
                config.add_graph(&name, &path_str);
            }
            if config.current.is_none() {
                config.current = Some(path_str);
            }
            config
                .save(&config_path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            let state = AppState {
                graph: Arc::new(Mutex::new(graph)),
                watcher: Mutex::new(None),
            };
            state.restart_graph_watcher().expect("Failed to start graph watcher");

            // Start sync monitor (checks for USB/mount availability)
            let sync_graph = state.graph.clone();
            let sync_app_handle = app.handle().clone();
            start_sync_monitor(sync_app_handle, sync_graph);

            // Start smplos theme watcher
            let theme_app_handle = app.handle().clone();
            start_smplos_theme_watcher(theme_app_handle);

            app.manage(state);

            // Initialize Knowledge Engine
            let knowledge_state = {
                let data_dir = app_dir.join("knowledge");
                let config_path = data_dir.join("ai_config.json");
                let ai_config = if config_path.exists() {
                    std::fs::read_to_string(&config_path)
                        .ok()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default()
                } else {
                    grafium_core::ai::config::AiConfig::default()
                };
                let engine = grafium_core::KnowledgeEngine::new(&data_dir, ai_config)
                    .map(|e| e.with_models_root(app_dir.clone()))
                    .ok();
                commands::knowledge::KnowledgeState {
                    engine: Arc::new(tokio::sync::RwLock::new(engine)),
                }
            };
            app.manage(knowledge_state);

            // On Linux, intercept Ctrl+Z/Shift+Z at the GtkWindow level
            // WebKitGTK intercepts these keys internally before JS sees them,
            // and the WebView widget signal doesn't fire. By connecting to the
            // toplevel GtkWindow, we intercept BEFORE WebKitGTK processes them.
            #[cfg(target_os = "linux")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    let win_for_eval = window.clone();
                    if let Err(err) = window.with_webview(move |webview| {
                        use gtk::prelude::*;

                        let wk_webview = webview.inner();
                        // Get the toplevel GtkWindow - key events go here first
                        if let Some(toplevel) = wk_webview.toplevel() {
                            if let Ok(gtk_window) = toplevel.downcast::<gtk::Window>() {
                                let eval_window = win_for_eval.clone();
                                gtk_window.connect_key_press_event(move |_, event| {
                                    let state = event.state();
                                    let keyval = event.keyval();
                                    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                                    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                                    eprintln!("[GTK-WINDOW] key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

                                    // Ctrl+. toggles reference panel
                                    if ctrl && !shift && *keyval == 46 /* period */ {
                                        eprintln!("[GTK-WINDOW] => Ctrl+. detected, toggling reference panel");
                                        let _ = eval_window.eval("window.__toggleReferencePanel && window.__toggleReferencePanel()");
                                        return gtk::glib::Propagation::Stop;
                                    }

                                    if ctrl && !shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                        eprintln!("[GTK-WINDOW] => Ctrl+Z detected, calling eval(__handleNativeUndo)");
                                        // Call undo AND capture console output via a self-reporting mechanism
                                        let result = eval_window.eval(r#"
                                            (function() {
                                                var msg = 'EVAL_OK: __handleNativeUndo exists=' + (typeof window.__handleNativeUndo) + ' activeView=' + !!(window.__activeEditorView);
                                                var el = document.getElementById('__dbg');
                                                if (el) { el.innerHTML += '<br>' + msg; }
                                                if (window.__handleNativeUndo) {
                                                    window.__handleNativeUndo();
                                                } else {
                                                    document.title = 'ERROR: __handleNativeUndo not found!';
                                                }
                                            })();
                                        "#);
                                        eprintln!("[GTK-WINDOW] => eval result: {:?}", result);
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    if ctrl && shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                        eprintln!("[GTK-WINDOW] => Ctrl+Shift+Z detected, calling eval(__handleNativeRedo)");
                                        let _ = eval_window.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    if ctrl && (keyval == gdk::keys::constants::y || keyval == gdk::keys::constants::Y) {
                                        eprintln!("[GTK-WINDOW] => Ctrl+Y detected, calling eval(__handleNativeRedo)");
                                        let _ = eval_window.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    gtk::glib::Propagation::Proceed
                                });
                            } else {
                                eprintln!("Warning: Linux shortcut setup skipped because the webview toplevel is not a GtkWindow");
                            }
                        } else {
                            eprintln!("Warning: Linux shortcut setup skipped because the webview toplevel is unavailable");
                        }

                        // ALSO connect directly on the WebView widget itself
                        let eval_window2 = win_for_eval.clone();
                        wk_webview.connect_key_press_event(move |_, event| {
                            let state = event.state();
                            let keyval = event.keyval();
                            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                            eprintln!("[GTK-WEBVIEW] key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

                            if ctrl && !shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                eprintln!("[GTK-WEBVIEW] => Ctrl+Z detected, calling eval(__handleNativeUndo)");
                                let _ = eval_window2.eval("window.__handleNativeUndo && window.__handleNativeUndo()");
                                return gtk::glib::Propagation::Stop;
                            }
                            if ctrl && shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                eprintln!("[GTK-WEBVIEW] => Ctrl+Shift+Z detected");
                                let _ = eval_window2.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                return gtk::glib::Propagation::Stop;
                            }
                            if ctrl && (keyval == gdk::keys::constants::y || keyval == gdk::keys::constants::Y) {
                                eprintln!("[GTK-WEBVIEW] => Ctrl+Y detected");
                                let _ = eval_window2.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                return gtk::glib::Propagation::Stop;
                            }
                            gtk::glib::Propagation::Proceed
                        });
                    }) {
                        eprintln!(
                            "Warning: failed to access the main webview for Linux shortcut setup: {}",
                            err
                        );
                    }
                } else {
                    eprintln!(
                        "Warning: Linux shortcut setup skipped because the main webview window is unavailable"
                    );
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pages::list_pages,
            commands::pages::count_pages,
            commands::pages::list_pages_window,
            commands::pages::list_journal_pages,
            commands::pages::get_page,
            commands::pages::create_page,
            commands::pages::update_page_meta,
            commands::pages::delete_page,
            commands::pages::get_parent_page,
            commands::pages::get_child_pages,
            commands::pages::search_page_titles,
            commands::blocks::list_blocks,
            commands::blocks::create_block,
            commands::blocks::update_block,
            commands::blocks::delete_block,
            commands::blocks::move_block,
            commands::blocks::reorder_blocks,
            commands::blocks::get_block_page_title,
            commands::blocks::search_fts,
            commands::links::get_backlinks,
            commands::tasks::list_tasks,
            commands::tasks::update_task_state,
            commands::tasks::cycle_task_state,
            commands::tasks::get_completion_counts,
            commands::tasks::get_completed_tasks,
            commands::tasks::get_open_tasks,
            commands::tasks::set_task_date,
            commands::assistant::handle_assistant_command,
            commands::flashcards::list_flashcards_due,
            commands::flashcards::list_flashcard_topics,
            commands::flashcards::list_all_flashcards,
            commands::flashcards::update_flashcard_review,
            commands::flashcards::grade_flashcard,
            commands::flashcards::import_anki_apkg,
            commands::favorites::add_favorite,
            commands::favorites::remove_favorite,
            commands::favorites::list_favorites,
            commands::favorites::record_page_open,
            commands::favorites::list_recent_pages,
            commands::query::run_query,
            commands::query::get_property_keys,
            commands::query::get_property_values,
            commands::graph::get_graph_info,
            commands::graph::get_graph_data,
            commands::graph::list_graphs,
            commands::graph::open_graph,
            commands::graph::create_graph,
            commands::graph::validate_graph,
            commands::graph::reindex_current,
            commands::graph::remove_graph,
            commands::graph::get_app_version,
            commands::graph::list_directory,
            commands::graph::get_default_graph_base,
            commands::sync::sync_list_targets,
            commands::sync::sync_add_filesystem_target,
            commands::sync::sync_add_webdav_target,
            commands::sync::sync_remove_target,
            commands::sync::sync_check_status,
            commands::sync::sync_run,
            commands::sync::sync_run_all,
            commands::theme::get_smplos_theme,
            commands::theme::get_smplos_theme_colors,
            commands::theme::get_app_theme,
            commands::theme::set_app_theme,
            commands::assets::download_asset,
            commands::assets::list_assets,
            commands::assets::read_asset_data_url,
            commands::assets::find_orphaned_assets,
            commands::assets::delete_assets,
            commands::knowledge::ai_get_config,
            commands::knowledge::ai_set_config,
            commands::knowledge::ai_health_check,
            commands::knowledge::ai_index_page,
            commands::knowledge::ai_index_all_pages,
            commands::knowledge::ai_search,
            commands::knowledge::ai_generate_references,
            commands::knowledge::ai_summarize_selection,
            commands::knowledge::ai_research_web,
            commands::knowledge::text_wrap_known_terms,
            commands::knowledge::ai_insert_page_summary,
            commands::knowledge::ai_ask,
            commands::knowledge::ai_ask_stream,
            commands::knowledge::ai_list_registered_graphs,
            commands::knowledge::ai_register_graph,
            commands::knowledge::ai_list_schemas,
            commands::knowledge::ai_save_schema,
            commands::knowledge::ai_create_default_schemas,
            commands::media::media_import_video,
            commands::media::media_get_config,
            commands::media::media_set_config,
            commands::ui_log,
            commands::model_library::list_local_models,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
