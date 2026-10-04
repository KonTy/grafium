use crate::commands::jobs::{JobHandle, JobsState};
use crate::commands::knowledge::KnowledgeState;
use crate::private_reader::{ReaderKind, ReaderState};
use grafium_core::library_index::{
    cap_reason, extract_book_chunks_from_path, LibraryFileInput, LibraryIndexSettings,
    LibraryIndexStatus, LibraryIndexStore, LibraryItemInput, LibraryItemKind, LibrarySearchHit,
    LibrarySemanticState, LibraryTranscriptionState,
};
#[cfg(not(target_os = "android"))]
use grafium_core::library_index::{
    index_media_file_slices, MediaSlice, MediaSliceSource, MediaSliceTranscriber,
};
#[cfg(not(target_os = "android"))]
use grafium_core::media::Transcriber;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "android"))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

const LIBRARY_INDEX_EVENT: &str = "library-index-updated";
const LIBRARY_INDEX_JOB_KIND: &str = "library_index";
#[cfg(not(target_os = "android"))]
const MEDIA_SLICE_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChatAvailable {
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Default)]
pub struct LibraryIndexState {
    inner: Mutex<LibraryIndexInner>,
    ai_busy: AtomicUsize,
}

#[derive(Default)]
struct LibraryIndexInner {
    running_job: Option<String>,
    dirty: bool,
    pending_rebuild: bool,
    semantic_error: Option<String>,
    cache_cleaned: bool,
}

pub struct LibraryAiBusyGuard<'a>(&'a LibraryIndexState);

impl LibraryIndexState {
    pub fn ai_busy_guard(&self) -> LibraryAiBusyGuard<'_> {
        self.ai_busy.fetch_add(1, Ordering::SeqCst);
        LibraryAiBusyGuard(self)
    }
}

impl Drop for LibraryAiBusyGuard<'_> {
    fn drop(&mut self) {
        self.0.ai_busy.fetch_sub(1, Ordering::SeqCst);
    }
}

fn index_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("library-index")
        .join("index.sqlite"))
}

#[cfg(not(target_os = "android"))]
fn cache_workdir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("library-index")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn cache_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("library-index"))
}

fn cleanup_cache_root(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    std::fs::create_dir_all(path).map_err(|e| e.to_string())
}

fn store(app: &AppHandle) -> Result<LibraryIndexStore, String> {
    LibraryIndexStore::open(index_path(app)?).map_err(|e| e.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn library_index_status(
    app: AppHandle,
    state: State<'_, LibraryIndexState>,
    knowledge: State<'_, KnowledgeState>,
) -> Result<LibraryIndexStatus, String> {
    let (running, job_id) = {
        let inner = state.inner.lock().map_err(|e| e.to_string())?;
        (inner.running_job.is_some(), inner.running_job.clone())
    };
    status_for(&app, running, job_id, &knowledge).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn library_index_settings_set(
    app: AppHandle,
    state: State<'_, LibraryIndexState>,
    knowledge: State<'_, KnowledgeState>,
    enabled: bool,
    transcribe_media: bool,
) -> Result<LibraryIndexStatus, String> {
    let store = store(&app)?;
    store
        .set_settings(&LibraryIndexSettings {
            enabled,
            transcribe_media,
        })
        .map_err(|e| e.to_string())?;
    if !enabled {
        if let Some(job) = state
            .inner
            .lock()
            .map_err(|e| e.to_string())?
            .running_job
            .clone()
        {
            if let Some(jobs) = app.try_state::<JobsState>() {
                jobs.registry.request_cancel(&job);
            }
        }
    } else {
        schedule_delta_run(&app);
    }
    let status = status_for(&app, false, None, &knowledge).await?;
    let _ = app.emit(LIBRARY_INDEX_EVENT, &status);
    Ok(status)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn library_index_start(app: AppHandle, rebuild: bool) -> Result<String, String> {
    let (has_location, _) = library_inputs(&app).await?;
    if !has_location {
        return Err("Choose a Library folder before indexing".into());
    }
    start_indexing(app, rebuild, false)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn library_chat_available(
    knowledge: State<'_, KnowledgeState>,
) -> Result<LibraryChatAvailable, String> {
    let guard = knowledge.engine.read().await;
    let reason = guard
        .as_ref()
        .map(|engine| engine.library_chat_unavailable_reason())
        .unwrap_or_else(|| Some("Library questions need a chat model on this computer, so book and transcript text never leaves it. Choose one in Settings > AI.".into()));
    Ok(LibraryChatAvailable {
        available: reason.is_none(),
        reason,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn library_search(
    app: AppHandle,
    _knowledge: State<'_, KnowledgeState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<LibrarySearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let library_state = app.state::<LibraryIndexState>();
    let _busy = library_state.ai_busy_guard();
    let keyword_hits = store(&app)?
        .search(&query, limit.unwrap_or(30).clamp(1, 100), None)
        .map_err(|e| e.to_string())?;
    Ok(keyword_hits)
}

pub fn schedule_delta_run(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let _ = start_indexing(app, false, true);
    });
}

pub fn schedule_startup_delta(app: &AppHandle) {
    #[cfg(target_os = "android")]
    {
        let _ = app;
    }
    #[cfg(not(target_os = "android"))]
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let Ok((has_location, _)) = library_inputs(&app).await else {
                return;
            };
            if !has_location {
                return;
            }
            let _ = start_indexing(app, false, true);
        });
    }
}

/// `automatic` runs (opening Library, startup, settings changes) leave no
/// entry in the job history when they find nothing to do; a run the user
/// started always does.
fn start_indexing(app: AppHandle, rebuild: bool, automatic: bool) -> Result<String, String> {
    let settings = store(&app)?.settings().map_err(|e| e.to_string())?;
    if !settings.enabled {
        return Err("Library indexing is disabled".into());
    }
    let state = app.state::<LibraryIndexState>();
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if let Some(id) = inner.running_job.clone() {
            inner.dirty = true;
            inner.pending_rebuild |= rebuild;
            return Ok(id);
        }
        if !inner.cache_cleaned {
            cleanup_cache_root(&cache_root(&app)?)?;
            inner.cache_cleaned = true;
        }
        inner.running_job = Some("reserved".into());
        inner.pending_rebuild = false;
        inner.semantic_error = None;
    }
    let jobs = app.state::<JobsState>();
    let handle = match jobs.registry.start(
        app.clone(),
        LIBRARY_INDEX_JOB_KIND,
        "Indexing Library",
        true,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            state.inner.lock().map_err(|e| e.to_string())?.running_job = None;
            return Err(error);
        }
    };
    let job_id = handle.id().to_string();
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.running_job = Some(job_id.clone());
        inner.dirty = false;
    }
    tauri::async_runtime::spawn(run_job(app.clone(), handle, rebuild, automatic));
    Ok(job_id)
}

async fn run_job(app: AppHandle, handle: JobHandle, rebuild: bool, automatic: bool) {
    let result = run_indexing(app.clone(), handle.clone(), rebuild).await;
    let cancelled = handle.is_cancelled();
    let job_id = handle.id().to_string();
    match result {
        Ok(_) if cancelled => handle.cancelled(),
        Ok(0) if automatic => handle.discard(),
        Ok(_) => handle.succeeded_with_details("Library index updated", None, None::<String>),
        Err(_) if cancelled => handle.cancelled(),
        Err(error) => handle.failed(error),
    }
    let rerun = {
        let state = app.state::<LibraryIndexState>();
        let mut inner = state.inner.lock().unwrap_or_else(|p| p.into_inner());
        finish_job_state(&mut inner, &job_id, cancelled)
    };
    emit_status(&app).await;
    if let Some(pending_rebuild) = rerun {
        // A rebuild asked for during the run is the user's; a plain rerun is
        // the follow-up to changes noticed meanwhile.
        let _ = start_indexing(app, pending_rebuild, !pending_rebuild);
    }
}

fn finish_job_state(inner: &mut LibraryIndexInner, job_id: &str, cancelled: bool) -> Option<bool> {
    let rerun = !cancelled && inner.dirty;
    let pending_rebuild = !cancelled && inner.pending_rebuild;
    if inner.running_job.as_deref() == Some(job_id) {
        inner.running_job = None;
        inner.dirty = false;
        inner.pending_rebuild = false;
    }
    rerun.then_some(pending_rebuild)
}

/// What the Jobs panel shows for a Library run: which item of how many is
/// being worked on, how far into its media transcription is, and how many
/// items are done and left. The bar also moves within a long file instead of
/// sitting still for the hours a long video can take.
#[derive(Clone)]
struct RunProgress {
    handle: JobHandle,
    position: usize,
    total: usize,
    title: String,
}

impl RunProgress {
    const STEPS_PER_ITEM: usize = 1000;

    fn report(&self, action: &str, item_fraction: f64, detail: Option<String>) {
        let (done, total, message, details) =
            progress_parts(self.position, self.total, &self.title, action, item_fraction, detail);
        self.handle.progress_with_details(done, total, message, Some(details));
    }
}

/// The numbers and text behind [`RunProgress::report`], kept pure for tests.
fn progress_parts(
    position: usize,
    total: usize,
    title: &str,
    action: &str,
    item_fraction: f64,
    detail: Option<String>,
) -> (usize, usize, String, String) {
    let steps = RunProgress::STEPS_PER_ITEM;
    let within = (item_fraction.clamp(0.0, 0.999) * steps as f64) as usize;
    let left = total.saturating_sub(position);
    let counts = format!("{position} done, {left} left");
    (
        position * steps + within,
        total * steps,
        format!("{action} {title} ({} of {total})", position + 1),
        match detail {
            Some(detail) => format!("{detail} · {counts}"),
            None => counts,
        },
    )
}

/// "12 of 47 min", or "52 min so far" once a file runs past its probed length.
#[cfg(not(target_os = "android"))]
fn transcribed_minutes(start_ms: i64, duration_hint_ms: i64) -> String {
    let done = start_ms.max(0) / 60_000;
    if duration_hint_ms > 0 && start_ms <= duration_hint_ms {
        format!("{done} of {} min", (duration_hint_ms + 59_999) / 60_000)
    } else {
        format!("{done} min so far")
    }
}

/// Returns how much work was done: items indexed plus excerpts embedded.
async fn run_indexing(app: AppHandle, handle: JobHandle, rebuild: bool) -> Result<usize, String> {
    let store = store(&app)?;
    let (has_location, inputs) = library_inputs(&app).await?;
    if !has_location {
        return Err("Choose a Library folder before indexing".into());
    }
    let active: HashSet<String> = inputs.iter().map(|i| i.book_id.clone()).collect();
    store.remove_absent(&active).map_err(|e| e.to_string())?;
    let actions = store.delta_actions(&inputs).map_err(|e| e.to_string())?;
    let statuses = store.item_statuses().map_err(|e| e.to_string())?;
    let settings = store.settings().map_err(|e| e.to_string())?;
    let changed = selected_item_ids(actions, &statuses, &settings, &inputs, rebuild);
    let total = changed.len().max(1);
    let mut work = 0usize;
    let items = inputs.iter().filter(|i| changed.contains(&i.book_id));
    for (position, item) in items.enumerate() {
        if handle.is_cancelled() {
            return Ok(work);
        }

        wait_for_ai_idle(&app).await;
        work += 1;
        let progress = RunProgress {
            handle: handle.clone(),
            position,
            total,
            title: item.title.clone(),
        };
        progress.report("Indexing", 0.0, None);
        if let Err(error) = index_one_item(&app, &progress, &store, item, rebuild).await {
            let _ = store.index_failed(item, &error);
        }

        work += embed_pending(&app, &store, &handle, Some(&progress)).await.unwrap_or(0);
        emit_status(&app).await;
    }
    work += embed_pending(&app, &store, &handle, None).await.unwrap_or(0);
    Ok(work)
}

fn selected_item_ids(
    actions: Vec<grafium_core::library_index::DeltaAction>,
    statuses: &std::collections::HashMap<String, (String, Option<String>)>,
    settings: &LibraryIndexSettings,
    inputs: &[LibraryItemInput],
    rebuild: bool,
) -> HashSet<String> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            grafium_core::library_index::DeltaAction::New(id)
            | grafium_core::library_index::DeltaAction::Changed(id) => Some(id),
            grafium_core::library_index::DeltaAction::Unchanged(id) if rebuild => Some(id),
            grafium_core::library_index::DeltaAction::Unchanged(id)
                if statuses.get(&id).is_some_and(|(status, _)| {
                    status == "failed"
                        || status == "pending"
                        || (settings.transcribe_media
                            && status == "title_only"
                            && inputs.iter().any(|item| {
                                item.book_id == id
                                    && matches!(
                                        item.kind,
                                        LibraryItemKind::Audio | LibraryItemKind::Video
                                    )
                            }))
                }) =>
            {
                Some(id)
            }
            _ => None,
        })
        .collect()
}

async fn index_one_item(
    app: &AppHandle,
    _progress: &RunProgress,
    store: &LibraryIndexStore,
    item: &LibraryItemInput,
    rebuild: bool,
) -> Result<(), String> {
    if item.source_url.is_some() {
        store
            .index_title_only(
                item,
                "Network Library items are indexed by title only; no network fetch is performed.",
            )
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    if !item.available || item.files.iter().any(|f| !f.available) {
        store
            .index_failed(item, "Source missing, inaccessible, or replaced")
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let due_files = store
        .due_files(item, rebuild, now_ms())
        .map_err(|e| e.to_string())?;
    if due_files.is_empty() {
        return Ok(());
    }
    match item.kind {
        LibraryItemKind::Epub => index_epub_item(app, store, item).await?,
        LibraryItemKind::Audio | LibraryItemKind::Video => {
            let settings = store.settings().map_err(|e| e.to_string())?;
            if !settings.transcribe_media {
                store
                    .index_title_only(item, "Library media transcription is turned off.")
                    .map_err(|e| e.to_string())?;
            } else {
                #[cfg(target_os = "android")]
                {
                    store
                        .index_title_only(
                            item,
                            "Local media transcription is currently available in desktop builds.",
                        )
                        .map_err(|e| e.to_string())?;
                }
                #[cfg(not(target_os = "android"))]
                index_media_item(app, _progress, store, item, &due_files, rebuild).await?;
            }
        }
        LibraryItemKind::Youtube => {
            store
                .index_title_only(
                    item,
                    "Network Library items are indexed by title only; no network fetch is performed.",
                )
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn index_epub_item(
    app: &AppHandle,
    store: &LibraryIndexStore,
    item: &LibraryItemInput,
) -> Result<(), String> {
    wait_for_ai_idle(app).await;
    let Some(file) = item.files.first() else {
        store
            .index_failed(item, "EPUB source file is missing")
            .map_err(|e| e.to_string())?;
        return Ok(());
    };
    let path = file.absolute_path.clone();
    let title = item.title.clone();
    let book_id = item.book_id.clone();
    let (chunks, warning) = tauri::async_runtime::spawn_blocking(move || {
        extract_book_chunks_from_path(&book_id, &title, &path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    store
        .index_chunks(item, &chunks, None, warning.as_deref())
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "android"))]
async fn index_media_item(
    app: &AppHandle,
    progress: &RunProgress,
    store: &LibraryIndexStore,
    item: &LibraryItemInput,
    due_files: &[LibraryFileInput],
    rebuild: bool,
) -> Result<(), String> {
    for (file_index, file) in due_files.iter().enumerate() {
        if progress.handle.is_cancelled() {
            return Ok(());
        }
        wait_for_ai_idle(app).await;
        let workdir = cache_workdir(app)?;
        let path = file.absolute_path.clone();
        let track_id = file
            .track_id
            .clone()
            .unwrap_or_else(|| item.book_id.clone());
        let transcript = transcribe_local_media(
            app,
            progress,
            (file_index, due_files.len()),
            item,
            file,
            path,
            workdir.clone(),
            rebuild,
        )
        .await;
        let _ = std::fs::remove_dir_all(&workdir);
        match transcript {
            Ok(()) => {}
            Err(error) => {
                if is_pause_reason(&error) {
                    return Ok(());
                }
                store
                    .mark_file_failed(
                        item,
                        file,
                        &format!("Media transcription unavailable for {track_id}: {error}"),
                        now_ms(),
                    )
                    .map_err(|e| e.to_string())?;
                continue;
            }
        }
    }
    Ok(())
}

/// Checked before every transcription slice. Returns why indexing must pause,
/// or `None` once the next slice may start. Chat and search share the single
/// native model worker, so while they are busy this waits instead of queueing
/// another slice in front of them; stopping still takes effect while waiting.
#[cfg(not(target_os = "android"))]
fn wait_for_slice_turn(
    cancelled: impl Fn() -> bool,
    settings: impl Fn() -> Result<LibraryIndexSettings, String>,
    ai_busy: impl Fn() -> bool,
    nap: impl Fn(),
) -> Option<String> {
    loop {
        if cancelled() {
            return Some("Library indexing cancelled".into());
        }
        match settings() {
            Ok(settings) if !settings.enabled => return Some("Library index is off".into()),
            Ok(settings) if !settings.transcribe_media => {
                return Some("Library media transcription is turned off.".into())
            }
            Ok(_) => {}
            Err(error) => return Some(error),
        }
        if !ai_busy() {
            return None;
        }
        nap();
    }
}

#[cfg(not(target_os = "android"))]
fn is_pause_reason(error: &str) -> bool {
    matches!(
        error,
        "Library indexing cancelled"
            | "Library index is off"
            | "Library media transcription is turned off."
            | "Library media transcription is turned off"
    )
}

#[cfg(not(target_os = "android"))]
async fn transcribe_local_media(
    app: &AppHandle,
    progress: &RunProgress,
    (file_index, file_count): (usize, usize),
    item: &LibraryItemInput,
    file: &LibraryFileInput,
    path: PathBuf,
    workdir: PathBuf,
    rebuild: bool,
) -> Result<(), String> {
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;
    let (app, item, file) = (app.clone(), item.clone(), file.clone());
    let handle = progress.handle.clone();
    // Where this file's transcription is, as a share of the item, so the Jobs
    // bar holds its place while waiting for Chat.
    let item_fraction = Arc::new(AtomicU32::new(0));
    let track = move |detail: String| {
        if file_count > 1 {
            format!("track {} of {file_count}, {detail}", file_index + 1)
        } else {
            detail
        }
    };
    let on_slice = {
        let (progress, item_fraction, track) = (progress.clone(), item_fraction.clone(), track.clone());
        move |start_ms: i64, duration_hint_ms: i64| {
            let within_file = if duration_hint_ms > 0 {
                (start_ms as f64 / duration_hint_ms as f64).min(0.99)
            } else {
                0.0
            };
            let fraction = (file_index as f64 + within_file) / file_count.max(1) as f64;
            item_fraction.store((fraction * 1e6) as u32, Ordering::Relaxed);
            progress.report(
                "Transcribing",
                fraction,
                Some(track(transcribed_minutes(start_ms, duration_hint_ms))),
            );
        }
    };
    let waiting = {
        let progress = progress.clone();
        let naps = std::cell::Cell::new(0u32);
        move || {
            // About every two seconds, not on every short nap.
            if naps.get() % 8 == 0 {
                progress.report(
                    "Waiting to transcribe",
                    f64::from(item_fraction.load(Ordering::Relaxed)) / 1e6,
                    Some(track("paused while Chat or search uses the model".into())),
                );
            }
            naps.set(naps.get().wrapping_add(1));
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    };
    // Every slice runs ffmpeg, Whisper and SQLite synchronously: keep the whole
    // file off the async runtime so other jobs and commands keep running.
    let result = tauri::async_runtime::spawn_blocking(move || {
        let media_config = load_media_config(&app)?;
        if !media_config.enabled {
            return Err("Whisper transcription is disabled in media settings".to_string());
        }
        let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let transcriber = grafium_core::media::managed_transcriber(&media_config, &data_dir)
            .map_err(|e| e.to_string())?;
        // The store only holds its path, so this thread opens its own handle.
        let index = self::store(&app)?;
        let mut source = FfmpegSliceSource::new(path, workdir.clone())?.with_progress(on_slice);
        let mut slice_transcriber = WhisperSliceTranscriber {
            workdir,
            transcriber,
        };
        index_media_file_slices(
            &index,
            &item,
            &file,
            MEDIA_SLICE_MS,
            rebuild,
            || {
                wait_for_slice_turn(
                    || handle.is_cancelled(),
                    || index.settings().map_err(|e| e.to_string()),
                    // Chat, search and a media import the user started all
                    // come before background Library transcription.
                    || app.state::<LibraryIndexState>().ai_busy.load(Ordering::SeqCst) > 0
                        || crate::commands::media::media_import_running(),
                    &waiting,
                )
            },
            &mut source,
            &mut slice_transcriber,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    match result {
        grafium_core::library_index::MediaIndexOutcome::Completed => Ok(()),
        grafium_core::library_index::MediaIndexOutcome::Paused(reason) => Err(reason),
        // The loop already recorded this failure (and its retry time) for the
        // file; recording it again would double the backoff.
        grafium_core::library_index::MediaIndexOutcome::Failed(_) => Ok(()),
    }
}

#[cfg(not(target_os = "android"))]
struct FfmpegSliceSource {
    input: PathBuf,
    workdir: PathBuf,
    duration_ms: i64,
    /// Told the start of each slice and the probed length, for progress.
    on_slice: Box<dyn Fn(i64, i64) + Send>,
}

#[cfg(not(target_os = "android"))]
impl FfmpegSliceSource {
    fn new(input: PathBuf, workdir: PathBuf) -> Result<Self, String> {
        Ok(Self {
            duration_ms: media_duration_ms(&input)?,
            input,
            workdir,
            on_slice: Box::new(|_, _| {}),
        })
    }

    fn with_progress(mut self, on_slice: impl Fn(i64, i64) + Send + 'static) -> Self {
        self.on_slice = Box::new(on_slice);
        self
    }
}

#[cfg(not(target_os = "android"))]
impl MediaSliceSource for FfmpegSliceSource {
    fn slice(
        &mut self,
        start_ms: i64,
        duration_ms: i64,
    ) -> Result<MediaSlice, grafium_core::CoreError> {
        (self.on_slice)(start_ms, self.duration_ms);
        let wav = self.workdir.join(format!("slice-{start_ms}.wav"));
        extract_audio_slice(&self.input, &wav, start_ms, duration_ms)
            .map_err(grafium_core::CoreError::Other)?;
        let bytes = wav_data_bytes(&wav).unwrap_or(0);
        if bytes == 0 {
            let _ = std::fs::remove_file(&wav);
            return Ok(if start_ms == 0 {
                MediaSlice::NoAudio
            } else {
                MediaSlice::EndOfFile
            });
        }
        // Concatenated MP3 audiobooks and stitched-in segments drop a few
        // milliseconds at their joins. Only a clearly shorter slice marks the
        // end of the file (a slice with no samples at all ends it above).
        let clearly_short = bytes + ONE_SECOND_OF_AUDIO_BYTES < requested_bytes(duration_ms);
        if clearly_short && start_ms >= self.duration_ms {
            // Ogg and Opus repeat their last second past the end; the hint says
            // this is that tail, not new audio.
            let _ = std::fs::remove_file(&wav);
            return Ok(MediaSlice::EndOfFile);
        }
        Ok(MediaSlice::Data {
            start_ms,
            end_ms: start_ms + duration_ms,
            is_final: clearly_short,
        })
    }
}

/// 16 kHz mono 16-bit PCM, as produced for Whisper.
#[cfg(not(target_os = "android"))]
const ONE_SECOND_OF_AUDIO_BYTES: u64 = 32_000;

#[cfg(not(target_os = "android"))]
fn requested_bytes(duration_ms: i64) -> u64 {
    duration_ms.saturating_mul(32).max(0) as u64
}

#[cfg(not(target_os = "android"))]
struct WhisperSliceTranscriber {
    workdir: PathBuf,
    transcriber: grafium_core::media::ManagedTranscriber,
}

#[cfg(not(target_os = "android"))]
impl MediaSliceTranscriber for WhisperSliceTranscriber {
    fn transcribe_slice(
        &mut self,
        start_ms: i64,
        _end_ms: i64,
    ) -> Result<Vec<grafium_core::media::TranscriptSegment>, grafium_core::CoreError> {
        let wav = self.workdir.join(format!("slice-{start_ms}.wav"));
        let transcript = self
            .transcriber
            .transcribe_with_progress(&wav, &mut |_| {})
            .map_err(|e| grafium_core::CoreError::Other(e.to_string()))?;
        let _ = std::fs::remove_file(&wav);
        Ok(transcript.segments)
    }
}

#[cfg(not(target_os = "android"))]
fn media_duration_ms(input: &Path) -> Result<i64, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &input.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffprobe failed to start: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let seconds: f64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|e| format!("ffprobe returned an invalid duration: {e}"))?;
    Ok((seconds * 1000.0).ceil() as i64)
}

#[cfg(not(target_os = "android"))]
fn extract_audio_slice(
    input: &Path,
    output: &Path,
    start_ms: i64,
    duration_ms: i64,
) -> Result<(), String> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &(start_ms as f64 / 1000.0).to_string(),
            "-t",
            &(duration_ms as f64 / 1000.0).to_string(),
            "-i",
            &input.to_string_lossy(),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            &output.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("ffmpeg failed while slicing media: {status}"));
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn wav_data_bytes(path: &Path) -> Option<u64> {
    let Ok(bytes) = std::fs::read(path) else {
        return None;
    };
    let mut index = 12usize;
    while index.saturating_add(8) <= bytes.len() {
        let id = &bytes[index..index + 4];
        let size = u32::from_le_bytes([
            bytes[index + 4],
            bytes[index + 5],
            bytes[index + 6],
            bytes[index + 7],
        ]) as usize;
        if id == b"data" {
            return Some(size as u64);
        }
        index = index.saturating_add(8).saturating_add(size + (size % 2));
    }
    None
}

#[cfg(not(target_os = "android"))]
fn load_media_config(app: &AppHandle) -> Result<grafium_core::media::MediaConfig, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("media");
    let path = dir.join("media_config.json");
    if path.exists() {
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    } else {
        Ok(Default::default())
    }
}

async fn embed_pending(
    app: &AppHandle,
    store: &LibraryIndexStore,
    handle: &JobHandle,
    progress: Option<&RunProgress>,
) -> Result<usize, String> {
    let mut embedded = 0usize;
    loop {
        if handle.is_cancelled() {
            return Ok(embedded);
        }
        wait_for_ai_idle(app).await;
        let knowledge = app.state::<KnowledgeState>();
        let Some(scheme) = ({
            let guard = knowledge.engine.read().await;
            guard.as_ref().and_then(|e| e.library_semantic_scheme())
        }) else {
            return Ok(embedded);
        };
        let batch = store
            .unembedded_chunks(&scheme, 24)
            .map_err(|e| e.to_string())?;
        if batch.is_empty() {
            return Ok(embedded);
        }
        match progress {
            Some(progress) => progress.report("Preparing search for", 0.999, None),
            None => handle.progress(1, 1, "Preparing Library search"),
        }
        let texts: Vec<String> = batch.iter().map(|(_, text)| text.clone()).collect();
        let vectors = {
            let guard = knowledge.engine.read().await;
            let Some(engine) = guard.as_ref() else {
                return Ok(embedded);
            };
            match engine.embed_library_documents(&texts).await {
                Ok(vectors) => vectors,
                Err(error) => {
                    if let Ok(mut inner) = app.state::<LibraryIndexState>().inner.lock() {
                        inner.semantic_error = Some(cap_reason(&error.to_string()));
                    }
                    tracing::warn!("Library semantic embedding unavailable: {error}");
                    return Ok(embedded);
                }
            }
        };
        let Some((scheme, vectors)) = vectors else {
            return Ok(embedded);
        };
        let pairs: Vec<_> = batch
            .iter()
            .map(|(id, _)| *id)
            .zip(vectors.into_iter())
            .collect();
        store
            .upsert_vectors(&scheme, &pairs)
            .map_err(|e| e.to_string())?;
        embedded += pairs.len();
        if let Ok(mut inner) = app.state::<LibraryIndexState>().inner.lock() {
            inner.semantic_error = None;
        }
    }
}

async fn library_inputs(app: &AppHandle) -> Result<(bool, Vec<LibraryItemInput>), String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let reader = app.state::<ReaderState>();
        let directory = super::private_reader::directory(&app)?;
        let (root, records) = reader.with_store(directory, |store| store.index_records())?;
        let Some(root) = root else {
            return Ok((false, Vec::new()));
        };
        let root = PathBuf::from(root);
        let mut inputs = Vec::new();
        for record in records {
            let mut files = Vec::new();
            for file in record.files {
                let absolute_path = match safe_join(&root, &file.relative_path) {
                    Ok(path) => path,
                    Err(_) => {
                        files.push(LibraryFileInput {
                            track_id: file.track_id,
                            relative_path: file.relative_path,
                            absolute_path: root.clone(),
                            size: 0,
                            mtime_ms: 0,
                            available: false,
                        });
                        continue;
                    }
                };
                let (size, mtime_ms, available) = match std::fs::metadata(&absolute_path) {
                    Ok(meta) => (
                        meta.len(),
                        meta.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                        file.available,
                    ),
                    Err(_) => (0, 0, false),
                };
                files.push(LibraryFileInput {
                    track_id: file.track_id,
                    relative_path: file.relative_path,
                    absolute_path,
                    size,
                    mtime_ms,
                    available,
                });
            }
            inputs.push(LibraryItemInput {
                book_id: record.book.id,
                title: record.book.title,
                kind: match record.book.kind {
                    ReaderKind::Audio => LibraryItemKind::Audio,
                    ReaderKind::Video => LibraryItemKind::Video,
                    ReaderKind::Epub => LibraryItemKind::Epub,
                    ReaderKind::Youtube => LibraryItemKind::Youtube,
                },
                source_url: record.book.source_url,
                available: record.book.available,
                files,
            });
        }
        Ok((true, inputs))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.contains('\\')
        || path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Library file path is not relative".into());
    }
    Ok(root.join(path))
}

async fn wait_for_ai_idle(app: &AppHandle) {
    loop {
        let state = app.state::<LibraryIndexState>();
        if state.ai_busy.load(Ordering::SeqCst) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn status_for(
    app: &AppHandle,
    running: bool,
    job_id: Option<String>,
    knowledge: &KnowledgeState,
) -> Result<LibraryIndexStatus, String> {
    let store = store(app)?;
    let semantic_error = app
        .try_state::<LibraryIndexState>()
        .and_then(|s| s.inner.lock().ok().and_then(|i| i.semantic_error.clone()));
    let semantic_reason = if semantic_error.is_some() {
        semantic_error.map(|reason| cap_reason(&reason))
    } else {
        let guard = knowledge.engine.read().await;
        guard
            .as_ref()
            .and_then(|engine| engine.library_semantic_unavailable_reason())
            .or_else(|| {
                guard
                    .as_ref()
                    .is_none()
                    .then(|| "AI engine is not initialized.".into())
            })
            .map(|reason| cap_reason(&reason))
    };
    let semantic = if semantic_reason.is_some() {
        LibrarySemanticState::Unavailable
    } else {
        let scheme = {
            let guard = knowledge.engine.read().await;
            guard
                .as_ref()
                .and_then(|engine| engine.library_semantic_scheme())
        };
        if store
            .mark_vectors_stale(scheme.as_deref())
            .map_err(|e| e.to_string())?
        {
            LibrarySemanticState::Stale
        } else {
            LibrarySemanticState::Ready
        }
    };
    let settings = store.settings().map_err(|e| e.to_string())?;
    let (transcription, transcription_reason) = if !settings.transcribe_media {
        (
            LibraryTranscriptionState::Off,
            Some("Library media transcription is turned off.".to_string()),
        )
    } else {
        #[cfg(target_os = "android")]
        {
            (
                LibraryTranscriptionState::Unavailable,
                Some(
                    "Library media transcription is currently available in desktop builds.".into(),
                ),
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            match load_media_config(app) {
                Ok(config) if config.enabled => (LibraryTranscriptionState::Ready, None),
                Ok(_) => (
                    LibraryTranscriptionState::Unavailable,
                    Some("Whisper transcription is disabled in media settings.".into()),
                ),
                Err(error) => (
                    LibraryTranscriptionState::Unavailable,
                    Some(cap_reason(&error)),
                ),
            }
        }
    };
    store
        .status(
            running,
            job_id,
            semantic,
            semantic_reason,
            transcription,
            transcription_reason,
        )
        .map_err(|e| e.to_string())
}

async fn emit_status(app: &AppHandle) {
    if let (Some(state), Some(knowledge)) = (
        app.try_state::<LibraryIndexState>(),
        app.try_state::<KnowledgeState>(),
    ) {
        let (running, job_id) = {
            let inner = state.inner.lock().unwrap_or_else(|p| p.into_inner());
            (inner.running_job.is_some(), inner.running_job.clone())
        };
        if let Ok(status) = status_for(app, running, job_id, &knowledge).await {
            let _ = app.emit(LIBRARY_INDEX_EVENT, status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_accepts_ellipsis_but_rejects_parent_components() {
        let root = Path::new("/synthetic/library");
        assert_eq!(
            safe_join(root, "Wait... What.mp3").unwrap(),
            root.join("Wait... What.mp3")
        );
        assert!(safe_join(root, "../secret.mp3").is_err());
        assert!(safe_join(root, "/tmp/secret.mp3").is_err());
    }

    #[test]
    fn cancelled_job_clears_dirty_and_pending_rebuild_without_rerun() {
        let mut inner = LibraryIndexInner {
            running_job: Some("job".into()),
            dirty: true,
            pending_rebuild: true,
            semantic_error: None,
            cache_cleaned: true,
        };
        assert_eq!(finish_job_state(&mut inner, "job", true), None);
        assert!(!inner.dirty);
        assert!(!inner.pending_rebuild);
        assert!(inner.running_job.is_none());
    }

    #[test]
    fn queued_delta_during_rebuild_does_not_force_another_rebuild() {
        let mut inner = LibraryIndexInner {
            running_job: Some("job".into()),
            dirty: true,
            pending_rebuild: false,
            semantic_error: None,
            cache_cleaned: true,
        };
        assert_eq!(finish_job_state(&mut inner, "job", false), Some(false));
    }

    #[test]
    fn selection_resumes_pending_items_on_delta() {
        let input = LibraryItemInput {
            book_id: "audio".into(),
            title: "Audio".into(),
            kind: LibraryItemKind::Audio,
            source_url: None,
            available: true,
            files: vec![],
        };
        let mut statuses = std::collections::HashMap::new();
        statuses.insert("audio".into(), ("pending".into(), None));
        let selected = selected_item_ids(
            vec![grafium_core::library_index::DeltaAction::Unchanged(
                "audio".into(),
            )],
            &statuses,
            &LibraryIndexSettings {
                enabled: true,
                transcribe_media: true,
            },
            &[input],
            false,
        );
        assert!(selected.contains("audio"));
    }

    #[test]
    fn selection_reprocesses_unchanged_items_on_rebuild_but_not_delta() {
        let input = LibraryItemInput {
            book_id: "book".into(),
            title: "Book".into(),
            kind: LibraryItemKind::Epub,
            source_url: None,
            available: true,
            files: vec![],
        };
        let statuses = std::collections::HashMap::from([("book".into(), ("indexed".into(), None))]);
        let actions = vec![grafium_core::library_index::DeltaAction::Unchanged(
            "book".into(),
        )];
        let settings = LibraryIndexSettings {
            enabled: true,
            transcribe_media: true,
        };
        assert!(selected_item_ids(
            actions.clone(),
            &statuses,
            &settings,
            &[input.clone()],
            false
        )
        .is_empty());
        assert!(selected_item_ids(actions, &statuses, &settings, &[input], true).contains("book"));
    }

    #[test]
    fn cleanup_cache_root_removes_stale_slice_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library-index");
        std::fs::create_dir_all(root.join("old-job")).unwrap();
        std::fs::write(root.join("old-job").join("slice.wav"), b"stale").unwrap();
        cleanup_cache_root(&root).unwrap();
        assert!(root.exists());
        assert!(!root.join("old-job").exists());
    }

    #[test]
    fn jobs_show_which_item_how_far_and_how_many_are_left() {
        let (done, total, message, details) = super::progress_parts(
            2,
            12,
            "Fuel filter replacement",
            "Transcribing",
            0.5,
            Some("14 of 47 min".into()),
        );
        assert_eq!(message, "Transcribing Fuel filter replacement (3 of 12)");
        assert_eq!(details, "14 of 47 min · 2 done, 10 left");
        // Two whole items plus half of the third, out of twelve.
        assert_eq!((done, total), (2_500, 12_000));
        let (done, _, _, details) = super::progress_parts(11, 12, "Last", "Indexing", 7.0, None);
        assert_eq!(done, 11_999, "a finishing item never shows the run as complete");
        assert_eq!(details, "11 done, 1 left");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn transcription_progress_counts_minutes_even_past_a_wrong_length() {
        assert_eq!(super::transcribed_minutes(14 * 60_000, 46 * 60_000 + 1), "14 of 47 min");
        assert_eq!(super::transcribed_minutes(0, 0), "0 min so far");
        assert_eq!(super::transcribed_minutes(52 * 60_000, 45 * 60_000), "52 min so far");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn production_slicer_keeps_going_across_concatenated_mp3_joins() {
        let encoders = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .output();
        if !encoders.is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("libmp3lame")) {
            eprintln!("skipping concatenated MP3 slicer test: ffmpeg with libmp3lame is not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // Audiobooks are often chapters joined byte for byte; the joins drop a
        // few milliseconds from the slice that spans them.
        let mut book = Vec::new();
        for (index, frequency) in [330, 550].iter().enumerate() {
            let part = dir.path().join(format!("part-{index}.mp3"));
            let status = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(format!("sine=frequency={frequency}:duration=400"))
                .args(["-c:a", "libmp3lame", "-q:a", "4"])
                .arg(&part)
                .stdin(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
            book.extend(std::fs::read(&part).unwrap());
        }
        let input = dir.path().join("book.mp3");
        std::fs::write(&input, book).unwrap();
        let workdir = dir.path().join("slices");
        std::fs::create_dir_all(&workdir).unwrap();
        let mut source = super::FfmpegSliceSource::new(input, workdir).unwrap();
        let mut start = 0;
        for _ in 0..10 {
            match source.slice(start, super::MEDIA_SLICE_MS).unwrap() {
                MediaSlice::Data { end_ms, is_final, .. } => {
                    start = end_ms;
                    if is_final {
                        break;
                    }
                }
                MediaSlice::EndOfFile => break,
                MediaSlice::NoAudio => panic!("the book has audio"),
            }
        }
        assert!(start >= 780_000, "only {start} ms of the 800 s book would be transcribed");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn transcription_waits_for_chat_between_slices_and_still_stops() {
        use std::cell::Cell;
        let on = || Ok(LibraryIndexSettings { enabled: true, transcribe_media: true });
        // Busy for three checks, then free: the next slice starts only then.
        let polls = Cell::new(0);
        let naps = Cell::new(0);
        let turn = super::wait_for_slice_turn(
            || false,
            on,
            || {
                polls.set(polls.get() + 1);
                polls.get() <= 3
            },
            || naps.set(naps.get() + 1),
        );
        assert_eq!(turn, None);
        assert_eq!(naps.get(), 3);
        // Cancelling while waiting for chat stops at the next check.
        let cancelled = Cell::new(false);
        let turn = super::wait_for_slice_turn(
            || cancelled.get(),
            on,
            || true,
            || cancelled.set(true),
        );
        assert_eq!(turn.as_deref(), Some("Library indexing cancelled"));
        // Turning transcription off pauses instead of waiting.
        let turn = super::wait_for_slice_turn(
            || false,
            || Ok(LibraryIndexSettings { enabled: true, transcribe_media: false }),
            || true,
            || panic!("must not wait when transcription is off"),
        );
        assert_eq!(turn.as_deref(), Some("Library media transcription is turned off."));
    }

    #[test]
    fn production_ffmpeg_slicer_reports_eof_after_synthetic_audio() {
        if !tool_available("ffmpeg") || !tool_available("ffprobe") {
            eprintln!("skipping production ffmpeg slicer test: ffmpeg is not on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("seven-minutes.wav");
        assert!(generate_audio(&input, &["-c:a", "pcm_s16le"]));
        assert_slice_counts(&input, 2);
    }

    #[test]
    fn production_ffmpeg_slicer_reports_eof_for_ogg_and_opus() {
        if !tool_available("ffmpeg") || !tool_available("ffprobe") {
            eprintln!("skipping production container slicer test: ffmpeg/ffprobe is not on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let ogg = dir.path().join("seven-minutes.ogg");
        if generate_audio(&ogg, &["-c:a", "libvorbis"]) {
            assert_slice_counts(&ogg, 2);
        } else {
            eprintln!("skipping Ogg/Vorbis part: libvorbis encoder is unavailable");
        }
        let opus = dir.path().join("seven-minutes.opus");
        if generate_audio(&opus, &["-c:a", "libopus"]) {
            assert_slice_counts(&opus, 2);
        } else {
            eprintln!("skipping Opus part: libopus encoder is unavailable");
        }
    }

    fn tool_available(name: &str) -> bool {
        Command::new(name)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn generate_audio(path: &Path, codec_args: &[&str]) -> bool {
        let mut args = vec![
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=420",
        ];
        args.extend_from_slice(codec_args);
        let path_text = path.to_string_lossy();
        args.push(&path_text);
        Command::new("ffmpeg")
            .args(args)
            .stdin(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn assert_slice_counts(path: &Path, expected_data_slices: usize) {
        let workdir = path
            .parent()
            .unwrap()
            .join(format!("slices-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir).unwrap();
        let mut source = FfmpegSliceSource::new(path.to_path_buf(), workdir).unwrap();
        let mut start = 0;
        let mut count = 0;
        loop {
            match source.slice(start, MEDIA_SLICE_MS).unwrap() {
                MediaSlice::Data { end_ms, .. } => {
                    count += 1;
                    start = end_ms;
                }
                MediaSlice::NoAudio => break,
                MediaSlice::EndOfFile => break,
            }
        }
        assert_eq!(count, expected_data_slices);
        assert_eq!(
            source.slice(start, MEDIA_SLICE_MS).unwrap(),
            MediaSlice::EndOfFile
        );
    }
}
