use super::{source, types::*};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 2;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    path: String,
    identity: source::RootIdentity,
}

pub struct ScanJob {
    root: Binding,
    directory: cap_std::fs::Dir,
    generation: u64,
}

pub struct ScanResult {
    root: Binding,
    found: Vec<source::Discovered>,
    generation: u64,
}

impl ScanJob {
    pub fn library(path: String, generation: u64) -> ReaderResult<Self> {
        let (directory, identity) = source::root(&path)?;
        Ok(Self {
            root: Binding { path, identity },
            directory,
            generation,
        })
    }

    pub fn run(self) -> ReaderResult<ScanResult> {
        Ok(ScanResult {
            found: source::discover(&self.directory)?,
            root: self.root,
            generation: self.generation,
        })
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    version: u32,
    library: Option<Binding>,
    books: Vec<StoredBook>,
}

/// This database lives only in application data; it has no graph dependency.
pub struct ReaderStore {
    directory: PathBuf,
    document: Document,
    error: Option<String>,
    generation: u64,
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
                let document: Document = serde_json::from_slice(&bytes).map_err(|e| {
                    format!("Private reader database is damaged; it has not been reset: {e}")
                })?;
                validate(&document)?;
                document
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document {
                version: VERSION,
                library: None,
                books: Vec::new(),
            },
            Err(e) => return Err(e.to_string()),
        };
        let mut store = Self {
            directory,
            document,
            error: None,
            generation: 0,
        };
        // Availability is always rechecked; persisted success is never authority.
        store.refresh_availability();
        Ok(store)
    }

    pub fn snapshot(&self) -> ReaderSnapshot {
        ReaderSnapshot {
            library_path: self.document.library.as_ref().map(|r| r.path.clone()),
            books: self.document.books.iter().map(|b| b.book.clone()).collect(),
            error: self.error.clone(),
        }
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

    pub fn set_library(&mut self, path: String) -> ReaderResult<ReaderSnapshot> {
        let scan = ScanJob::library(path, self.generation)?.run()?;
        self.finish_set_library(scan)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn finish_set_library(&mut self, scan: ScanResult) -> ReaderResult<ReaderSnapshot> {
        self.finish_scan_inner(Ok(scan), true)
    }

    fn active_root(&self) -> ReaderResult<(cap_std::fs::Dir, &Binding)> {
        let binding = self
            .document
            .library
            .as_ref()
            .ok_or("Choose a library folder in Settings")?;
        let (dir, identity) = source::root(&binding.path)?;
        if identity != binding.identity {
            return Err("The library folder was replaced; select it again and explicitly relink affected books".into());
        }
        Ok((dir, binding))
    }

    fn refresh_availability(&mut self) {
        let root = self
            .active_root()
            .map(|(dir, binding)| (dir, binding.clone()));
        match root {
            Ok((dir, binding)) => {
                self.error = None;
                for book in &mut self.document.books {
                    if book.book.source_url.is_some() {
                        book.book.available = true;
                        book.book.error = None;
                        continue;
                    }
                    let same_root = book.authorized && book.root.as_ref() == Some(&binding);
                    let mut all = same_root;
                    for file in &book.files {
                        let available = same_root
                            && source::open(&dir, &file.path)
                                .and_then(|f| source::Fingerprint::of(&f))
                                .is_ok_and(|f| f == file.fingerprint);
                        all &= available;
                        if let Some(track) = book.book.tracks.iter_mut().find(|t| t.id == file.id) {
                            track.available = available;
                        }
                    }
                    book.book.available = all && !book.files.is_empty();
                    book.book.error = (!book.book.available).then(|| "Source missing, inaccessible, or replaced. History is retained; relink explicitly.".into());
                }
            }
            Err(error) => {
                self.error = self.document.library.as_ref().map(|_| error);
                for book in &mut self.document.books {
                    if book.book.source_url.is_some() {
                        book.book.available = true;
                        book.book.error = None;
                        continue;
                    }
                    book.book.available = false;
                    book.book.error = Some("Library unavailable; history is retained".into());
                    for track in &mut book.book.tracks {
                        track.available = false;
                    }
                }
            }
        }
    }

    pub fn rescan(&mut self) -> ReaderResult<ReaderSnapshot> {
        let result = self.prepare_scan().and_then(ScanJob::run);
        self.finish_scan(result)
    }

    pub fn prepare_scan(&self) -> ReaderResult<ScanJob> {
        let (directory, root) = self.active_root()?;
        Ok(ScanJob {
            directory,
            root: root.clone(),
            generation: self.generation,
        })
    }

    pub fn finish_scan(
        &mut self,
        result: ReaderResult<ScanResult>,
    ) -> ReaderResult<ReaderSnapshot> {
        self.finish_scan_inner(result, false)
    }

    fn finish_scan_inner(
        &mut self,
        result: ReaderResult<ScanResult>,
        select_library: bool,
    ) -> ReaderResult<ReaderSnapshot> {
        let ScanResult {
            found,
            root,
            generation,
        } = match result {
            Ok(result) => result,
            Err(error) => {
                self.refresh_availability();
                self.error = Some(error);
                return Ok(self.snapshot());
            }
        };
        if generation != self.generation
            || (!select_library && self.document.library.as_ref() != Some(&root))
            || !source::root(&root.path).is_ok_and(|(_, identity)| identity == root.identity)
        {
            return Err("Library registrations changed during scan; retry the scan".into());
        }
        let availability: HashMap<_, _> = found
            .iter()
            .flat_map(|book| book.files.iter().cloned())
            .collect();
        let mut document = self.document.clone();
        if select_library {
            document.library = Some(root.clone());
        }
        let existing_books: HashMap<_, _> = document
            .books
            .iter()
            .enumerate()
            .filter(|(_, book)| {
                book.authorized && book.root.as_ref().is_some_and(|b| b.path == root.path)
            })
            .map(|(index, book)| (book.key.clone(), index))
            .collect();
        for discovered in found {
            if let Some(index) = existing_books.get(&discovered.key) {
                let existing = &mut document.books[*index];
                // Do not adopt replacement roots/files, even when the path matches.
                if existing.root.as_ref() != Some(&root) {
                    continue;
                }
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
                };
                document.books.push(StoredBook {
                    book,
                    root: Some(root.clone()),
                    key: discovered.key,
                    files,
                    manual_order: false,
                    authorized: true,
                });
            }
        }
        for stored in &mut document.books {
            if stored.book.source_url.is_some() {
                continue;
            }
            let same_root = stored.authorized && stored.root.as_ref() == Some(&root);
            let statuses: HashMap<_, _> = stored
                .files
                .iter()
                .map(|file| {
                    (
                        &file.id,
                        same_root && availability.get(&file.path) == Some(&file.fingerprint),
                    )
                })
                .collect();
            stored.book.available = !statuses.is_empty() && statuses.values().all(|value| *value);
            for track in &mut stored.book.tracks {
                track.available = statuses.get(&track.id).copied().unwrap_or(false);
            }
            stored.book.error = (!stored.book.available).then(|| "Source missing, inaccessible, or replaced. History is retained; relink explicitly.".into());
        }
        self.commit(document)?;
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        Ok(self.snapshot())
    }

    pub fn open_media(&self, book_id: &str, track_id: Option<&str>) -> ReaderResult<File> {
        let (dir, active) = self.active_root()?;
        let book = self
            .document
            .books
            .iter()
            .find(|b| b.book.id == book_id)
            .ok_or("Unknown private book")?;
        if !book.authorized || book.book.source_url.is_some() || book.root.as_ref() != Some(active)
        {
            return Err("Book belongs to an unavailable library source".into());
        }
        let file = match (book.book.kind, track_id) {
            (ReaderKind::Audio | ReaderKind::Video, Some(id)) => {
                book.files.iter().find(|f| f.id == id)
            }
            (ReaderKind::Epub, None) => book.files.first(),
            _ => None,
        }
        .ok_or("Unknown source for this book")?;
        let opened = source::open(&dir, &file.path)?;
        if source::Fingerprint::of(&opened)? != file.fingerprint {
            return Err("Source changed; explicit replacement confirmation is required".into());
        }
        Ok(opened)
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

    pub fn relink(
        &mut self,
        book_id: &str,
        relative_path: String,
        confirm_replacement: bool,
    ) -> ReaderResult<ReaderSnapshot> {
        let scan = self.prepare_scan()?.run()?;
        self.finish_relink(scan, book_id, relative_path, confirm_replacement)
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
            root,
            found,
            generation,
        } = scan;
        if generation != self.generation
            || self.document.library.as_ref() != Some(&root)
            || !source::root(&root.path).is_ok_and(|(_, identity)| identity == root.identity)
        {
            return Err("Library registrations changed during relink; retry".into());
        }
        let candidate = found
            .into_iter()
            .find(|b| b.key == relative_path)
            .ok_or("No discovered book at that relative path")?;
        let mut document = self.document.clone();
        if book_mut(&mut document, book_id)?.book.source_url.is_some() {
            return Err("External Library links cannot be relinked to local files".into());
        }
        if document.books.iter().any(|b| {
            b.book.id != book_id
                && b.root.as_ref().is_some_and(|r| r.path == root.path)
                && b.key == candidate.key
        }) {
            // A freshly discovered entry may be merged only if it has no user history.
            let duplicate = document
                .books
                .iter()
                .find(|b| {
                    b.book.id != book_id
                        && b.root.as_ref().is_some_and(|r| r.path == root.path)
                        && b.key == candidate.key
                })
                .unwrap();
            if duplicate.book.position.is_some()
                || !duplicate.book.bookmarks.is_empty()
                || duplicate.book.favorite
                || duplicate.book.last_used_at != 0
                || duplicate.book.progress.is_some()
            {
                return Err("Relink destination already has reader history".into());
            }
            document.books.retain(|b| {
                b.book.id == book_id
                    || !b.root.as_ref().is_some_and(|r| r.path == root.path)
                    || b.key != candidate.key
            });
        }
        let book = book_mut(&mut document, book_id)?;
        if candidate.kind != book.book.kind {
            return Err("Relink must keep the book format".into());
        }
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
            if (&old.fingerprint != fingerprint || book.root.as_ref() != Some(&root))
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
        self.generation = self.generation.wrapping_add(1);
        Ok(self.snapshot())
    }

    pub fn export(&self) -> ReaderResult<String> {
        serde_json::to_string(&self.document).map_err(|e| e.to_string())
    }

    /// Backup data is not filesystem authority. Only the currently user-selected
    /// root remains active; imported roots must subsequently be selected/relinked.
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
