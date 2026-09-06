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

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

/// Event channel the frontend subscribes to for all job activity.
pub const JOB_EVENT: &str = "job://update";

/// How many finished jobs we keep for history before evicting the oldest.
/// Bounded on purpose — an unbounded registry in a long-lived desktop session
/// is a slow memory leak.
const MAX_RETAINED_JOBS: usize = 50;
const MAX_RUNNING_JOBS: usize = 2;

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
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            id: id.clone(),
            kind: kind.into(),
            title: title.into(),
            status: JobStatus::Running,
            progress: None,
            message: None,
            link: None,
            error: None,
            cancellable,
            started_at: now_ms(),
            finished_at: None,
        };

        {
            let mut entries = self.entries.lock().map_err(|e| e.to_string())?;
            let running = entries
                .iter()
                .filter(|entry| entry.job.status == JobStatus::Running)
                .count();
            if running >= MAX_RUNNING_JOBS {
                return Err(format!(
                    "Grafium is already running {running} background AI jobs. \
                     Wait for one to finish or cancel it before starting another."
                ));
            }
            if job.kind == "ai_index_all"
                && entries.iter().any(|entry| {
                    entry.job.status == JobStatus::Running && entry.job.kind == "ai_index_all"
                })
            {
                return Err("A full AI index is already running".to_string());
            }
            entries.push(JobEntry {
                job: job.clone(),
                cancel: cancel.clone(),
            });
            evict_old_finished(&mut entries);
        }

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

    /// Drop finished jobs from the activity list.
    pub fn clear_finished(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|e| !e.job.status.is_terminal());
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
        let fraction = if total == 0 {
            None
        } else {
            Some((done as f32 / total as f32).clamp(0.0, 1.0))
        };
        let message = message.into();
        self.emit(|job| {
            job.progress = fraction;
            job.message = Some(message);
        });
    }

    pub fn succeeded(self, message: impl Into<String>, link: Option<JobLink>) {
        let message = message.into();
        self.emit(|job| {
            job.status = JobStatus::Succeeded;
            job.progress = Some(1.0);
            job.message = Some(message);
            job.link = link;
            job.finished_at = Some(now_ms());
        });
    }

    pub fn failed(self, error: impl Into<String>) {
        let error = error.into();
        self.emit(|job| {
            job.status = JobStatus::Failed;
            job.error = Some(error);
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

    fn emit(&self, f: impl FnOnce(&mut Job)) {
        if let Some(updated) = self.registry.mutate(&self.id, f) {
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

#[tauri::command]
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
                cancellable: true,
                started_at: 0,
                finished_at: None,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        }
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
}
