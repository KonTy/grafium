//! Explicit, conservative cleanup; a missing reference is never proof of disuse.

use super::*;
use std::io::{Read, Write};

static NUMERIC_ENTITY: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(r"&#([xX][0-9a-fA-F]+|[0-9]+);").expect("static entity regex")
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanedAsset {
    pub filename: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct AssetCleanupScan {
    pub graph_path: String,
    pub assets: Vec<OrphanedAsset>,
}

#[derive(Debug, Default, Serialize)]
pub struct AssetCleanupResult {
    pub moved: Vec<String>,
    pub trash_path: Option<String>,
    pub errors: Vec<String>,
}

fn error(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn text_source(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "md" | "markdown"
            | "org"
            | "txt"
            | "json"
            | "jsonld"
            | "yaml"
            | "yml"
            | "toml"
            | "edn"
            | "css"
            | "html"
            | "htm"
            | "svg"
            | "xml"
            | "ini"
            | "conf"
            | "js"
            | "ts"
            | "csv"
            | "rst"
            | "tex"
    )
}

pub(super) fn protected_source(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "md" | "markdown" | "org" | "jsonld"
    )
}

fn walk(
    root: &Path,
    dir: &Path,
    in_assets: bool,
    assets: &mut Vec<PathBuf>,
    sources: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in
        fs::read_dir(dir).map_err(|e| error(format!("Cannot scan {}: {e}", dir.display())))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| error(format!("Non-UTF-8 path in {}", dir.display())))?;
        let relative = path.strip_prefix(root).map_err(|e| error(e.to_string()))?;
        if name == ".git" || relative == Path::new(".grafium/asset-trash") {
            continue;
        }
        let kind = entry.file_type()?;
        // A linked note can contain references we cannot safely inspect. Do not
        // turn an incomplete scan into an apparently safe list of candidates.
        if kind.is_symlink() {
            return Err(error(format!("Asset scan stopped at symbolic link {}. Cleanup requires a graph without linked files or folders.", relative.display())));
        }
        if kind.is_dir() {
            walk(root, &path, in_assets || name == "assets", assets, sources)?;
        } else if kind.is_file() {
            if text_source(&path) {
                sources.push(path.clone());
            }
            if in_assets
                && !relative
                    .components()
                    .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                && !protected_source(&path)
            {
                assets.push(path);
            }
        } else {
            return Err(error(format!(
                "Cannot safely scan special file {}",
                relative.display()
            )));
        }
    }
    Ok(())
}

fn normalized(text: &str) -> String {
    let mut decoded = Vec::with_capacity(text.len());
    let mut bytes = text.as_bytes().iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let mut rest = bytes.clone();
            if let (Some(a), Some(b)) = (rest.next(), rest.next()) {
                if let (Some(a), Some(b)) = ((a as char).to_digit(16), (b as char).to_digit(16)) {
                    decoded.push((a * 16 + b) as u8);
                    bytes = rest;
                    continue;
                }
            }
        }
        decoded.push(byte);
    }
    let text = String::from_utf8_lossy(&decoded)
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    let text = NUMERIC_ENTITY.replace_all(&text, |captures: &regex::Captures<'_>| {
        let digits = &captures[1];
        let value = if let Some(hex) = digits.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            digits.parse().ok()
        };
        value
            .and_then(char::from_u32)
            .map(|c| c.to_string())
            .unwrap_or_else(|| captures[0].to_string())
    });
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|c| c.is_ascii_punctuation()) {
            continue;
        }
        output.push(c);
    }
    output.to_lowercase()
}

fn reference_text(content: &str) -> String {
    fn strings(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::String(s) => {
                out.push('\n');
                out.push_str(s);
            }
            serde_json::Value::Array(values) => values.iter().for_each(|v| strings(v, out)),
            serde_json::Value::Object(values) => values.values().for_each(|v| strings(v, out)),
            _ => {}
        }
    }
    let mut text = content.to_string();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
        strings(&json, &mut text);
    }
    if content.contains('&') {
        let html = scraper::Html::parse_fragment(content);
        for node in html.tree.nodes() {
            if let Some(value) = node.value().as_text() {
                text.push('\n');
                text.push_str(value);
            }
            if let Some(element) = node.value().as_element() {
                for (_, value) in element.attrs() {
                    text.push('\n');
                    text.push_str(value);
                }
            }
        }
    }
    normalized(&text)
}

pub(super) fn fingerprint(path: &Path) -> Result<(u64, String)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&buffer[..count]);
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

/// Check every component, not just the canonical destination: symlinked media
/// and recovery folders must not make cleanup write outside the graph.
pub(super) fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(error("Cleanup paths must be graph-relative"));
        };
        path.push(name);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(error(format!("Symbolic link rejected: {}", path.display())));
        }
    }
    Ok(path)
}

pub(super) fn create_directory(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(error("Recovery paths must be graph-relative"));
        };
        path.push(name);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(error(format!(
                "Unsafe recovery directory: {}",
                path.display()
            )));
        }
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(path)
}

impl Graph {
    fn asset_reference_texts(&self, sources: Vec<PathBuf>) -> Result<Vec<String>> {
        let mut references = self
            .db
            .get_all_media_references()?
            .iter()
            .map(|text| reference_text(text))
            .collect::<Vec<_>>();
        for source in sources {
            let text = fs::read_to_string(&source).map_err(|e| {
                error(format!(
                    "Cannot inspect references in {}: {e}",
                    source.display()
                ))
            })?;
            references.push(reference_text(&text));
        }
        Ok(references)
    }

    pub(super) fn asset_is_referenced(&self, path: &Path) -> Result<bool> {
        let root = self.root_dir.canonicalize()?;
        let mut sources = Vec::new();
        walk(&root, &root, false, &mut Vec::new(), &mut sources)?;
        let name = normalized(
            path.file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| error("Invalid asset filename"))?,
        );
        Ok(self
            .asset_reference_texts(sources)?
            .iter()
            .any(|text| text.contains(&name)))
    }

    pub fn scan_unused_assets(&self) -> Result<AssetCleanupScan> {
        self.scan_unused_assets_matching(None)
    }

    pub(super) fn scan_unused_assets_matching(
        &self,
        candidates: Option<&HashSet<String>>,
    ) -> Result<AssetCleanupScan> {
        let _operation = self.source_operations.lock();
        let root = self.root_dir.canonicalize()?;
        let mut assets = Vec::new();
        let mut sources = Vec::new();
        walk(&root, &root, false, &mut assets, &mut sources)?;
        let references = self.asset_reference_texts(sources)?;
        let mut unused = Vec::new();
        for path in assets {
            let relative = path
                .strip_prefix(&root)
                .map_err(|e| error(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if candidates.is_some_and(|candidates| !candidates.contains(&relative)) {
                continue;
            }
            let name = normalized(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| error("Invalid asset filename"))?,
            );
            // Bare-name matching intentionally retains duplicates and ambiguous
            // mentions, regardless of link syntax or page-relative path style.
            if references.iter().any(|text| text.contains(&name)) {
                continue;
            }
            let (size, sha256) = fingerprint(&path)?;
            unused.push(OrphanedAsset {
                filename: relative,
                size,
                sha256,
            });
        }
        unused.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(AssetCleanupScan {
            graph_path: root.to_string_lossy().into_owned(),
            assets: unused,
        })
    }

    pub fn trash_unused_assets(
        &self,
        graph_path: &str,
        requested: &[OrphanedAsset],
    ) -> Result<AssetCleanupResult> {
        let _operation = self.source_operations.lock();
        let root = self.root_dir.canonicalize()?;
        if root.to_string_lossy() != graph_path {
            return Err(error("The active graph changed. Scan its assets again."));
        }
        let mut result = AssetCleanupResult::default();
        if requested.is_empty() {
            return Ok(result);
        }
        let candidates = requested
            .iter()
            .map(|asset| asset.filename.clone())
            .collect();
        let scan = self.scan_unused_assets_matching(Some(&candidates))?;
        let current: HashMap<_, _> = scan
            .assets
            .iter()
            .map(|asset| (asset.filename.as_str(), asset))
            .collect();
        let batch = PathBuf::from(".grafium/asset-trash").join(Uuid::new_v4().to_string());
        let mut staged = Vec::new();
        let mut seen = HashSet::new();
        for asset in requested {
            if !seen.insert(&asset.filename) {
                continue;
            }
            let prepare = || -> Result<()> {
                let now = current
                    .get(asset.filename.as_str())
                    .ok_or_else(|| error("No longer an unused asset; rescan before continuing"))?;
                if now.sha256 != asset.sha256 || now.size != asset.size {
                    return Err(error(
                        "File changed since preview; rescan before continuing",
                    ));
                }
                let source = checked_path(&root, Path::new(&asset.filename))?;
                let relative = batch.join(&asset.filename);
                create_directory(
                    &root,
                    relative
                        .parent()
                        .ok_or_else(|| error("Invalid recovery path"))?,
                )?;
                let destination = root.join(relative);
                let mut backup = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)?;
                std::io::copy(&mut fs::File::open(&source)?, &mut backup)?;
                backup.flush()?;
                backup.sync_all()?;
                if fingerprint(&destination)? != (asset.size, asset.sha256.clone()) {
                    return Err(error(
                        "Recovery copy did not match preview; original left unchanged",
                    ));
                }
                #[cfg(unix)]
                fs::File::open(
                    destination
                        .parent()
                        .ok_or_else(|| error("Invalid recovery directory"))?,
                )?
                .sync_all()?;
                Ok(())
            };
            match prepare() {
                Ok(()) => staged.push(asset),
                Err(e) => result.errors.push(format!("{}: {e}", asset.filename)),
            }
        }
        if root.join(&batch).exists() {
            result.trash_path = Some(root.join(&batch).to_string_lossy().into_owned());
        }
        // Copying may take time. Re-read disk references and content hashes after
        // backups are durable, still under the app/sync source-operation lock.
        let fresh = match self.scan_unused_assets_matching(Some(&candidates)) {
            Ok(scan) => scan
                .assets
                .into_iter()
                .map(|a| (a.filename.clone(), a))
                .collect::<HashMap<_, _>>(),
            Err(e) => {
                result
                    .errors
                    .push(format!("Recheck failed; originals left unchanged: {e}"));
                return Ok(result);
            }
        };
        for asset in staged {
            let move_file = || -> Result<()> {
                let now = fresh
                    .get(&asset.filename)
                    .ok_or_else(|| error("Now referenced or missing; original left unchanged"))?;
                if now.sha256 != asset.sha256 || now.size != asset.size {
                    return Err(error("Changed during cleanup; original left unchanged"));
                }
                let source = checked_path(&root, Path::new(&asset.filename))?;
                let destination = checked_path(&root, &batch.join(&asset.filename))?;
                if fingerprint(&source)? != (asset.size, asset.sha256.clone())
                    || fingerprint(&destination)? != (asset.size, asset.sha256.clone())
                {
                    return Err(error(
                        "File or recovery copy changed; original left unchanged",
                    ));
                }
                // Move the actual source, rather than unlinking after a copy:
                // even an external write racing this rename survives in trash.
                fs::rename(source, &destination)?;
                Ok(())
            };
            match move_file() {
                Ok(()) => {
                    result.moved.push(asset.filename.clone());
                    #[cfg(unix)]
                    for path in [
                        root.join(&asset.filename),
                        root.join(&batch).join(&asset.filename),
                    ] {
                        if let Some(parent) = path.parent() {
                            if let Err(e) = fs::File::open(parent).and_then(|file| file.sync_all())
                            {
                                result.errors.push(format!(
                                    "{} moved, but could not flush directory: {e}",
                                    asset.filename
                                ));
                            }
                        }
                    }
                    if let Err(e) =
                        crate::fsutil::record_source_mutation(&root, Path::new(&asset.filename))
                    {
                        result.errors.push(format!(
                            "{} moved, but could not record sync fence: {e}",
                            asset.filename
                        ));
                    }
                }
                Err(e) => result.errors.push(format!("{}: {e}", asset.filename)),
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
