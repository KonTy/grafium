//! Background job registry.
//!
//! Long AI work (indexing a whole graph, generating references for a page)
//! used to run inside the Tauri command the UI awaited, which meant the work
//! was owned by whichever panel happened to be open. Close the panel and the
//! result was dropped on the floor: no status, no notification, no way to
//! cancel.
//!
//! A job is that same work, detached. The command that starts it returns a
//! `JobId` immediately, the work continues regardless of what the user does
//! next, and progress arrives as `job://update` events. The registry keeps the
//! latest snapshot of every job so a freshly-mounted UI can rehydrate rather
//! than guess.
//!
//! Finished jobs are also kept as history in the app data folder, so the Jobs
//! page still shows them after a restart. That file is local to this device;
//! it is not part of any graph and is never synced.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{Emitter, State};

/// Event channel the frontend subscribes to for all job activity.
pub const JOB_EVENT: &str = "job://update";
/// A job that was dropped from the list without being kept as history.
pub const JOB_REMOVED_EVENT: &str = "job://removed";

/// How many jobs we keep, as history across restarts, before evicting the
/// oldest finished ones. Bounded so the list and its file stay small.
const MAX_RETAINED_JOBS: usize = 100;
/// Longest message, error or details text kept in the saved history.
const MAX_SAVED_TEXT: usize = 4_000;
const STOPPED_BY_CLOSE: &str = "Stopped when Grafium closed";
const MAX_RUNNING_JOBS: usize = 2;

/// Automatic AI search indexing after imports, syncs and rebuilds. It is owned
/// by the app, not the user, so it never takes one of the user's job slots and
/// never blocks one from starting.
pub const BACKGROUND_INDEX_JOB_KIND: &str = "ai_index_background";

fn counts_against_limit(kind: &str) -> bool {
    kind != BACKGROUND_INDEX_JOB_KIND && kind != "library_index"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn is_terminal(self) -> bool {
        !matches!(self, JobStatus::Running)
    }
}

/// Where to send the user when a job finishes. A completion toast without a
/// way to reach the thing that was produced is only half a notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLink {
    pub page_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Machine-readable kind, e.g. `ai_index_all` — lets the UI pick an icon.
    pub kind: String,
    /// Human-readable label shown in the activity list.
    pub title: String,
    pub status: JobStatus,
    /// 0.0–1.0 when the total is known up front, `None` when it isn't.
    pub progress: Option<f32>,
    /// Current step, e.g. "Indexing 40 of 512 pages".
    pub message: Option<String>,
    pub link: Option<JobLink>,
    pub error: Option<String>,
    pub details: Option<String>,
    pub cancellable: bool,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct JobEntry {
    job: Job,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct JobRegistry {
    /// Insertion-ordered so eviction can drop the oldest finished job.
    entries: Mutex<Vec<JobEntry>>,
    /// Where history is saved; set once the app data folder is known. Saving
    /// holds this lock, so the last write always reflects the latest list.
    history: Mutex<Option<PathBuf>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new running job and return a handle the worker reports through.
    pub fn start(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        kind: impl Into<String>,
        title: impl Into<String>,
        cancellable: bool,
    ) -> Result<JobHandle, String> {
        self.start_with_link(app, kind, title, cancellable, None)
    }

    pub fn start_with_link(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        kind: impl Into<String>,
        title: impl Into<String>,
        cancellable: bool,
        link: Option<JobLink>,
    ) -> Result<JobHandle, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            id: id.clone(),
            kind: kind.into(),
            title: title.into(),
            status: JobStatus::Running,
            progress: None,
            message: None,
            link,
            error: None,
            details: None,
            cancellable,
            started_at: now_ms(),
            finished_at: None,
        };

        {
            let mut entries = self.entries.lock().map_err(|e| e.to_string())?;
            if let Some(error) = admission_error(&job, &entries) {
                return Err(error);
            }
            entries.push(JobEntry {
                job: job.clone(),
                cancel: cancel.clone(),
            });
            evict_old_finished(&mut entries);
        }
        // Saved while running too, so a job cut short by closing the app is
        // still listed afterwards, as stopped.
        self.save_history();

        let _ = app.emit(JOB_EVENT, &job);

        Ok(JobHandle {
            id,
            app,
            registry: Arc::clone(self),
            cancel,
        })
    }

    pub fn list(&self) -> Vec<Job> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|e| e.job.clone())
            .collect()
    }

    /// Ask a job to stop. Cooperative: the worker decides when to notice.
    pub fn request_cancel(&self, id: &str) -> bool {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match entries.iter().find(|e| e.job.id == id) {
            Some(entry) if entry.job.status == JobStatus::Running => {
                entry.cancel.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    /// Clear the history: drop every finished job, here and on disk.
    /// Running jobs stay.
    pub fn clear_finished(&self) {
        locked(&self.entries).retain(|e| !e.job.status.is_terminal());
        self.save_history();
    }

    /// Bring back the history saved by earlier runs and keep saving it at
    /// `path`. A job that was still running when Grafium closed comes back as
    /// stopped; nothing will finish it now.
    pub fn restore_history(&self, path: PathBuf) {
        let saved: Vec<Job> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        {
            let mut entries = locked(&self.entries);
            let mut restored: Vec<JobEntry> = saved
                .into_iter()
                .filter(|job| !entries.iter().any(|entry| entry.job.id == job.id))
                .map(|mut job| {
                    if job.status == JobStatus::Running {
                        job.status = JobStatus::Cancelled;
                        job.message = Some(STOPPED_BY_CLOSE.to_string());
                        job.progress = None;
                        job.cancellable = false;
                    }
                    JobEntry {
                        job,
                        cancel: Arc::new(AtomicBool::new(false)),
                    }
                })
                .collect();
            restored.append(&mut entries);
            *entries = restored;
            evict_old_finished(&mut entries);
        }
        *locked(&self.history) = Some(path);
        self.save_history();
    }

    fn save_history(&self) {
        let path = locked(&self.history);
        let Some(path) = path.as_ref() else {
            return;
        };
        let jobs: Vec<Job> = locked(&self.entries).iter().map(|e| saved_copy(&e.job)).collect();
        if let Err(error) = write_history(path, &jobs) {
            tracing::warn!("Could not save the job history: {error}");
        }
    }

    /// Remove a job without keeping it as history.
    fn remove(&self, id: &str) -> bool {
        let removed = {
            let mut entries = locked(&self.entries);
            let before = entries.len();
            entries.retain(|e| e.job.id != id);
            entries.len() != before
        };
        if removed {
            self.save_history();
        }
        removed
    }

    fn mutate(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = entries.iter_mut().find(|e| e.job.id == id)?;
        // A job that already reported a terminal status must never be revived;
        // a late progress tick from a worker that hasn't noticed cancellation
        // yet would otherwise flip it back to Running.
        if entry.job.status.is_terminal() {
            return None;
        }
        f(&mut entry.job);
        Some(entry.job.clone())
    }
}

/// Why `job` may not start alongside `entries`, if it may not.
fn admission_error(job: &Job, entries: &[JobEntry]) -> Option<String> {
    let running_like = |kind: &str| {
        entries
            .iter()
            .any(|entry| entry.job.status == JobStatus::Running && entry.job.kind == kind)
    };
    if !counts_against_limit(&job.kind) {
        return running_like(&job.kind)
            .then(|| "Automatic AI search indexing is already running".to_string());
    }
    let running = entries
        .iter()
        .filter(|entry| {
            entry.job.status == JobStatus::Running && counts_against_limit(&entry.job.kind)
        })
        .count();
    if running >= MAX_RUNNING_JOBS {
        return Some(format!(
            "Grafium is already running {running} background jobs. \
             Wait for one to finish or cancel it before starting another."
        ));
    }
    if job.kind == "ai_index_all" && running_like("ai_index_all") {
        return Some("A full AI index is already running".to_string());
    }
    if is_duplicate_concept_edge_job(job, entries) {
        return Some("Concept edge discovery is already running for this page".to_string());
    }
    None
}

fn is_duplicate_concept_edge_job(job: &Job, entries: &[JobEntry]) -> bool {
    if job.kind != "ai_concept_edges" {
        return false;
    }
    let page_id = job.link.as_ref().map(|link| link.page_id.as_str());
    page_id.is_some()
        && entries.iter().any(|entry| {
            entry.job.status == JobStatus::Running
                && entry.job.kind == "ai_concept_edges"
                && entry.job.link.as_ref().map(|link| link.page_id.as_str()) == page_id
        })
}

/// A copy small enough to keep: long texts (tool output, stack traces) are cut.
fn saved_copy(job: &Job) -> Job {
    let cut = |text: &Option<String>| {
        text.as_ref().map(|text| match text.char_indices().nth(MAX_SAVED_TEXT) {
            Some((end, _)) => format!("{}…", &text[..end]),
            None => text.clone(),
        })
    };
    Job {
        message: cut(&job.message),
        error: cut(&job.error),
        details: cut(&job.details),
        ..job.clone()
    }
}

fn write_history(path: &Path, jobs: &[Job]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(jobs)?)?;
    std::fs::rename(&temporary, path)
}

fn evict_old_finished(entries: &mut Vec<JobEntry>) {
    while entries.len() > MAX_RETAINED_JOBS {
        match entries.iter().position(|e| e.job.status.is_terminal()) {
            Some(idx) => {
                entries.remove(idx);
            }
            // Everything still running: nothing safe to evict.
            None => break,
        }
    }
}

/// Worker-side handle. Reporting through this is the only way a job's state
/// changes, so every transition emits exactly one event.
#[derive(Clone)]
pub struct JobHandle {
    id: String,
    app: tauri::AppHandle,
    registry: Arc<JobRegistry>,
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Whether cancellation has been requested. Workers should check this
    /// between units of work and bail out via [`JobHandle::cancelled`].
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn progress(&self, done: usize, total: usize, message: impl Into<String>) {
        self.progress_with_details(done, total, message, None::<String>);
    }

    pub fn progress_with_details(
        &self,
        done: usize,
        total: usize,
        message: impl Into<String>,
        details: Option<impl Into<String>>,
    ) {
        let fraction = if total == 0 {
            None
        } else {
            Some((done as f32 / total as f32).clamp(0.0, 1.0))
        };
        let message = message.into();
        let details = details.map(Into::into);
        self.emit(|job| {
            job.progress = fraction;
            job.message = Some(message);
            job.details = details;
        });
    }

    pub fn succeeded_with_details(
        self,
        message: impl Into<String>,
        link: Option<JobLink>,
        details: Option<impl Into<String>>,
    ) {
        let message = message.into();
        let details = details.map(Into::into);
        self.emit(|job| {
            job.status = JobStatus::Succeeded;
            job.progress = Some(1.0);
            job.message = Some(message);
            job.link = link;
            job.details = details;
            job.finished_at = Some(now_ms());
        });
    }

    pub fn failed(self, error: impl Into<String>) {
        self.failed_with_details(error, None::<String>);
    }

    pub fn failed_with_details(self, error: impl Into<String>, details: Option<impl Into<String>>) {
        let error = error.into();
        let details = details.map(Into::into);
        self.emit(|job| {
            job.status = JobStatus::Failed;
            job.error = Some(error);
            job.details = details;
            job.finished_at = Some(now_ms());
        });
    }

    pub fn cancelled(self) {
        self.emit(|job| {
            job.status = JobStatus::Cancelled;
            job.message = Some("Cancelled".to_string());
            job.finished_at = Some(now_ms());
        });
    }

    /// Finish without leaving an entry: for automatic runs that turned out to
    /// have nothing to do, which would otherwise crowd out real history.
    pub fn discard(self) {
        if self.registry.remove(&self.id) {
            let _ = self.app.emit(JOB_REMOVED_EVENT, &self.id);
        }
    }

    fn emit(&self, f: impl FnOnce(&mut Job)) {
        if let Some(updated) = self.registry.mutate(&self.id, f) {
            if updated.status.is_terminal() {
                self.registry.save_history();
            }
            let _ = self.app.emit(JOB_EVENT, &updated);
        }
    }
}

/// Shared state wrapper so Tauri can manage the registry.
pub struct JobsState {
    pub registry: Arc<JobRegistry>,
}

impl JobsState {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(JobRegistry::new()),
        }
    }
}

impl Default for JobsState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Commands ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn jobs_list(state: State<'_, JobsState>) -> Result<Vec<Job>, String> {
    Ok(state.registry.list())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn jobs_cancel(state: State<'_, JobsState>, job_id: String) -> Result<bool, String> {
    Ok(state.registry.request_cancel(&job_id))
}

#[tauri::command]
pub async fn jobs_clear_finished(state: State<'_, JobsState>) -> Result<(), String> {
    state.registry.clear_finished();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, status: JobStatus) -> JobEntry {
        JobEntry {
            job: Job {
                id: id.to_string(),
                kind: "test".into(),
                title: "Test".into(),
                status,
                progress: None,
                message: None,
                link: None,
                error: None,
                details: None,
                cancellable: true,
                started_at: 0,
                finished_at: None,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn background_indexing_neither_uses_nor_needs_a_user_job_slot() {
        let running = |id: &str, kind: &str| {
            let mut e = entry(id, JobStatus::Running);
            e.job.kind = kind.into();
            e
        };
        let background = running("bg", BACKGROUND_INDEX_JOB_KIND).job;
        let user = running("new", "book_import").job;

        let full = vec![running("a", "book_import"), running("b", "ai_index_all")];
        assert!(admission_error(&background, &full).is_none());
        assert!(admission_error(&user, &full).is_some());

        let with_background = vec![
            running("a", "book_import"),
            running("bg", BACKGROUND_INDEX_JOB_KIND),
        ];
        assert!(admission_error(&user, &with_background).is_none());
        assert!(admission_error(&background, &with_background).is_some());
    }

    fn registry_with(entries: Vec<JobEntry>) -> JobRegistry {
        JobRegistry {
            entries: Mutex::new(entries),
            history: Mutex::new(None),
        }
    }

    #[test]
    fn history_survives_a_restart_and_unfinished_jobs_come_back_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        let first = registry_with(Vec::new());
        first.restore_history(path.clone());
        {
            let mut entries = first.entries.lock().unwrap();
            entries.push(entry("done", JobStatus::Succeeded));
            entries.push(entry("broken", JobStatus::Failed));
            let mut long = entry("long", JobStatus::Failed);
            long.job.details = Some("x".repeat(MAX_SAVED_TEXT + 500));
            entries.push(long);
            entries.push(entry("cut-short", JobStatus::Running));
        }
        first.save_history();

        // A new app run: the jobs are listed again, in their original order.
        let second = registry_with(vec![entry("new", JobStatus::Running)]);
        second.restore_history(path.clone());
        let jobs = second.list();
        let ids: Vec<_> = jobs.iter().map(|job| job.id.as_str()).collect();
        assert_eq!(ids, ["done", "broken", "long", "cut-short", "new"]);
        let stopped = &jobs[3];
        assert_eq!(stopped.status, JobStatus::Cancelled);
        assert_eq!(stopped.message.as_deref(), Some(STOPPED_BY_CLOSE));
        assert!(!stopped.cancellable);
        let details = jobs[2].details.as_deref().unwrap();
        assert_eq!(details.chars().count(), MAX_SAVED_TEXT + 1, "long output is cut");
        assert_eq!(jobs[4].status, JobStatus::Running, "this run's jobs are untouched");
    }

    #[test]
    fn clearing_history_keeps_running_jobs_and_empties_the_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        let registry = registry_with(vec![
            entry("done", JobStatus::Succeeded),
            entry("running", JobStatus::Running),
            entry("stopped", JobStatus::Cancelled),
        ]);
        registry.restore_history(path.clone());

        registry.clear_finished();

        let ids: Vec<_> = registry.list().into_iter().map(|job| job.id).collect();
        assert_eq!(ids, ["running"]);
        let saved: Vec<Job> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "running");
    }

    #[test]
    fn history_keeps_the_newest_hundred_finished_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        let old: Vec<Job> = (0..MAX_RETAINED_JOBS + 20)
            .map(|i| entry(&format!("old-{i}"), JobStatus::Succeeded).job)
            .collect();
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let registry = registry_with(Vec::new());
        registry.restore_history(path.clone());
        let jobs = registry.list();
        assert_eq!(jobs.len(), MAX_RETAINED_JOBS);
        assert_eq!(jobs[0].id, "old-20", "the oldest are dropped first");
        // A missing or unreadable file is an empty history, not an error.
        let fresh = registry_with(Vec::new());
        std::fs::write(&path, b"not json").unwrap();
        fresh.restore_history(path);
        assert!(fresh.list().is_empty());
    }

    #[test]
    fn a_discarded_job_leaves_no_history() {
        let registry = registry_with(vec![entry("keep", JobStatus::Succeeded), entry("noop", JobStatus::Running)]);
        assert!(registry.remove("noop"));
        assert!(!registry.remove("noop"));
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn eviction_never_drops_a_running_job() {
        // A long index must not be forgotten just because the user generated
        // a burst of short jobs afterwards.
        let mut entries: Vec<JobEntry> = Vec::new();
        entries.push(entry("long-running", JobStatus::Running));
        for i in 0..MAX_RETAINED_JOBS + 10 {
            entries.push(entry(&format!("done-{i}"), JobStatus::Succeeded));
        }

        evict_old_finished(&mut entries);

        assert!(entries.len() <= MAX_RETAINED_JOBS);
        assert!(
            entries.iter().any(|e| e.job.id == "long-running"),
            "the running job was evicted"
        );
    }

    #[test]
    fn eviction_stops_when_everything_is_still_running() {
        // Must terminate rather than spin forever looking for a victim.
        let mut entries: Vec<JobEntry> = (0..MAX_RETAINED_JOBS + 5)
            .map(|i| entry(&format!("run-{i}"), JobStatus::Running))
            .collect();
        let before = entries.len();

        evict_old_finished(&mut entries);

        assert_eq!(entries.len(), before);
    }

    #[test]
    fn eviction_drops_oldest_finished_first() {
        let mut entries: Vec<JobEntry> = (0..MAX_RETAINED_JOBS + 1)
            .map(|i| entry(&format!("done-{i}"), JobStatus::Succeeded))
            .collect();

        evict_old_finished(&mut entries);

        assert_eq!(entries.len(), MAX_RETAINED_JOBS);
        assert!(
            !entries.iter().any(|e| e.job.id == "done-0"),
            "expected the oldest finished job to be evicted first"
        );
    }

    #[test]
    fn cancelling_an_unknown_job_is_not_an_error() {
        let registry = JobRegistry::new();
        assert!(!registry.request_cancel("nope"));
    }

    #[test]
    fn clear_finished_keeps_running_work_visible() {
        let registry = JobRegistry::new();
        {
            let mut entries = registry.entries.lock().unwrap();
            entries.push(entry("a", JobStatus::Succeeded));
            entries.push(entry("b", JobStatus::Running));
            entries.push(entry("c", JobStatus::Failed));
        }

        registry.clear_finished();

        let remaining = registry.list();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "b");
    }

    #[test]
    fn a_finished_job_cannot_be_revived_by_a_late_update() {
        // A worker that hasn't noticed cancellation yet may emit one more
        // progress tick. That must not flip the job back to Running.
        let registry = JobRegistry::new();
        {
            let mut entries = registry.entries.lock().unwrap();
            entries.push(entry("a", JobStatus::Cancelled));
        }

        let updated = registry.mutate("a", |job| {
            job.status = JobStatus::Running;
            job.message = Some("late tick".into());
        });

        assert!(updated.is_none(), "a terminal job accepted a late update");
        assert_eq!(registry.list()[0].status, JobStatus::Cancelled);
    }

    #[test]
    fn cancel_is_refused_once_a_job_has_finished() {
        let registry = JobRegistry::new();
        {
            let mut entries = registry.entries.lock().unwrap();
            entries.push(entry("a", JobStatus::Succeeded));
        }
        assert!(!registry.request_cancel("a"));
    }

    #[test]
    fn duplicate_concept_edge_job_for_same_page_is_rejected() {
        let mut entries = Vec::new();
        let mut first = entry("first", JobStatus::Running);
        first.job.kind = "ai_concept_edges".to_string();
        first.job.link = Some(JobLink {
            page_id: "page-1".to_string(),
            page_title: Some("Book".to_string()),
            label: "Book".to_string(),
        });
        entries.push(first);
        let candidate = Job {
            id: "second".to_string(),
            kind: "ai_concept_edges".to_string(),
            title: "Find concept edges: Book".to_string(),
            status: JobStatus::Running,
            progress: None,
            message: None,
            link: Some(JobLink {
                page_id: "page-1".to_string(),
                page_title: Some("Book".to_string()),
                label: "Book".to_string(),
            }),
            error: None,
            details: None,
            cancellable: true,
            started_at: 0,
            finished_at: None,
        };

        assert!(is_duplicate_concept_edge_job(&candidate, &entries));
    }
}
