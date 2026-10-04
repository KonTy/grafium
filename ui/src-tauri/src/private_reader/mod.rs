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
        self.scan_with(directory, |store| store.prepare_scan(), store::ScanJob::run)
    }

    pub fn add_location(&self, directory: PathBuf, path: String) -> ReaderResult<ReaderSnapshot> {
        self.add_location_with_scan(directory, path, store::ScanJob::run)
    }

    fn add_location_with_scan(
        &self,
        directory: PathBuf,
        path: String,
        scan: impl Fn(store::ScanJob) -> ReaderResult<store::ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        self.scan_with(
            directory,
            |store| store.prepare_add_location(path.clone()),
            scan,
        )
    }

    pub fn move_location(
        &self,
        directory: PathBuf,
        from: String,
        to: String,
    ) -> ReaderResult<ReaderSnapshot> {
        self.scan_with(
            directory,
            |store| store.prepare_move_location(&from, to.clone()),
            store::ScanJob::run,
        )
    }

    pub fn remove_location(&self, directory: PathBuf, path: &str) -> ReaderResult<ReaderSnapshot> {
        self.with_store(directory, |store| store.remove_location(path))
    }

    /// Slow external storage traversal must not hold the checkpoint mutex. A
    /// scan overtaken by another registration change is planned again.
    fn scan_with(
        &self,
        directory: PathBuf,
        prepare: impl Fn(&store::ReaderStore) -> ReaderResult<store::ScanJob>,
        scan: impl Fn(store::ScanJob) -> ReaderResult<store::ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let job = self.with_store(directory.clone(), |store| prepare(store))?;
            let result = scan(job);
            match self.with_store(directory.clone(), |store| store.finish_scan(result)) {
                Err(error) if error == store::RETRY_SCAN && attempt < 3 => continue,
                finished => return finished,
            }
        }
    }

    pub fn relink(
        &self,
        directory: PathBuf,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
        location: Option<String>,
    ) -> ReaderResult<ReaderSnapshot> {
        self.relink_with_scan(
            directory,
            book_id,
            relative_path,
            confirm_replacement,
            location,
            store::ScanJob::run,
        )
    }

    fn relink_with_scan(
        &self,
        directory: PathBuf,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
        location: Option<String>,
        scan: impl FnOnce(store::ScanJob) -> ReaderResult<store::ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        source::relative(&relative_path)?;
        let job = self.with_store(directory.clone(), |store| {
            store.prepare_relink(book_id, location)
        })?;
        let result = scan(job)?;
        self.with_store(directory, |store| {
            store.finish_relink(result, book_id, relative_path, confirm_replacement)
        })
    }

    pub fn read_epub(&self, directory: PathBuf, book_id: &str) -> ReaderResult<Vec<u8>> {
        let source = self.with_store(directory, |store| store.media_source(book_id, None))?;
        store::ReaderStore::read_epub_file(source.open()?)
    }

    pub fn media_url(
        &self,
        directory: PathBuf,
        book_id: &str,
        track_id: &str,
    ) -> ReaderResult<String> {
        let source = self.with_store(directory, |store| {
            store.media_source(book_id, Some(track_id))
        })?;
        source.open()?;
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
