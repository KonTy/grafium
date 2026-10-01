//! Device-private external media. Nothing in this module opens a graph, indexes
//! text, syncs a file, or changes the originals in the selected library.
pub mod source;
pub mod store;
pub mod stream;
pub mod types;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
pub use types::*;

#[derive(Default)]
pub struct ReaderState {
    runtime: Mutex<Option<Runtime>>,
}

struct Runtime {
    store: Arc<Mutex<store::ReaderStore>>,
    server: Option<stream::MediaServer>,
}

impl ReaderState {
    pub fn with_store<T>(
        &self,
        directory: PathBuf,
        action: impl FnOnce(&mut store::ReaderStore) -> ReaderResult<T>,
    ) -> ReaderResult<T> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Private reader lock failed")?;
        if runtime.is_none() {
            *runtime = Some(Runtime {
                store: Arc::new(Mutex::new(store::ReaderStore::load(directory)?)),
                server: None,
            });
        }
        let store = runtime.as_ref().unwrap().store.clone();
        drop(runtime);
        let mut store = store
            .lock()
            .map_err(|_| "Private reader store lock failed")?;
        action(&mut store)
    }

    pub fn rescan(&self, directory: PathBuf) -> ReaderResult<ReaderSnapshot> {
        // Slow external storage traversal must not hold the checkpoint mutex.
        let job = self.with_store(directory.clone(), |store| store.prepare_scan());
        let result = job.and_then(store::ScanJob::run);
        self.with_store(directory, |store| store.finish_scan(result))
    }

    pub fn set_library(&self, directory: PathBuf, path: String) -> ReaderResult<ReaderSnapshot> {
        self.set_library_with_scan(directory, path, store::ScanJob::run)
    }

    fn set_library_with_scan(
        &self,
        directory: PathBuf,
        path: String,
        scan: impl FnOnce(store::ScanJob) -> ReaderResult<store::ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        let generation = self.with_store(directory.clone(), |store| Ok(store.generation()))?;
        let result = scan(store::ScanJob::library(path, generation)?)?;
        self.with_store(directory, |store| store.finish_set_library(result))
    }

    pub fn relink(
        &self,
        directory: PathBuf,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
    ) -> ReaderResult<ReaderSnapshot> {
        self.relink_with_scan(
            directory,
            book_id,
            relative_path,
            confirm_replacement,
            store::ScanJob::run,
        )
    }

    fn relink_with_scan(
        &self,
        directory: PathBuf,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
        scan: impl FnOnce(store::ScanJob) -> ReaderResult<store::ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        source::relative(&relative_path)?;
        let job = self.with_store(directory.clone(), |store| store.prepare_scan())?;
        let result = scan(job)?;
        self.with_store(directory, |store| {
            store.finish_relink(result, book_id, relative_path, confirm_replacement)
        })
    }

    pub fn read_epub(&self, directory: PathBuf, book_id: &str) -> ReaderResult<Vec<u8>> {
        let file = self.with_store(directory, |store| store.open_media(book_id, None))?;
        store::ReaderStore::read_epub_file(file)
    }

    pub fn media_url(
        &self,
        directory: PathBuf,
        book_id: &str,
        track_id: &str,
    ) -> ReaderResult<String> {
        self.with_store(directory, |store| {
            store.open_media(book_id, Some(track_id)).map(|_| ())
        })?;
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Private reader lock failed")?;
        let runtime = runtime.as_mut().unwrap();
        if runtime.server.is_none() {
            runtime.server = Some(stream::MediaServer::start(runtime.store.clone())?);
        }
        runtime.server.as_ref().unwrap().url(book_id, track_id)
    }

    pub fn revoke_media(&self) -> ReaderResult<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| "Private reader lock failed")?;
        if let Some(server) = runtime.as_ref().and_then(|r| r.server.as_ref()) {
            server.revoke();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
