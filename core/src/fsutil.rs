//! Filesystem helpers shared by the graph and sync layers.

use crate::error::Result;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use uuid::Uuid;

pub type GraphOperationLock = Arc<parking_lot::ReentrantMutex<()>>;
#[derive(Clone, Copy, Default)]
struct SourceGeneration {
    direct: u64,
    subtree: u64,
}
#[derive(Default)]
struct SourceGenerationState {
    epoch: u64,
    paths: HashMap<PathBuf, SourceGeneration>,
}
type SourceGenerations = Arc<parking_lot::Mutex<SourceGenerationState>>;

struct SourceDomain {
    operation: Weak<parking_lot::ReentrantMutex<()>>,
    generations: SourceGenerations,
}

static SOURCE_DOMAINS: OnceLock<Mutex<HashMap<PathBuf, SourceDomain>>> = OnceLock::new();

fn source_domain(root: &Path) -> Result<(GraphOperationLock, SourceGenerations)> {
    let key = root.canonicalize()?;
    let mut domains = SOURCE_DOMAINS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| crate::CoreError::Other("Graph operation registry poisoned".into()))?;
    domains.retain(|_, domain| domain.operation.strong_count() > 0);
    if let Some(domain) = domains.get(&key) {
        if let Some(operation) = domain.operation.upgrade() {
            return Ok((operation, domain.generations.clone()));
        }
    }
    let operation = Arc::new(parking_lot::ReentrantMutex::new(()));
    let generations = Arc::new(parking_lot::Mutex::new(SourceGenerationState::default()));
    domains.insert(
        key,
        SourceDomain {
            operation: Arc::downgrade(&operation),
            generations: generations.clone(),
        },
    );
    Ok((operation, generations))
}

fn source_key(relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err(crate::CoreError::Other(
            "Source generation paths must be graph-relative".into(),
        ));
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    return Ok(PathBuf::from(relative.to_string_lossy().to_lowercase()));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    Ok(relative.to_path_buf())
}

/// An in-flight operation's per-path mutation fence. This retains its graph
/// lock/domain across network waits, but does not hold a mutex during them.
#[derive(Clone)]
pub struct SourceMutationFence {
    root: PathBuf,
    operation: GraphOperationLock,
    generations: SourceGenerations,
    path: PathBuf,
    generation: u64,
}

impl SourceMutationFence {
    pub fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, ()> {
        self.operation.lock()
    }

    /// Check while holding `lock()`, together with content/existence revision
    /// checks and publication. A changed generation rejects absent→present→
    /// absent and same-byte rewrites that a content hash alone cannot detect.
    pub fn is_current(&self) -> bool {
        let _operation = self.operation.lock();
        if !self.root.is_dir() {
            return false;
        }
        let generations = self.generations.lock();
        if generations
            .paths
            .get(&self.path)
            .copied()
            .unwrap_or_default()
            .subtree
            > self.generation
        {
            return false;
        }
        let mut parent = self.path.parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            if generations
                .paths
                .get(path)
                .copied()
                .unwrap_or_default()
                .direct
                > self.generation
            {
                return false;
            }
            parent = path.parent();
        }
        true
    }
}

/// Capture before reading the initial local revision. Release `fence.lock()`
/// for network I/O; reacquire it and check `is_current()` plus the original
/// content/existence revision before local publication.
pub fn source_mutation_fence(root: &Path, relative: &Path) -> Result<SourceMutationFence> {
    graph_mutation_epoch(root)?.for_path(relative)
}

/// Capture once at job/snapshot start, before remote listings or downloads.
/// Paths discovered later can still be checked against that initial epoch.
#[derive(Clone)]
pub struct GraphMutationEpoch {
    root: PathBuf,
    operation: GraphOperationLock,
    generations: SourceGenerations,
    generation: u64,
}

impl GraphMutationEpoch {
    pub fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, ()> {
        self.operation.lock()
    }

    pub fn for_path(&self, relative: &Path) -> Result<SourceMutationFence> {
        Ok(SourceMutationFence {
            root: self.root.clone(),
            operation: self.operation.clone(),
            generations: self.generations.clone(),
            generation: self.generation,
            path: source_key(relative)?,
        })
    }
}

pub fn graph_mutation_epoch(root: &Path) -> Result<GraphMutationEpoch> {
    let root = root.canonicalize()?;
    let (operation, generations) = source_domain(&root)?;
    let generation = {
        let _operation = operation.lock();
        generations.lock().epoch
    };
    Ok(GraphMutationEpoch {
        root,
        operation,
        generations,
        generation,
    })
}

/// Record a direct filesystem publication/deletion under the shared graph
/// operation lock. Graph methods and atomic_write do this automatically;
/// native recovery/sync code using rename/remove directly must call it too.
pub fn record_source_mutation(root: &Path, relative: &Path) -> Result<()> {
    let path = source_key(relative)?;
    let (operation, generations) = source_domain(root)?;
    let _operation = operation.lock();
    advance_generation(&generations, &path)
}

fn advance_generation(generations: &SourceGenerations, path: &Path) -> Result<()> {
    let mut generations = generations.lock();
    let generation = generations
        .epoch
        .checked_add(1)
        .ok_or_else(|| crate::CoreError::Other("Source generation exhausted".into()))?;
    generations.epoch = generation;
    generations
        .paths
        .entry(path.to_path_buf())
        .or_default()
        .direct = generation;
    let mut ancestor = Some(path);
    while let Some(path) = ancestor.filter(|path| !path.as_os_str().is_empty()) {
        generations
            .paths
            .entry(path.to_path_buf())
            .or_default()
            .subtree = generation;
        ancestor = path.parent();
    }
    Ok(())
}

fn registered_publication(
    path: &Path,
) -> Result<Option<(GraphOperationLock, SourceGenerations, PathBuf)>> {
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let Some(name) = path.file_name() else {
        return Ok(None);
    };
    let absolute = parent.join(name);
    let Some(registry) = SOURCE_DOMAINS.get() else {
        return Ok(None);
    };
    let domains = registry
        .lock()
        .map_err(|_| crate::CoreError::Other("Graph operation registry poisoned".into()))?;
    let candidate = domains
        .iter()
        .filter(|(root, _)| absolute.starts_with(root))
        .filter_map(|(root, domain)| {
            domain
                .operation
                .upgrade()
                .map(|operation| (root, domain, operation))
        })
        .max_by_key(|(root, _, _)| root.components().count());
    let Some((root, domain, operation)) = candidate else {
        return Ok(None);
    };
    let relative = absolute
        .strip_prefix(root)
        .map_err(|_| crate::CoreError::Other("Publication escaped graph root".into()))?;
    if !is_authoritative_source(relative) {
        return Ok(None);
    }
    Ok(Some((
        operation,
        domain.generations.clone(),
        source_key(relative)?,
    )))
}

/// All synchronous source publication/index/deletion operations for one graph
/// share this lock, even when they use different Graph/database handles.
/// Callers doing network I/O must fetch first, then lock and revalidate the
/// local revision before publishing. Reentrancy supports nested Graph APIs.
pub fn graph_operation_lock(root: &Path) -> Result<GraphOperationLock> {
    Ok(source_domain(root)?.0)
}

/// Conflict copies and staged/recovery files are user recovery artifacts,
/// never independent authoritative sources.
/// Pass a graph-relative path. A dot-prefixed Markdown leaf may still be an
/// authored note; hidden support directories and scratch files are excluded.
pub fn is_authoritative_source(path: &Path) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        if let std::path::Component::Normal(name) = component {
            let name = name.to_string_lossy();
            if name.contains(".conflict_") {
                return false;
            }
            if name.starts_with('.')
                && !(components.peek().is_none()
                    && path.extension().is_some_and(|extension| extension == "md"))
            {
                return false;
            }
        }
    }
    true
}

pub fn is_authoritative_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md") && is_authoritative_source(path)
}

/// Write `content` to `path` so that readers only ever observe the complete
/// old or the complete new file.
///
/// The data is written to a temporary file in the same directory, flushed and
/// fsynced, then renamed over the target. This matters most for removable
/// drives: a stick pulled mid-write can otherwise leave a truncated note or a
/// corrupt sync state file behind.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }

    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp_path = dir.join(format!(".{}.{}.tmp", file_name, Uuid::new_v4().as_simple()));

    // Scope the handle so it is closed before the rename (required on
    // Windows, harmless elsewhere).
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(content)?;
        file.flush()?;
        // Durability: without this the rename can land before the data.
        file.sync_all()?;
    }

    let publication = registered_publication(path)?;
    let _operation = publication
        .as_ref()
        .map(|(operation, _, _)| operation.lock());
    if let Some((_, generations, relative)) = &publication {
        advance_generation(generations, relative)?;
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        // Never leave scratch files behind in the user's graph.
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }

    Ok(())
}
