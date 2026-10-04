use super::{source, types::*};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 3;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOCATIONS: usize = 32;
pub(super) const RETRY_SCAN: &str = "Library registrations changed during scan; retry the scan";
const MISSING_SOURCE: &str =
    "Source missing, inaccessible, or replaced. History is retained; relink explicitly.";
const RESTORED_SOURCE: &str =
    "Restored from a backup. History is retained; relink it to confirm its source.";
const REMOVED_LOCATION: &str =
    "Not in any current Library location. History is retained; relink it to a file in one of your locations.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    path: String,
    identity: source::RootIdentity,
}

/// A location as registered when its scan was planned.
struct Planned {
    path: String,
    /// Files registered from it, checked one by one when there is no full
    /// discovery. They also tell an emptied mount point from a new folder.
    registered: Vec<String>,
    discover: bool,
}

enum Change {
    Add(String),
    Move { from: String, to: String },
}

pub struct ScanJob {
    planned: Vec<Planned>,
    change: Option<Change>,
    generation: u64,
}

enum Observation {
    Disconnected(String),
    Connected {
        identity: source::RootIdentity,
        stable_ids: bool,
        files: HashMap<String, source::Fingerprint>,
        found: Option<Vec<source::Discovered>>,
        error: Option<String>,
    },
}

pub struct ScanResult {
    observed: Vec<(String, Observation)>,
    change: Option<Change>,
    generation: u64,
}

impl ScanJob {
    /// Runs without the store lock: external drives and shares can be slow.
    pub fn run(self) -> ReaderResult<ScanResult> {
        Ok(ScanResult {
            observed: self
                .planned
                .iter()
                .map(|planned| (planned.path.clone(), observe(planned)))
                .collect(),
            change: self.change,
            generation: self.generation,
        })
    }
}

fn observe(planned: &Planned) -> Observation {
    let (dir, identity) = match source::probe(&planned.path, source::PROBE_TIMEOUT) {
        Ok(root) => root,
        Err(reason) => return Observation::Disconnected(reason),
    };
    // A drive or share that is not mounted often leaves its empty mount
    // point behind; that is not a Library whose every item was deleted.
    if !planned.registered.is_empty() && source::is_empty(&dir) {
        return Observation::Disconnected(source::EMPTY_LOCATION.into());
    }
    let stable_ids = source::stable_file_ids(&dir);
    let (found, error) = if planned.discover {
        match source::discover(&dir) {
            Ok(found) => (Some(found), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let files = match &found {
        Some(found) => found
            .iter()
            .flat_map(|book| book.files.iter().cloned())
            .collect(),
        None => planned
            .registered
            .iter()
            .filter_map(|path| {
                source::open(&dir, path)
                    .and_then(|file| source::Fingerprint::of(&file))
                    .ok()
                    .map(|fingerprint| (path.clone(), fingerprint))
            })
            .collect(),
    };
    Observation::Connected {
        identity,
        stable_ids,
        files,
        found,
        error,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredFile {
    id: String,
    path: String,
    fingerprint: source::Fingerprint,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredBook {
    book: ReaderBook,
    root: Option<Binding>,
    key: String,
    files: Vec<RegisteredFile>,
    manual_order: bool,
    #[serde(default = "authorized_by_default")]
    authorized: bool,
}

fn authorized_by_default() -> bool {
    true
}

/// A local item belongs to the location whose folder it was found in.
fn belongs(stored: &StoredBook, path: &str) -> bool {
    stored.book.source_url.is_none() && stored.root.as_ref().is_some_and(|root| root.path == path)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    version: u32,
    /// The single Library folder of version 2 and earlier; read only to
    /// become the first location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    library: Option<Binding>,
    #[serde(default)]
    locations: Vec<Binding>,
    books: Vec<StoredBook>,
}

impl Document {
    fn migrate(&mut self) {
        if let Some(library) = self.library.take() {
            if !self.locations.iter().any(|l| l.path == library.path) {
                self.locations.insert(0, library);
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Reach {
    /// Reachable; a scan may still have failed part-way.
    Connected(Option<String>),
    Disconnected(String),
}

/// A registered file resolved under the store lock and opened after it is
/// released, so a slow location cannot block other Library requests.
pub struct MediaSource {
    location: Binding,
    registered_under: Option<source::RootIdentity>,
    path: String,
    fingerprint: source::Fingerprint,
}

impl MediaSource {
    pub fn open(&self) -> ReaderResult<File> {
        let (dir, identity) = source::probe(&self.location.path, source::PROBE_TIMEOUT)
            .map_err(|reason| unreachable_location(&self.location.path, &reason))?;
        let strict = identity == self.location.identity
            && self.registered_under.as_ref() == Some(&identity)
            && source::stable_file_ids(&dir);
        let opened = source::open(&dir, &self.path)?;
        if !self
            .fingerprint
            .matches(&source::Fingerprint::of(&opened)?, strict)
        {
            return Err("Source changed; explicit replacement confirmation is required".into());
        }
        Ok(opened)
    }
}

fn unreachable_location(path: &str, reason: &str) -> String {
    format!(
        "The Library location {path} is not available ({reason}). Plug in the drive or connect to the share, then try again."
    )
}

/// This database lives only in application data; it has no graph dependency.
pub struct ReaderStore {
    directory: PathBuf,
    document: Document,
    error: Option<String>,
    generation: u64,
    reach: HashMap<String, Reach>,
}

impl ReaderStore {
    pub fn load(directory: PathBuf) -> ReaderResult<Self> {
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let path = directory.join("reader.json");
        let document = match File::open(path) {
            Ok(file) => {
                if file.metadata().map_err(|e| e.to_string())?.len() > MAX_STATE_BYTES {
                    return Err("Private reader database exceeds the size limit".into());
                }
                let mut bytes = Vec::new();
                file.take(MAX_STATE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > MAX_STATE_BYTES {
                    return Err("Private reader database exceeds the size limit".into());
                }
                let mut document: Document = serde_json::from_slice(&bytes).map_err(|e| {
                    format!("Private reader database is damaged; it has not been reset: {e}")
                })?;
                // In memory only: the file is rewritten by the next change.
                document.migrate();
                validate(&document)?;
                document
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document {
                version: VERSION,
                library: None,
                locations: Vec::new(),
                books: Vec::new(),
            },
            Err(e) => return Err(e.to_string()),
        };
        let mut store = Self {
            directory,
            document,
            error: None,
            generation: 0,
            reach: HashMap::new(),
        };
        // Availability is always rechecked; persisted success is never authority.
        store.refresh_availability();
        Ok(store)
    }

    pub fn snapshot(&self) -> ReaderSnapshot {
        let books = self
            .document
            .books
            .iter()
            .map(|stored| {
                let mut book = stored.book.clone();
                let location = stored
                    .root
                    .as_ref()
                    .filter(|_| stored.book.source_url.is_none())
                    .and_then(|root| self.location(&root.path));
                book.location = location.map(|l| l.path.clone());
                book.disconnected = location.is_some_and(|l| self.is_disconnected(&l.path));
                book
            })
            .collect();
        ReaderSnapshot {
            library_path: self.document.locations.first().map(|l| l.path.clone()),
            locations: self
                .document
                .locations
                .iter()
                .map(|location| {
                    let reach = self.reach.get(&location.path);
                    ReaderLocation {
                        path: location.path.clone(),
                        connected: matches!(reach, Some(Reach::Connected(_))),
                        reason: match reach {
                            Some(Reach::Connected(error)) => error.clone(),
                            Some(Reach::Disconnected(reason)) => Some(reason.clone()),
                            None => Some("Not checked yet".into()),
                        },
                        items: self
                            .document
                            .books
                            .iter()
                            .filter(|b| belongs(b, &location.path))
                            .count(),
                    }
                })
                .collect(),
            books,
            error: self.error.clone(),
        }
    }

    fn location(&self, path: &str) -> Option<&Binding> {
        self.document.locations.iter().find(|l| l.path == path)
    }

    fn is_disconnected(&self, path: &str) -> bool {
        matches!(self.reach.get(path), Some(Reach::Disconnected(_)))
    }

    /// Items on disconnected locations, which keep their index entries.
    pub fn disconnected_ids(&self) -> HashSet<String> {
        self.document
            .books
            .iter()
            .filter(|stored| {
                stored
                    .root
                    .as_ref()
                    .is_some_and(|root| belongs(stored, &root.path) && self.is_disconnected(&root.path))
            })
            .map(|stored| stored.book.id.clone())
            .collect()
    }

    /// Whether any location is configured, and every item with the folder
    /// its files are relative to.
    pub fn index_records(&self) -> ReaderResult<(bool, Vec<ReaderIndexRecord>)> {
        Ok((
            !self.document.locations.is_empty(),
            self.document
                .books
                .iter()
                .map(|stored| {
                    let root = stored
                        .root
                        .as_ref()
                        .filter(|root| belongs(stored, &root.path) && self.location(&root.path).is_some())
                        .map(|root| root.path.clone());
                    ReaderIndexRecord {
                        disconnected: root.as_ref().is_some_and(|path| self.is_disconnected(path)),
                        root,
                        book: stored.book.clone(),
                        files: stored
                            .files
                            .iter()
                            .map(|file| ReaderIndexFile {
                                track_id: (stored.book.kind != ReaderKind::Epub)
                                    .then(|| file.id.clone()),
                                relative_path: file.path.clone(),
                                available: stored.book.source_url.is_none()
                                    && stored.book.tracks.iter().find(|t| t.id == file.id).map_or(
                                        stored.book.kind == ReaderKind::Epub
                                            && stored.book.available,
                                        |t| t.available,
                                    ),
                            })
                            .collect(),
                    }
                })
                .collect(),
        ))
    }

    fn commit(&mut self, mut document: Document) -> ReaderResult<()> {
        document.version = VERSION;
        validate(&document)?;
        let bytes = serde_json::to_vec(&document).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err("Private reader database exceeds the size limit".into());
        }
        atomic_write(&self.directory, &bytes)?;
        self.document = document;
        Ok(())
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn registered_files(&self, path: &str) -> Vec<String> {
        self.document
            .books
            .iter()
            .filter(|stored| belongs(stored, path))
            .flat_map(|stored| stored.files.iter().map(|file| file.path.clone()))
            .collect()
    }

    fn plan(&self, path: &str, discover: bool) -> Planned {
        Planned {
            path: path.to_owned(),
            registered: self.registered_files(path),
            discover,
        }
    }

    /// Recheck every location's registered files without discovering new
    /// ones. Changes are kept in memory until the next write.
    fn refresh_availability(&mut self) {
        let observed = self
            .document
            .locations
            .iter()
            .map(|location| {
                let planned = self.plan(&location.path, false);
                (planned.path.clone(), observe(&planned))
            })
            .collect();
        let mut document = self.document.clone();
        let reach = apply(&mut document, observed).1;
        settle_unlocated(&mut document);
        self.document = document;
        self.reach = reach.into_iter().collect();
        self.error = None;
    }

    #[cfg(test)]
    pub fn rescan(&mut self) -> ReaderResult<ReaderSnapshot> {
        let result = self.prepare_scan().and_then(ScanJob::run);
        self.finish_scan(result)
    }

    pub fn prepare_scan(&self) -> ReaderResult<ScanJob> {
        Ok(ScanJob {
            planned: self
                .document
                .locations
                .iter()
                .map(|location| self.plan(&location.path, true))
                .collect(),
            change: None,
            generation: self.generation,
        })
    }

    #[cfg(test)]
    pub fn add_location(&mut self, path: String) -> ReaderResult<ReaderSnapshot> {
        let result = self.prepare_add_location(path)?.run();
        self.finish_scan(result)
    }

    /// Only the new folder is scanned; items registered there before, while
    /// it was not a location, are attached to it again.
    pub fn prepare_add_location(&self, path: String) -> ReaderResult<ScanJob> {
        let path = location_path(&path)?;
        if self.document.locations.len() >= MAX_LOCATIONS {
            return Err(format!("A Library can have at most {MAX_LOCATIONS} locations"));
        }
        self.check_overlap(&path, None)?;
        Ok(ScanJob {
            planned: vec![self.plan(&path, true)],
            change: Some(Change::Add(path)),
            generation: self.generation,
        })
    }

    #[cfg(test)]
    pub fn move_location(&mut self, from: &str, to: String) -> ReaderResult<ReaderSnapshot> {
        let result = self.prepare_move_location(from, to)?.run();
        self.finish_scan(result)
    }

    /// Point a location at another folder, for example where a drive is now
    /// mounted. Its items keep their history and are matched by relative
    /// path, size and modification time.
    pub fn prepare_move_location(&self, from: &str, to: String) -> ReaderResult<ScanJob> {
        let to = location_path(&to)?;
        if self.location(from).is_none() {
            return Err("Unknown Library location".into());
        }
        if to == from {
            return Err("That folder is already this location".into());
        }
        self.check_overlap(&to, Some(from))?;
        let mut planned = self.plan(from, true);
        planned.path = to.clone();
        Ok(ScanJob {
            planned: vec![planned],
            change: Some(Change::Move {
                from: from.to_owned(),
                to,
            }),
            generation: self.generation,
        })
    }

    fn check_overlap(&self, path: &str, except: Option<&str>) -> ReaderResult<()> {
        for location in &self.document.locations {
            if Some(location.path.as_str()) == except {
                continue;
            }
            if location.path == path {
                return Err("That folder is already a Library location".into());
            }
            if Path::new(path).starts_with(&location.path) || Path::new(&location.path).starts_with(path) {
                return Err(format!(
                    "That folder overlaps the Library location {}. Choose a folder that neither contains it nor is inside it.",
                    location.path
                ));
            }
        }
        Ok(())
    }

    /// Forget a location and every item found in it, with its reading
    /// history. Nothing in the folder itself is touched.
    pub fn remove_location(&mut self, path: &str) -> ReaderResult<ReaderSnapshot> {
        if self.location(path).is_none() {
            return Err("Unknown Library location".into());
        }
        let mut document = self.document.clone();
        document.locations.retain(|location| location.path != path);
        document.books.retain(|stored| !belongs(stored, path));
        self.commit(document)?;
        self.reach.remove(path);
        self.generation = self.generation.wrapping_add(1);
        Ok(self.snapshot())
    }

    pub fn finish_scan(
        &mut self,
        result: ReaderResult<ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        let ScanResult {
            observed,
            change,
            generation,
        } = match result {
            Ok(result) => result,
            Err(error) => {
                self.error = Some(error);
                return Ok(self.snapshot());
            }
        };
        if generation != self.generation {
            return Err(RETRY_SCAN.into());
        }
        // Each folder that was scanned must still be the folder now there.
        for (path, observation) in &observed {
            if let Observation::Connected { identity, .. } = observation {
                if !source::probe(path, source::PROBE_TIMEOUT)
                    .is_ok_and(|(_, current)| &current == identity)
                {
                    return Err(RETRY_SCAN.into());
                }
            }
        }
        let mut document = self.document.clone();
        let mut changed = false;
        let mut forget = None;
        match &change {
            None => {}
            Some(Change::Add(path)) => {
                let identity = connected_identity(&observed, path)?;
                document.locations.push(Binding {
                    path: path.clone(),
                    identity,
                });
                changed = true;
            }
            Some(Change::Move { from, to }) => {
                connected_identity(&observed, to)?;
                let location = document
                    .locations
                    .iter_mut()
                    .find(|location| &location.path == from)
                    .ok_or(RETRY_SCAN)?;
                // The previous identity stays until the files are compared,
                // so they are matched as on a remounted drive.
                location.path = to.clone();
                for stored in document.books.iter_mut().filter(|b| belongs(b, from)) {
                    if let Some(root) = stored.root.as_mut() {
                        root.path = to.clone();
                    }
                }
                changed = true;
                forget = Some(from.clone());
            }
        }
        let (registered, reach) = apply(&mut document, observed);
        settle_unlocated(&mut document);
        self.commit(document)?;
        if let Some(from) = forget {
            self.reach.remove(&from);
        }
        let errors: Vec<String> = reach
            .iter()
            .filter_map(|(path, reach)| match reach {
                Reach::Connected(Some(error)) => Some(format!("{path}: {error}")),
                _ => None,
            })
            .collect();
        self.reach.extend(reach);
        if changed || registered {
            self.generation = self.generation.wrapping_add(1);
        }
        self.error = (!errors.is_empty()).then(|| errors.join("; "));
        Ok(self.snapshot())
    }

    pub fn media_source(&self, book_id: &str, track_id: Option<&str>) -> ReaderResult<MediaSource> {
        let book = self
            .document
            .books
            .iter()
            .find(|b| b.book.id == book_id)
            .ok_or("Unknown private book")?;
        if !book.authorized || book.book.source_url.is_some() {
            return Err("Book belongs to an unavailable library source".into());
        }
        let root = book.root.as_ref().ok_or("Book has no Library location")?;
        let location = self.location(&root.path).ok_or(REMOVED_LOCATION)?;
        let file = match (book.book.kind, track_id) {
            (ReaderKind::Audio | ReaderKind::Video, Some(id)) => {
                book.files.iter().find(|f| f.id == id)
            }
            (ReaderKind::Epub, None) => book.files.first(),
            _ => None,
        }
        .ok_or("Unknown source for this book")?;
        Ok(MediaSource {
            location: location.clone(),
            registered_under: Some(root.identity.clone()),
            path: file.path.clone(),
            fingerprint: file.fingerprint.clone(),
        })
    }

    #[cfg(test)]
    pub fn open_media(&self, book_id: &str, track_id: Option<&str>) -> ReaderResult<File> {
        self.media_source(book_id, track_id)?.open()
    }

    pub fn media_mime(&self, book_id: &str, track_id: &str) -> ReaderResult<&'static str> {
        let file = self
            .document
            .books
            .iter()
            .find(|b| b.book.id == book_id)
            .and_then(|b| b.files.iter().find(|f| f.id == track_id))
            .ok_or("Unknown media")?;
        Ok(source::media_mime(&file.path))
    }

    pub fn set_favorite(&mut self, book_id: &str, favorite: bool) -> ReaderResult<ReaderSnapshot> {
        let mut document = self.document.clone();
        book_mut(&mut document, book_id)?.book.favorite = favorite;
        self.commit(document)?;
        Ok(self.snapshot())
    }

    pub fn record_activity(
        &mut self,
        book_id: &str,
        progress: Option<ReaderProgress>,
    ) -> ReaderResult<ReaderSnapshot> {
        if let Some(progress) = &progress {
            progress.validate()?;
        }
        let mut document = self.document.clone();
        let book = &mut book_mut(&mut document, book_id)?.book;
        book.last_used_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;
        if let Some(progress) = progress {
            book.progress = Some(progress);
        }
        self.commit(document)?;
        Ok(self.snapshot())
    }

    pub fn add_link(
        &mut self,
        title: String,
        kind: ReaderKind,
        url: String,
    ) -> ReaderResult<ReaderSnapshot> {
        let url = normalize_link(kind, &url)?;
        let title = title.trim().to_string();
        if title.is_empty() || title.len() > 1024 || title.chars().any(char::is_control) {
            return Err("Library title must contain 1–1024 characters".into());
        }
        if self
            .document
            .books
            .iter()
            .any(|b| b.book.kind == kind && b.book.source_url.as_ref() == Some(&url))
        {
            return Ok(self.snapshot());
        }
        let mut document = self.document.clone();
        let book_id = id();
        document.books.push(StoredBook {
            book: ReaderBook {
                id: book_id.clone(),
                title,
                kind,
                available: true,
                tracks: vec![],
                position: None,
                bookmarks: vec![],
                favorite: false,
                last_used_at: 0,
                source_url: Some(url),
                progress: None,
                error: None,
                location: None,
                disconnected: false,
            },
            root: None,
            key: book_id,
            files: vec![],
            manual_order: false,
            authorized: false,
        });
        self.commit(document)?;
        Ok(self.snapshot())
    }

    #[cfg(test)]
    pub fn read_epub(&self, book_id: &str) -> ReaderResult<Vec<u8>> {
        let file = self.open_media(book_id, None)?;
        Self::read_epub_file(file)
    }

    pub fn read_epub_file(file: File) -> ReaderResult<Vec<u8>> {
        if file.metadata().map_err(|e| e.to_string())?.len() > source::MAX_EPUB_BYTES {
            return Err("EPUB exceeds the 128 MiB reader limit".into());
        }
        let mut bytes = Vec::new();
        file.take(source::MAX_EPUB_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > source::MAX_EPUB_BYTES {
            return Err("EPUB exceeds the 128 MiB reader limit".into());
        }
        Ok(bytes)
    }

    pub fn save_position(&mut self, book_id: &str, position: ReaderPosition) -> ReaderResult<()> {
        let mut document = self.document.clone();
        let book = book_mut(&mut document, book_id)?;
        position.validate(&book.book)?;
        book.book.position = Some(position);
        self.commit(document)
    }

    /// Persist bookmark and captured playback position in the same atomic write.
    pub fn add_bookmark(
        &mut self,
        book_id: &str,
        position: ReaderPosition,
        note: String,
    ) -> ReaderResult<ReaderBookmark> {
        if note.len() > 16_384 {
            return Err("Bookmark note exceeds 16 KiB".into());
        }
        let mut document = self.document.clone();
        let book = book_mut(&mut document, book_id)?;
        position.validate(&book.book)?;
        let bookmark = ReaderBookmark {
            id: id(),
            book_id: book_id.into(),
            position: position.clone(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis() as u64,
            note,
        };
        book.book.position = Some(position);
        book.book.bookmarks.push(bookmark.clone());
        self.commit(document)?;
        Ok(bookmark)
    }

    pub fn update_bookmark(
        &mut self,
        book_id: &str,
        bookmark_id: &str,
        note: String,
    ) -> ReaderResult<ReaderBookmark> {
        if note.len() > 16_384 {
            return Err("Bookmark note exceeds 16 KiB".into());
        }
        let mut document = self.document.clone();
        let book = book_mut(&mut document, book_id)?;
        let bookmark = book
            .book
            .bookmarks
            .iter_mut()
            .find(|bookmark| bookmark.id == bookmark_id)
            .ok_or("Unknown bookmark")?;
        bookmark.note = note;
        let updated = bookmark.clone();
        self.commit(document)?;
        Ok(updated)
    }

    pub fn delete_bookmark(&mut self, book_id: &str, bookmark_id: &str) -> ReaderResult<()> {
        let mut document = self.document.clone();
        let book = book_mut(&mut document, book_id)?;
        if !book.book.bookmarks.iter().any(|b| b.id == bookmark_id) {
            return Err("Unknown bookmark".into());
        }
        book.book.bookmarks.retain(|b| b.id != bookmark_id);
        self.commit(document)
    }

    pub fn reorder(
        &mut self,
        book_id: &str,
        track_ids: Vec<String>,
    ) -> ReaderResult<ReaderSnapshot> {
        let mut document = self.document.clone();
        let book = book_mut(&mut document, book_id)?;
        let expected: HashSet<_> = book.book.tracks.iter().map(|t| &t.id).collect();
        let actual: HashSet<_> = track_ids.iter().collect();
        if book.book.kind != ReaderKind::Audio
            || book.book.source_url.is_some()
            || expected != actual
            || actual.len() != track_ids.len()
        {
            return Err("Reorder must contain each track ID exactly once".into());
        }
        book.book.tracks = track_ids
            .iter()
            .map(|id| {
                book.book
                    .tracks
                    .iter()
                    .find(|t| &t.id == id)
                    .unwrap()
                    .clone()
            })
            .collect();
        book.manual_order = true;
        self.commit(document)?;
        Ok(self.snapshot())
    }

    #[cfg(test)]
    pub fn relink(
        &mut self,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
        location: Option<String>,
    ) -> ReaderResult<ReaderSnapshot> {
        let scan = self.prepare_relink(book_id, location)?.run()?;
        self.finish_relink(scan, book_id, relative_path, confirm_replacement)
    }

    /// Scan only the location holding the replacement: by default the
    /// item's own location.
    pub fn prepare_relink(&self, book_id: &str, location: Option<String>) -> ReaderResult<ScanJob> {
        let book = self
            .document
            .books
            .iter()
            .find(|b| b.book.id == book_id)
            .ok_or("Unknown private book")?;
        let path = match location {
            Some(path) => location_path(&path)?,
            None => book
                .root
                .as_ref()
                .map(|root| root.path.clone())
                .ok_or("Choose the Library location that holds the replacement")?,
        };
        if self.location(&path).is_none() {
            return Err("Choose a replacement inside one of your Library locations".into());
        }
        Ok(ScanJob {
            planned: vec![self.plan(&path, true)],
            change: None,
            generation: self.generation,
        })
    }

    pub fn finish_relink(
        &mut self,
        scan: ScanResult,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
    ) -> ReaderResult<ReaderSnapshot> {
        source::relative(&relative_path)?;
        let ScanResult {
            observed,
            generation,
            ..
        } = scan;
        let retry = || "Library registrations changed during relink; retry".to_string();
        if generation != self.generation || observed.len() != 1 {
            return Err(retry());
        }
        let (path, observation) = observed.into_iter().next().ok_or_else(retry)?;
        let previous = self.location(&path).cloned().ok_or_else(retry)?;
        let (identity, stable_ids, found) = match observation {
            Observation::Connected {
                identity,
                stable_ids,
                found: Some(found),
                ..
            } => (identity, stable_ids, found),
            Observation::Connected { error, .. } => {
                return Err(format!(
                    "{path} could not be scanned: {}",
                    error.unwrap_or_default()
                ))
            }
            Observation::Disconnected(reason) => return Err(unreachable_location(&path, &reason)),
        };
        if !source::probe(&path, source::PROBE_TIMEOUT)
            .is_ok_and(|(_, current)| current == identity)
        {
            return Err(retry());
        }
        let candidate = found
            .into_iter()
            .find(|b| b.key == relative_path)
            .ok_or("No discovered book at that relative path")?;
        let root = Binding {
            path: path.clone(),
            identity: identity.clone(),
        };
        let mut document = self.document.clone();
        if book_mut(&mut document, book_id)?.book.source_url.is_some() {
            return Err("External Library links cannot be relinked to local files".into());
        }
        if document
            .books
            .iter()
            .any(|b| b.book.id != book_id && belongs(b, &path) && b.key == candidate.key)
        {
            // A freshly discovered entry may be merged only if it has no user history.
            let duplicate = document
                .books
                .iter()
                .find(|b| b.book.id != book_id && belongs(b, &path) && b.key == candidate.key)
                .unwrap();
            if duplicate.book.position.is_some()
                || !duplicate.book.bookmarks.is_empty()
                || duplicate.book.favorite
                || duplicate.book.last_used_at != 0
                || duplicate.book.progress.is_some()
            {
                return Err("Relink destination already has reader history".into());
            }
            document
                .books
                .retain(|b| b.book.id == book_id || !belongs(b, &path) || b.key != candidate.key);
        }
        if let Some(location) = document.locations.iter_mut().find(|l| l.path == path) {
            location.identity = identity.clone();
        }
        let book = book_mut(&mut document, book_id)?;
        if candidate.kind != book.book.kind {
            return Err("Relink must keep the book format".into());
        }
        let same_location = book.root.as_ref().is_some_and(|r| r.path == path);
        let strict = stable_ids
            && previous.identity == identity
            && book.root.as_ref().is_some_and(|r| r.identity == identity);
        let mut replacements = Vec::new();
        let mut used = HashSet::new();
        for old in &book.files {
            let suffix = old
                .path
                .strip_prefix(&format!("{}/", book.key))
                .unwrap_or(&old.path);
            let target = candidate.files.iter().find(|(p, _)| {
                if candidate.epub
                    || !candidate.key.contains('/')
                        && candidate.files.len() == 1
                        && !old.path.contains('/')
                {
                    true
                } else {
                    p.strip_prefix(&format!("{}/", candidate.key)).unwrap_or(p) == suffix
                }
            });
            let (path, fingerprint) = target.ok_or("Relink cannot map every saved track by name; preserve chapter filenames and disc layout")?;
            if !used.insert(path.clone()) {
                return Err("Relink track mapping is ambiguous".into());
            }
            if (!old.fingerprint.matches(fingerprint, strict) || !same_location)
                && !confirm_replacement
            {
                return Err(
                    "Source fingerprint changed; confirm replacement before relinking".into(),
                );
            }
            replacements.push(RegisteredFile {
                id: old.id.clone(),
                path: path.clone(),
                fingerprint: fingerprint.clone(),
            });
        }
        for (path, fingerprint) in candidate.files {
            if !used.contains(&path) {
                replacements.push(RegisteredFile {
                    id: id(),
                    path,
                    fingerprint,
                });
            }
        }
        if matches!(book.book.kind, ReaderKind::Audio | ReaderKind::Video) {
            for old in &mut book.book.tracks {
                let new = replacements.iter().find(|f| f.id == old.id).unwrap();
                *old = track(new);
            }
            for file in &replacements {
                if !book.book.tracks.iter().any(|t| t.id == file.id) {
                    book.book.tracks.push(track(file));
                }
            }
            if !book.manual_order {
                book.book
                    .tracks
                    .sort_by(|a, b| source::natural_cmp(&a.relative_path, &b.relative_path));
            }
        }
        book.key = candidate.key;
        book.files = replacements;
        book.root = Some(root);
        book.authorized = true;
        book.book.available = true;
        book.book.error = None;
        self.commit(document)?;
        self.reach.insert(path, Reach::Connected(None));
        self.generation = self.generation.wrapping_add(1);
        Ok(self.snapshot())
    }

    pub fn export(&self) -> ReaderResult<String> {
        serde_json::to_string(&self.document).map_err(|e| e.to_string())
    }

    /// Backup data is not filesystem authority. Neither its locations nor
    /// its items' folders become active; restored items must be relinked.
    pub fn restore(&mut self, backup: &str) -> ReaderResult<ReaderSnapshot> {
        if backup.len() as u64 > MAX_STATE_BYTES {
            return Err("Reader backup exceeds 64 MiB".into());
        }
        let incoming: Document = serde_json::from_str(backup).map_err(|e| e.to_string())?;
        validate(&incoming)?;
        let mut document = self.document.clone();
        for mut book in incoming.books {
            if let Some(existing) = document
                .books
                .iter_mut()
                .find(|b| b.book.id == book.book.id)
            {
                if existing.key != book.key
                    || existing.root.as_ref().map(|r| &r.path)
                        != book.root.as_ref().map(|r| &r.path)
                    || existing.book.kind != book.book.kind
                    || existing.book.source_url != book.book.source_url
                    || existing
                        .files
                        .iter()
                        .map(|f| (&f.id, &f.path, &f.fingerprint))
                        .ne(book.files.iter().map(|f| (&f.id, &f.path, &f.fingerprint)))
                {
                    return Err(
                        "Backup conflicts with a registered source; relink it explicitly instead"
                            .into(),
                    );
                }
                if existing.book.position.is_none() {
                    existing.book.position = book.book.position;
                }
                for bookmark in book.book.bookmarks {
                    if !existing.book.bookmarks.iter().any(|b| b.id == bookmark.id) {
                        existing.book.bookmarks.push(bookmark);
                    }
                }
            } else {
                // Provenance remains comparable on repeated offline restores,
                // but imported metadata never grants filesystem authority.
                book.authorized = false;
                document.books.push(book);
            }
        }
        self.commit(document)?;
        self.generation = self.generation.wrapping_add(1);
        self.refresh_availability();
        Ok(self.snapshot())
    }
}

fn connected_identity(
    observed: &[(String, Observation)],
    path: &str,
) -> ReaderResult<source::RootIdentity> {
    match observed.iter().find(|(observed, _)| observed == path) {
        Some((_, Observation::Connected { identity, .. })) => Ok(identity.clone()),
        Some((_, Observation::Disconnected(reason))) => Err(format!("Cannot use {path}: {reason}")),
        None => Err(RETRY_SCAN.into()),
    }
}

/// The normalized form a location is stored and compared under.
fn location_path(path: &str) -> ReaderResult<String> {
    let normalized: PathBuf = Path::new(path).components().collect();
    if !normalized.is_absolute() {
        return Err("Choose an absolute local library folder".into());
    }
    normalized
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Library folder paths must be valid Unicode".into())
}

/// Bring each observed location's items up to date. Returns whether any
/// registration changed, and how each location could be reached.
fn apply(document: &mut Document, observed: Vec<(String, Observation)>) -> (bool, Vec<(String, Reach)>) {
    let mut changed = false;
    let mut reach = Vec::new();
    for (path, observation) in observed {
        let Some(index) = document.locations.iter().position(|l| l.path == path) else {
            continue;
        };
        match observation {
            Observation::Disconnected(reason) => {
                for stored in document.books.iter_mut().filter(|b| belongs(b, &path)) {
                    stored.book.available = false;
                    stored.book.error = None;
                    for track in &mut stored.book.tracks {
                        track.available = false;
                    }
                }
                reach.push((path, Reach::Disconnected(reason)));
            }
            Observation::Connected {
                identity,
                stable_ids,
                files,
                found,
                error,
            } => {
                // While a filesystem stays mounted its file IDs are compared
                // too. A remounted drive or share gives the same files new
                // IDs, so then only size and modification time can be.
                let same_mount = stable_ids && document.locations[index].identity == identity;
                if document.locations[index].identity != identity {
                    document.locations[index].identity = identity.clone();
                    changed = true;
                }
                let binding = document.locations[index].clone();
                if let Some(found) = found {
                    changed |= register(document, &binding, found);
                }
                for stored in document.books.iter_mut().filter(|b| belongs(b, &path)) {
                    let strict =
                        same_mount && stored.root.as_ref().is_some_and(|r| r.identity == identity);
                    verify(stored, &files, strict);
                    if stored.root.as_ref() != Some(&binding) {
                        stored.root = Some(binding.clone());
                        changed = true;
                    }
                }
                reach.push((path, Reach::Connected(error)));
            }
        }
    }
    (changed, reach)
}

/// Register newly found items, and new files of existing items. Existing
/// items keep their IDs and history.
fn register(document: &mut Document, binding: &Binding, found: Vec<source::Discovered>) -> bool {
    let mut changed = false;
    let existing: HashMap<_, _> = document
        .books
        .iter()
        .enumerate()
        .filter(|(_, book)| book.authorized && belongs(book, &binding.path))
        .map(|(index, book)| (book.key.clone(), index))
        .collect();
    for discovered in found {
        if let Some(index) = existing.get(&discovered.key) {
            let existing = &mut document.books[*index];
            let existing_paths: HashSet<_> =
                existing.files.iter().map(|f| f.path.clone()).collect();
            for (path, fingerprint) in discovered.files {
                if existing_paths.contains(&path) {
                    continue;
                }
                let file = RegisteredFile {
                    id: id(),
                    path,
                    fingerprint,
                };
                if existing.book.kind == ReaderKind::Audio {
                    existing.book.tracks.push(track(&file));
                }
                existing.files.push(file);
                changed = true;
            }
            if !existing.manual_order {
                existing
                    .book
                    .tracks
                    .sort_by(|a, b| source::natural_cmp(&a.relative_path, &b.relative_path));
            }
        } else {
            let files: Vec<_> = discovered
                .files
                .into_iter()
                .map(|(path, fingerprint)| RegisteredFile {
                    id: id(),
                    path,
                    fingerprint,
                })
                .collect();
            let book = ReaderBook {
                id: id(),
                title: discovered.title,
                kind: discovered.kind,
                available: true,
                tracks: if discovered.epub {
                    Vec::new()
                } else {
                    files.iter().map(track).collect()
                },
                position: None,
                bookmarks: Vec::new(),
                favorite: false,
                last_used_at: 0,
                source_url: None,
                progress: None,
                error: None,
                location: None,
                disconnected: false,
            };
            document.books.push(StoredBook {
                book,
                root: Some(binding.clone()),
                key: discovered.key,
                files,
                manual_order: false,
                authorized: true,
            });
            changed = true;
        }
    }
    changed
}

fn verify(stored: &mut StoredBook, files: &HashMap<String, source::Fingerprint>, strict: bool) {
    let authorized = stored.authorized;
    let mut all = authorized && !stored.files.is_empty();
    for file in &mut stored.files {
        let found = files
            .get(&file.path)
            .filter(|found| authorized && file.fingerprint.matches(found, strict));
        if let Some(found) = found {
            // The same file on a remounted drive: remember its new IDs.
            if file.fingerprint != *found {
                file.fingerprint = found.clone();
            }
        }
        all &= found.is_some();
        if let Some(track) = stored.book.tracks.iter_mut().find(|t| t.id == file.id) {
            track.available = found.is_some();
        }
    }
    stored.book.available = all;
    stored.book.error = if all {
        None
    } else if !authorized {
        Some(RESTORED_SOURCE.into())
    } else {
        Some(MISSING_SOURCE.into())
    };
}

/// Network links are always reachable. Local items outside every current
/// location keep their history until relinked.
fn settle_unlocated(document: &mut Document) {
    let paths: HashSet<_> = document.locations.iter().map(|l| l.path.clone()).collect();
    for stored in &mut document.books {
        if stored.book.source_url.is_some() {
            stored.book.available = true;
            stored.book.error = None;
        } else if !stored.root.as_ref().is_some_and(|root| paths.contains(&root.path)) {
            stored.book.available = false;
            stored.book.error = Some(REMOVED_LOCATION.into());
            for track in &mut stored.book.tracks {
                track.available = false;
            }
        }
    }
}

fn book_mut<'a>(document: &'a mut Document, id: &str) -> ReaderResult<&'a mut StoredBook> {
    document
        .books
        .iter_mut()
        .find(|b| b.book.id == id)
        .ok_or_else(|| "Unknown private book".into())
}

fn track(file: &RegisteredFile) -> ReaderTrack {
    ReaderTrack {
        id: file.id.clone(),
        title: Path::new(&file.path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        relative_path: file.path.clone(),
        available: true,
    }
}

fn validate(document: &Document) -> ReaderResult<()> {
    if !(1..=VERSION).contains(&document.version) {
        return Err("Unsupported private reader database version".into());
    }
    if document.locations.len() > MAX_LOCATIONS {
        return Err("Too many Library locations".into());
    }
    for (index, location) in document.locations.iter().enumerate() {
        if location_path(&location.path)? != location.path {
            return Err("Invalid Library location".into());
        }
        if document.locations[..index].iter().any(|other| {
            Path::new(&other.path).starts_with(&location.path)
                || Path::new(&location.path).starts_with(&other.path)
        }) {
            return Err("Library locations must not overlap".into());
        }
    }
    let mut ids = HashSet::new();
    let mut bookmark_ids = HashSet::new();
    if document.books.len() > 100_000 {
        return Err("Too many books in reader backup".into());
    }
    for stored in &document.books {
        source::relative(&stored.key)?;
        if uuid::Uuid::parse_str(&stored.book.id).is_err() || !ids.insert(&stored.book.id) {
            return Err("Invalid or duplicate book ID".into());
        }
        if stored.book.title.len() > 16_384
            || stored.files.len() > 100_000
            || stored.book.last_used_at > 9_007_199_254_740_991
        {
            return Err("Invalid book metadata".into());
        }
        if let Some(progress) = &stored.book.progress {
            progress.validate()?;
        }
        if let Some(url) = &stored.book.source_url {
            if normalize_link(stored.book.kind, url)? != *url
                || stored.root.is_some()
                || !stored.files.is_empty()
                || !stored.book.tracks.is_empty()
                || stored.authorized
                || stored.manual_order
            {
                return Err("Invalid external Library registration".into());
            }
        } else if stored.root.is_none()
            || stored.files.is_empty()
            || stored.book.kind == ReaderKind::Youtube
        {
            return Err("Missing local Library registration".into());
        }
        let mut files = HashSet::new();
        let mut paths = HashSet::new();
        for file in &stored.files {
            source::relative(&file.path)?;
            if uuid::Uuid::parse_str(&file.id).is_err()
                || !files.insert(&file.id)
                || !paths.insert(&file.path)
            {
                return Err("Invalid or duplicate source ID/path".into());
            }
            if source::media_kind(&file.path) != Some(stored.book.kind) {
                return Err("Source extension does not match book format".into());
            }
        }
        if stored.book.kind == ReaderKind::Epub
            && (!stored.book.tracks.is_empty() || stored.files.len() != 1)
        {
            return Err("Invalid EPUB source registration".into());
        }
        if matches!(stored.book.kind, ReaderKind::Audio | ReaderKind::Video) {
            let tracks: HashSet<_> = stored.book.tracks.iter().map(|t| &t.id).collect();
            if tracks != files || tracks.len() != stored.book.tracks.len() {
                return Err("Invalid registered track list".into());
            }
            let sources: HashMap<_, _> = stored.files.iter().map(|f| (&f.id, &f.path)).collect();
            for track in &stored.book.tracks {
                if sources.get(&track.id).copied() != Some(&track.relative_path) {
                    return Err("Track source does not match registration".into());
                }
            }
        }
        if let Some(position) = &stored.book.position {
            position.validate(&stored.book)?;
        }
        for bookmark in &stored.book.bookmarks {
            if bookmark.book_id != stored.book.id
                || uuid::Uuid::parse_str(&bookmark.id).is_err()
                || !bookmark_ids.insert(&bookmark.id)
                || bookmark.note.len() > 16_384
            {
                return Err("Invalid reader bookmark".into());
            }
            bookmark.position.validate(&stored.book)?;
        }
    }
    Ok(())
}

fn atomic_write(directory: &Path, bytes: &[u8]) -> ReaderResult<()> {
    let pending = directory.join(format!(".reader-{}.pending", id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&pending).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&pending, directory.join("reader.json")).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(pending);
    }
    result
}
