//! Visible progress for automatic AI search indexing.
//!
//! The reindex drainer embeds changed pages quietly in the background. That's
//! right for ordinary typing, but after a book import or a sync pull thousands
//! of passages can take minutes to embed, and a user who asks Chat about the
//! book meanwhile gets weaker answers with no hint why. For those large batches
//! the drainer surfaces a single "Building AI search index" job in the activity
//! list instead.

use std::collections::HashSet;

use tauri::Manager;

use crate::commands::jobs::{JobHandle, JobLink, JobsState, BACKGROUND_INDEX_JOB_KIND};

/// Pages with fewer dirty passages than this embed silently: routine edits
/// finish in a moment and would only add activity-list noise.
pub const MIN_VISIBLE_CHUNKS: usize = 64;

#[derive(Default)]
pub struct IndexActivity {
    job: Option<JobHandle>,
    embedded: usize,
    last_page: Option<(String, String)>,
    /// Pages whose last attempt failed. A page that fails again closes the
    /// visible job, and its later retries stay silent until one succeeds, so a
    /// persistently failing page can't raise a failure toast every cycle.
    failed_pages: HashSet<String>,
}

impl IndexActivity {
    pub fn progress(
        &mut self,
        app: &tauri::AppHandle,
        page_id: &str,
        page_title: &str,
        done: usize,
        total: usize,
    ) {
        if self.job.is_none() {
            if !should_open(total, self.failed_pages.contains(page_id)) {
                return;
            }
            let Some(jobs) = app.try_state::<JobsState>() else {
                return;
            };
            match jobs.registry.start(
                app.clone(),
                BACKGROUND_INDEX_JOB_KIND,
                "Building AI search index",
                false,
            ) {
                Ok(handle) => {
                    self.job = Some(handle);
                    self.embedded = 0;
                }
                Err(_) => return,
            }
        }
        if done == 0 {
            self.last_page = Some((page_id.to_string(), page_title.to_string()));
        }
        if let Some(job) = &self.job {
            job.progress(done, total, progress_message(page_title, done, total));
        }
    }

    pub fn page_indexed(&mut self, page_id: &str, embedded: usize) {
        self.failed_pages.remove(page_id);
        if self.job.is_some() {
            self.embedded += embedded;
        }
    }

    /// Routine failures, like a page edited mid-embedding, clear up on the next
    /// cycle, so the first failure keeps the job open. A repeat failure closes
    /// it rather than leaving a job that never finishes.
    pub fn page_failed(&mut self, page_id: &str, error: &str) {
        if !self.failed_pages.insert(page_id.to_string()) {
            if let Some(job) = self.job.take() {
                self.last_page = None;
                job.failed_with_details(
                    "AI search indexing hit an error and will retry automatically",
                    Some(error.to_string()),
                );
            }
        }
    }

    /// Close the job once nothing is left in the queue.
    pub fn finish_if_idle(&mut self, pending_pages: usize) {
        if pending_pages > 0 {
            return;
        }
        if let Some(job) = self.job.take() {
            let link = self.last_page.take().map(|(page_id, page_title)| JobLink {
                page_id,
                page_title: Some(page_title),
                label: "page".to_string(),
            });
            job.succeeded_with_details(completion_message(self.embedded), link, None::<String>);
        }
    }

    /// AI was switched off or its models went away mid-backlog.
    pub fn pause(&mut self) {
        if let Some(job) = self.job.take() {
            job.failed(
                "AI search indexing paused because the AI models are unavailable; it resumes automatically once they are",
            );
        }
    }
}

fn should_open(total_chunks: usize, previously_failed: bool) -> bool {
    total_chunks >= MIN_VISIBLE_CHUNKS && !previously_failed
}

fn leaf_title(title: &str) -> &str {
    title.rsplit('/').next().unwrap_or(title)
}

fn progress_message(page_title: &str, done: usize, total: usize) -> String {
    format!(
        "Indexing “{}” · {} of {} passages",
        leaf_title(page_title),
        group_digits(done),
        group_digits(total)
    )
}

fn completion_message(embedded: usize) -> String {
    format!(
        "AI search index is up to date · {} passages indexed",
        group_digits(embedded)
    )
}

fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_large_batches_become_visible() {
        assert!(!should_open(MIN_VISIBLE_CHUNKS - 1, false));
        assert!(should_open(MIN_VISIBLE_CHUNKS, false));
        assert!(!should_open(10_000, true), "failed pages retry silently");
    }

    #[test]
    fn messages_name_the_book_and_count_passages() {
        assert_eq!(
            progress_message("Books/Deep Work", 1_200, 2_345),
            "Indexing “Deep Work” · 1,200 of 2,345 passages"
        );
        assert_eq!(
            completion_message(2_345),
            "AI search index is up to date · 2,345 passages indexed"
        );
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000_000), "1,000,000");
    }
}
