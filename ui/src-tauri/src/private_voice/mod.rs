//! Device-local voices. The only network operation is a separately authorized
//! artifact download; speech is never sent to an HTTP endpoint.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::time::Instant;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

pub const MAX_TEXT_BYTES: usize = 1_500;
const MAX_MODEL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 768 * 1024 * 1024;
const MAX_AUDIO_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 512 * 1024;
#[cfg(target_os = "linux")]
const CACHE_CHUNKS: usize = 3;

#[derive(Default)]
pub struct VoiceState {
    generation: AtomicU64,
    operation: Mutex<()>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceArtifact {
    pub path: String,
    pub role: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    /// BCP-47 tag; no fixed list of languages is imposed.
    pub language: String,
    /// `piper-onnx-v1` or `sherpa-vits-v1`; never a command or URL.
    pub runtime: String,
    pub license: String,
    pub license_url: String,
    pub sample_rate: u32,
    pub artifacts: Vec<VoiceArtifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceSelection {
    pub voice_id: String,
    pub language: String,
}

#[derive(Debug, Serialize)]
pub struct VoiceStatus {
    pub available: bool,
    pub runtime: String,
    pub reason: Option<String>,
    pub selection: Option<VoiceSelection>,
    pub runtime_executable: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfiguration {
    pub executable: PathBuf,
    pub directory: PathBuf,
    pub executable_sha256: String,
}

#[cfg(target_os = "linux")]
struct PiperExecutable {
    executable: PathBuf,
    directory: Option<PathBuf>,
}

fn validate_runtime(executable: &Path) -> Result<RuntimeConfiguration, String> {
    if !executable.is_absolute()
        || executable.file_name().is_none_or(|s| s != "piper")
        || !fs::metadata(executable).is_ok_and(|m| m.is_file())
    {
        return Err(
            "Choose the bin/piper entry point of a dedicated local Python virtual environment"
                .into(),
        );
    }
    let executable = executable
        .canonicalize()
        .map_err(|_| "Cannot resolve the local Piper entry point")?;
    let bin = executable.parent().ok_or("Invalid Piper entry point")?;
    let directory = bin
        .parent()
        .ok_or("Invalid Piper virtual environment")?
        .to_path_buf();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if bin.file_name().is_none_or(|s| s != "bin")
        || directory.components().count() < 4
        || home.as_ref() == Some(&directory)
        || directory == Path::new("/usr/local")
        || !directory.join("pyvenv.cfg").is_file()
    {
        return Err("Piper must use a dedicated virtual environment, never a home directory or filesystem root".into());
    }
    let python = bin
        .join("python")
        .canonicalize()
        .map_err(|_| "The Piper environment lacks its Python interpreter")?;
    if !python.starts_with("/usr/") {
        return Err("This sandbox supports a virtual environment based on system Python under /usr; external Python installations are not mounted".into());
    }
    let launcher = read_bounded(&executable, 64 * 1024)?;
    let launcher_text = std::str::from_utf8(&launcher)
        .map_err(|_| "Expected the official Python Piper entry point")?;
    if !launcher_text.starts_with("#!")
        || !launcher_text.contains("from piper.__main__ import main")
    {
        return Err("Selected file is not the supported Piper speech CLI entry point".into());
    }
    Ok(RuntimeConfiguration {
        executable,
        directory,
        executable_sha256: format!("{:x}", Sha256::digest(&launcher)),
    })
}

pub fn configure_runtime(
    root: &Path,
    state: &VoiceState,
    executable: &Path,
) -> Result<RuntimeConfiguration, String> {
    if !cfg!(target_os = "linux") {
        return Err("An external Piper environment is only supported on Linux".into());
    }
    let config = validate_runtime(executable)?;
    state.cancel();
    let _lock = state
        .operation
        .lock()
        .map_err(|_| "Voice worker unavailable")?;
    prepare_root(root)?;
    atomic_json(&root.join("runtime.json"), &config)?;
    Ok(config)
}

#[derive(Debug, Serialize)]
pub struct SynthesizedAudio {
    pub request_id: String,
    pub file_name: String,
    pub sample_rate: u32,
    pub bytes: u64,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn valid_language(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .split('-')
            .all(|s| !s.is_empty() && s.len() <= 8 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        && value.as_bytes()[0].is_ascii_alphabetic()
}

fn relative_file(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 240
        || value.contains(['\\', ':', '\0'])
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || value
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err("Voice artifact must be a safe relative file path".into());
    }
    Ok(path.to_path_buf())
}

fn download_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid voice download URL")?;
    let host = url.host_str().ok_or("Missing voice download host")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || !host.contains('.')
        || host.parse::<std::net::IpAddr>().is_ok()
    {
        return Err(
            "Voice downloads require public HTTPS URLs without credentials, queries or fragments"
                .into(),
        );
    }
    Ok(url)
}

impl VoiceManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || !valid_id(&self.id) {
            return Err("Unsupported voice manifest version or invalid voice ID".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 256 || !valid_language(&self.language) {
            return Err("Voice name or language is invalid".into());
        }
        if !matches!(self.runtime.as_str(), "piper-onnx-v1" | "sherpa-vits-v1") {
            return Err("Unsupported offline voice runtime".into());
        }
        if !(8_000..=48_000).contains(&self.sample_rate) {
            return Err("Unsupported voice sample rate".into());
        }
        if self.license.trim().is_empty() || self.license.len() > 256 {
            return Err("Per-voice license metadata is required".into());
        }
        download_url(&self.license_url)?;
        if self.artifacts.is_empty() || self.artifacts.len() > 2048 {
            return Err("Voice artifact count is out of bounds".into());
        }
        let mut total = 0u64;
        let mut paths = std::collections::HashSet::new();
        for artifact in &self.artifacts {
            relative_file(&artifact.path)?;
            if !paths.insert(&artifact.path)
                || !matches!(
                    artifact.role.as_str(),
                    "model" | "config" | "license" | "tokens" | "espeak" | "lexicon"
                )
                || artifact.bytes == 0
                || artifact.bytes > MAX_MODEL_BYTES
                || artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("Invalid voice artifact metadata, size or SHA-256".into());
            }
            if artifact.role != "model" && artifact.bytes > 16 * 1024 * 1024 {
                return Err("Voice support file is too large".into());
            }
            total = total
                .checked_add(artifact.bytes)
                .ok_or("Voice size overflow")?;
            if let Some(url) = &artifact.url {
                download_url(url)?;
            }
        }
        if total > MAX_PACKAGE_BYTES {
            return Err("Voice package exceeds the 768 MiB limit".into());
        }
        for role in ["model", "license"] {
            if self.artifacts.iter().filter(|a| a.role == role).count() != 1 {
                return Err(format!("Voice requires exactly one {role} artifact"));
            }
        }
        let required = if self.runtime == "piper-onnx-v1" {
            "config"
        } else {
            "tokens"
        };
        if self.artifacts.iter().filter(|a| a.role == required).count() != 1 {
            return Err(format!("Voice requires exactly one {required} artifact"));
        }
        if !self.artifact("model")?.path.ends_with(".onnx") {
            return Err("Voice model must be an ONNX file".into());
        }
        if self.runtime == "piper-onnx-v1"
            && self.artifact("config")?.path != format!("{}.json", self.artifact("model")?.path)
        {
            return Err(
                "Piper configuration must be adjacent to its model as <model>.onnx.json".into(),
            );
        }
        Ok(())
    }

    pub fn artifact(&self, role: &str) -> Result<&VoiceArtifact, String> {
        self.artifacts
            .iter()
            .find(|a| a.role == role)
            .ok_or_else(|| format!("Missing {role} artifact"))
    }
}

pub fn prepare_root(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join("installed"))
        .map_err(|_| "Cannot create private voice storage")?;
    fs::create_dir_all(root.join("audio")).map_err(|_| "Cannot create private audio storage")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect private voice storage")?;
    }
    Ok(())
}

fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "Voice file is missing or inaccessible")?;
    if !metadata.is_file() || metadata.len() > max {
        return Err("Voice file is not a regular file or exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "Cannot open voice file")?
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read voice file")?;
    if bytes.len() as u64 > max {
        return Err("Voice file exceeds its size limit".into());
    }
    Ok(bytes)
}

pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Cannot encode private voice state")?;
    let staging = path.with_extension(format!("{}.partial", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create(&staging).map_err(|_| "Cannot create private voice state")?;
        file.write_all(&bytes)
            .map_err(|_| "Cannot write private voice state")?;
        file.sync_all()
            .map_err(|_| "Cannot sync private voice state")?;
        fs::rename(&staging, path).map_err(|_| "Cannot commit private voice state")?;
        if let Some(parent) = path.parent() {
            File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|_| "Cannot sync voice state directory")?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

pub fn installed(root: &Path) -> Result<Vec<VoiceManifest>, String> {
    prepare_root(root)?;
    let mut voices = Vec::new();
    for entry in fs::read_dir(root.join("installed")).map_err(|_| "Cannot list installed voices")? {
        let entry = entry.map_err(|_| "Cannot read installed voice")?;
        if !entry
            .file_type()
            .map_err(|_| "Cannot inspect installed voice")?
            .is_dir()
        {
            continue;
        }
        let manifest = load_manifest(&entry.path().join("manifest.json"))?;
        voices.push(manifest);
        if voices.len() > 128 {
            return Err("Too many installed voices".into());
        }
    }
    voices.sort_by(|a, b| (&a.language, &a.name).cmp(&(&b.language, &b.name)));
    Ok(voices)
}

pub fn load_manifest(path: &Path) -> Result<VoiceManifest, String> {
    let manifest: VoiceManifest = serde_json::from_slice(&read_bounded(path, MAX_MANIFEST_BYTES)?)
        .map_err(|_| "Invalid voice manifest JSON")?;
    manifest.validate()?;
    Ok(manifest)
}

fn verify_file(path: &Path, artifact: &VoiceArtifact) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "Voice artifact is missing")?;
    if !metadata.is_file() || metadata.len() != artifact.bytes {
        return Err("Voice artifact size mismatch or non-regular file".into());
    }
    let mut file = File::open(path).map_err(|_| "Cannot read voice artifact")?;
    let mut digest = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| "Cannot verify voice artifact")?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > artifact.bytes {
            return Err("Voice artifact grew during verification".into());
        }
        digest.update(&buf[..n]);
    }
    if bytes != artifact.bytes
        || format!("{:x}", digest.finalize()) != artifact.sha256.to_ascii_lowercase()
    {
        return Err("Voice artifact SHA-256 mismatch".into());
    }
    Ok(())
}

pub(crate) fn validate_package(dir: &Path, manifest: &VoiceManifest) -> Result<(), String> {
    for artifact in &manifest.artifacts {
        verify_file(&dir.join(&artifact.path), artifact)?;
    }
    let model_metadata = validate_onnx(&dir.join(&manifest.artifact("model")?.path))?;
    if manifest.runtime == "sherpa-vits-v1" {
        validate_sherpa_metadata(manifest, &model_metadata)?;
    }
    let license = read_bounded(
        &dir.join(&manifest.artifact("license")?.path),
        16 * 1024 * 1024,
    )?;
    if std::str::from_utf8(&license).map_or(true, |s| s.trim().is_empty()) {
        return Err("Voice license/model card must be nonempty UTF-8".into());
    }
    if manifest.runtime == "piper-onnx-v1" {
        let config: serde_json::Value = serde_json::from_slice(&read_bounded(
            &dir.join(&manifest.artifact("config")?.path),
            16 * 1024 * 1024,
        )?)
        .map_err(|_| "Invalid Piper configuration JSON")?;
        if config
            .pointer("/audio/sample_rate")
            .and_then(|v| v.as_u64())
            != Some(manifest.sample_rate as u64)
            || config.get("phoneme_type").and_then(|v| v.as_str()) != Some("espeak")
            || config
                .get("phoneme_id_map")
                .and_then(|v| v.as_object())
                .map_or(true, |m| m.is_empty())
            || config
                .pointer("/espeak/voice")
                .and_then(|v| v.as_str())
                .map_or(true, |s| s.is_empty() || s.len() > 64)
        {
            return Err("Piper config is incompatible: expected espeak phonemes, IDs and matching sample rate".into());
        }
    }
    Ok(())
}

fn varint(reader: &mut impl Read) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let mut byte = [0];
        reader
            .read_exact(&mut byte)
            .map_err(|_| "Invalid ONNX protobuf")?;
        if shift == 63 && byte[0] > 1 {
            return Err("ONNX protobuf integer overflow".into());
        }
        value |= u64::from(byte[0] & 127) << shift;
        if byte[0] & 128 == 0 {
            return Ok(value);
        }
    }
    Err("Invalid ONNX protobuf integer".into())
}

/// Rejects wrong formats and unsupported IR versions before invoking a runtime.
/// Operator/tensor compatibility is subsequently checked by the actual engine.
fn validate_sherpa_metadata(
    manifest: &VoiceManifest,
    metadata: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    if metadata
        .get("sample_rate")
        .and_then(|s| s.parse::<u32>().ok())
        != Some(manifest.sample_rate)
        || !metadata
            .get("n_speakers")
            .and_then(|s| s.parse::<u32>().ok())
            .is_some_and(|n| (1..=4096).contains(&n))
        || !metadata
            .get("comment")
            .is_some_and(|s| s.contains("piper") && !s.chars().any(char::is_control))
        || !metadata
            .get("language")
            .is_some_and(|s| !s.is_empty() && s.len() <= 63 && !s.chars().any(char::is_control))
        || !metadata.get("voice").is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    {
        return Err("Sherpa requires a converted Piper/VITS model with sample_rate, n_speakers, comment=piper, language and voice metadata; raw Piper ONNX is incompatible".into());
    }
    // Sherpa 1.13.8 parses these through atoi and exits on negative values.
    for key in [
        "add_blank",
        "speaker_id",
        "version",
        "num_emotions",
        "jieba",
        "blank_id",
        "bos_id",
        "eos_id",
        "use_eos_bos",
        "pad_id",
        "has_g2pw",
    ] {
        if let Some(value) = metadata.get(key) {
            if value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
                || value.parse::<i32>().is_err()
            {
                return Err(format!("Invalid Sherpa integer metadata: {key}"));
            }
        }
    }
    if metadata.get("comment").is_some_and(|s| s.contains("melo"))
        && !metadata
            .get("version")
            .and_then(|s| s.parse::<i32>().ok())
            .is_some_and(|version| version >= 2)
    {
        return Err("Sherpa Melo model metadata requires version 2 or later".into());
    }
    for path in [
        "espeak-ng-data/phontab",
        "espeak-ng-data/phonindex",
        "espeak-ng-data/phondata",
    ] {
        if !manifest
            .artifacts
            .iter()
            .any(|a| a.role == "espeak" && a.path == path)
        {
            return Err("Sherpa Piper voices require hash-verified espeak-ng-data files".into());
        }
    }
    if manifest
        .artifacts
        .iter()
        .any(|a| a.role == "espeak" && !a.path.starts_with("espeak-ng-data/"))
    {
        return Err("Sherpa phonemizer data must remain inside espeak-ng-data".into());
    }
    Ok(())
}

fn onnx_metadata_pair(bytes: &[u8]) -> Result<(String, String), String> {
    let mut input = std::io::Cursor::new(bytes);
    let mut key = None;
    let mut value = None;
    while input.position() < bytes.len() as u64 {
        let field = varint(&mut input)?;
        if !matches!(field, 10 | 18) {
            return Err("Invalid ONNX metadata entry".into());
        }
        let length = varint(&mut input)?;
        if length > 64 * 1024 || input.position() + length > bytes.len() as u64 {
            return Err("ONNX metadata entry exceeds bounds".into());
        }
        let mut text = vec![0; length as usize];
        input
            .read_exact(&mut text)
            .map_err(|_| "Truncated ONNX metadata")?;
        let text = String::from_utf8(text).map_err(|_| "Invalid ONNX metadata text")?;
        if field == 10 {
            key = Some(text);
        } else {
            value = Some(text);
        }
    }
    Ok((
        key.ok_or("ONNX metadata lacks key")?,
        value.ok_or("ONNX metadata lacks value")?,
    ))
}

fn validate_onnx(path: &Path) -> Result<std::collections::HashMap<String, String>, String> {
    let mut file = File::open(path).map_err(|_| "Cannot open ONNX model")?;
    let len = file
        .metadata()
        .map_err(|_| "Cannot inspect ONNX model")?
        .len();
    let mut ir_version = None;
    let mut graph = false;
    let mut fields = 0;
    let mut metadata = std::collections::HashMap::new();
    while file
        .stream_position()
        .map_err(|_| "Cannot inspect ONNX structure")?
        < len
    {
        fields += 1;
        if fields > 100_000 {
            return Err("ONNX model has too many top-level fields".into());
        }
        let key = varint(&mut file)?;
        if key >> 3 == 0 {
            return Err("Invalid ONNX field number".into());
        }
        let skip = match key & 7 {
            0 => {
                let value = varint(&mut file)?;
                if key >> 3 == 1 {
                    ir_version = Some(value);
                }
                0
            }
            1 => 8,
            2 => {
                let size = varint(&mut file)?;
                if key >> 3 == 7 {
                    graph = size > 0;
                }
                if key >> 3 == 14 {
                    if size > 64 * 1024 || metadata.len() >= 128 {
                        return Err("ONNX metadata exceeds safe bounds".into());
                    }
                    let mut bytes = vec![0; size as usize];
                    file.read_exact(&mut bytes)
                        .map_err(|_| "Truncated ONNX metadata")?;
                    let (key, value) = onnx_metadata_pair(&bytes)?;
                    if metadata.insert(key, value).is_some() {
                        return Err("Duplicate ONNX model metadata".into());
                    }
                    0
                } else {
                    size
                }
            }
            5 => 4,
            _ => return Err("Unsupported ONNX protobuf wire format".into()),
        };
        let position = file
            .stream_position()
            .map_err(|_| "Cannot inspect ONNX structure")?;
        if skip > len.saturating_sub(position) {
            return Err("Truncated ONNX model".into());
        }
        file.seek(SeekFrom::Current(skip as i64))
            .map_err(|_| "Cannot inspect ONNX structure")?;
    }
    if !matches!(ir_version, Some(3..=10)) || !graph {
        return Err(
            "Voice needs an ONNX IR 3–10 model with a graph; this runtime contract cannot load it"
                .into(),
        );
    }
    Ok(metadata)
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stage(root: &Path, manifest: &VoiceManifest) -> Result<Staging, String> {
    prepare_root(root)?;
    manifest.validate()?;
    if root.join("installed").join(&manifest.id).exists() {
        return Err("Voice ID already installed; use a new versioned ID".into());
    }
    let path = root.join(format!("install-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&path).map_err(|_| "Cannot stage voice installation")?;
    Ok(Staging(path))
}

fn commit_package(root: &Path, staging: &Staging, manifest: &VoiceManifest) -> Result<(), String> {
    validate_package(&staging.0, manifest)?;
    atomic_json(&staging.0.join("manifest.json"), manifest)?;
    fs::rename(&staging.0, root.join("installed").join(&manifest.id))
        .map_err(|_| "Cannot commit voice installation")?;
    File::open(root.join("installed"))
        .and_then(|f| f.sync_all())
        .map_err(|_| "Cannot sync installed voice directory")?;
    Ok(())
}

pub fn import(root: &Path, manifest_path: &Path) -> Result<VoiceManifest, String> {
    let manifest = load_manifest(manifest_path)?;
    let source = manifest_path
        .parent()
        .ok_or("Missing voice package directory")?
        .canonicalize()
        .map_err(|_| "Cannot resolve voice package directory")?;
    let staging = stage(root, &manifest)?;
    for artifact in &manifest.artifacts {
        let from = source.join(&artifact.path);
        let canonical = from
            .canonicalize()
            .map_err(|_| "Voice source file is missing")?;
        if !canonical.starts_with(&source) {
            return Err("Voice source escapes its package directory".into());
        }
        verify_file(&from, artifact)?;
        let target = staging.0.join(&artifact.path);
        fs::create_dir_all(target.parent().ok_or("Invalid artifact path")?)
            .map_err(|_| "Cannot stage artifact")?;
        let mut input = File::open(from)
            .map_err(|_| "Cannot open voice artifact")?
            .take(artifact.bytes + 1);
        let mut output = File::create(&target).map_err(|_| "Cannot stage voice artifact")?;
        let count =
            std::io::copy(&mut input, &mut output).map_err(|_| "Cannot copy voice artifact")?;
        if count != artifact.bytes {
            return Err("Voice source changed during import".into());
        }
        output
            .sync_all()
            .map_err(|_| "Cannot sync voice artifact")?;
    }
    commit_package(root, &staging, &manifest)?;
    Ok(manifest)
}

pub async fn download(
    root: &Path,
    manifest: VoiceManifest,
    authorized: bool,
) -> Result<VoiceManifest, String> {
    if !authorized {
        return Err("Explicit authorization is required to download a voice".into());
    }
    let staging = stage(root, &manifest)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|_| "Cannot initialize voice downloader")?;
    for artifact in &manifest.artifacts {
        let url = download_url(
            artifact
                .url
                .as_deref()
                .ok_or("Every downloaded artifact needs a URL")?,
        )?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|_| "Voice download failed (no book content was sent)")?;
        if !response.status().is_success() {
            return Err("Voice download failed; redirects are deliberately not followed".into());
        }
        if response
            .content_length()
            .is_some_and(|n| n != artifact.bytes)
        {
            return Err("Voice download Content-Length mismatch".into());
        }
        let target = staging.0.join(&artifact.path);
        fs::create_dir_all(target.parent().ok_or("Invalid voice artifact path")?)
            .map_err(|_| "Cannot stage voice download")?;
        let mut file = File::create(&target).map_err(|_| "Cannot write voice download")?;
        let mut bytes = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Voice download interrupted")?
        {
            bytes += chunk.len() as u64;
            if bytes > artifact.bytes {
                return Err("Voice download exceeds declared size".into());
            }
            file.write_all(&chunk)
                .map_err(|_| "Cannot store voice download")?;
        }
        file.sync_all().map_err(|_| "Cannot sync voice download")?;
    }
    commit_package(root, &staging, &manifest)?;
    Ok(manifest)
}

pub fn selection(root: &Path) -> Result<Option<VoiceSelection>, String> {
    let path = root.join("selection.json");
    if !path.exists() {
        return Ok(None);
    }
    let value: VoiceSelection = serde_json::from_slice(&read_bounded(&path, 4096)?)
        .map_err(|_| "Invalid saved voice selection")?;
    if !valid_id(&value.voice_id) || !valid_language(&value.language) {
        return Err("Invalid saved voice selection".into());
    }
    Ok(Some(value))
}

pub fn select(
    root: &Path,
    state: &VoiceState,
    voice_id: &str,
    language: &str,
) -> Result<VoiceSelection, String> {
    if !valid_id(voice_id) || !valid_language(language) {
        return Err("Invalid voice or language".into());
    }
    let manifest = load_manifest(&root.join("installed").join(voice_id).join("manifest.json"))?;
    if !manifest.language.eq_ignore_ascii_case(language) {
        return Err("Selected language does not match the voice; install a matching voice".into());
    }
    state.cancel();
    let _lock = state
        .operation
        .lock()
        .map_err(|_| "Voice worker unavailable")?;
    let value = VoiceSelection {
        voice_id: voice_id.into(),
        language: language.into(),
    };
    atomic_json(&root.join("selection.json"), &value)?;
    clear_audio(root)?;
    Ok(value)
}

fn clear_audio(root: &Path) -> Result<(), String> {
    let dir = root.join("audio");
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|_| "Cannot clear private audio cache")? {
        let entry = entry.map_err(|_| "Cannot inspect private audio cache")?;
        if entry
            .file_type()
            .map_err(|_| "Cannot inspect audio cache entry")?
            .is_file()
        {
            fs::remove_file(entry.path()).map_err(|_| "Cannot remove private audio cache entry")?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn piper_binary(root: &Path) -> Result<PiperExecutable, String> {
    if root.join("runtime.json").exists() {
        let saved: RuntimeConfiguration =
            serde_json::from_slice(&read_bounded(&root.join("runtime.json"), 16 * 1024)?)
                .map_err(|_| "Invalid saved Piper environment configuration")?;
        let current = validate_runtime(&saved.executable)?;
        if saved.directory != current.directory
            || saved.executable_sha256 != current.executable_sha256
        {
            return Err(
                "The selected Piper entry point changed; explicitly select the environment again"
                    .into(),
            );
        }
        return Ok(PiperExecutable {
            executable: current.executable,
            directory: Some(current.directory),
        });
    }
    ["/usr/bin/piper", "/usr/local/bin/piper"].into_iter().find(|p| {
        read_bounded(Path::new(p), 64 * 1024).ok().is_some_and(|bytes| {
            std::str::from_utf8(&bytes).is_ok_and(|text| text.starts_with("#!")
                && text.contains("from piper.__main__ import main"))
        })
    })
        .map(|p| PiperExecutable { executable: p.into(), directory: None })
        .ok_or_else(|| "Piper speech is unavailable. Install piper-tts in a dedicated virtual environment based on system Python, then choose its bin/piper (or pipx launcher) in voice settings. An unrelated mouse-configuration app named Piper is not a speech runtime; no system/cloud fallback is used".into())
}

pub fn status(root: &Path) -> Result<VoiceStatus, String> {
    let selection = selection(root)?;
    #[cfg(target_os = "linux")]
    let runtime = piper_binary(root);
    #[cfg(target_os = "linux")]
    let runtime_executable = runtime
        .as_ref()
        .ok()
        .map(|r| r.executable.to_string_lossy().into_owned());
    #[cfg(target_os = "linux")]
    let reason = runtime.err().or_else(|| {
        (!Path::new("/usr/bin/bwrap").is_file())
            .then(|| "Bubblewrap is required to isolate the speech worker from the network".into())
    });
    #[cfg(not(target_os = "linux"))]
    let reason = Some("This platform's embedded offline voice adapter is not available through the desktop synthesis command".into());
    #[cfg(not(target_os = "linux"))]
    let runtime_executable = None;
    Ok(VoiceStatus {
        available: reason.is_none(),
        runtime: if cfg!(target_os = "android") {
            "sherpa-vits-v1"
        } else {
            "piper-onnx-v1"
        }
        .into(),
        reason,
        selection,
        runtime_executable,
    })
}

impl VoiceState {
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn check_generation(&self, expected: u64) -> Result<(), String> {
        if self.generation.load(Ordering::SeqCst) != expected {
            Err("Speech synthesis cancelled".into())
        } else {
            Ok(())
        }
    }
}

pub fn audio_path(root: &Path, file_name: &str) -> Result<PathBuf, String> {
    if !file_name.ends_with(".wav") || !valid_id(file_name.trim_end_matches(".wav")) {
        return Err("Invalid generated audio ID".into());
    }
    let path = root.join("audio").join(file_name);
    let meta = fs::symlink_metadata(&path)
        .map_err(|_| "Generated audio expired; synthesize the segment again")?;
    if !meta.is_file() || meta.len() > MAX_AUDIO_BYTES {
        return Err("Invalid generated audio file".into());
    }
    Ok(path)
}

pub fn read_audio(path: &Path) -> Result<Vec<u8>, String> {
    read_bounded(path, MAX_AUDIO_BYTES)
}

pub fn synthesize(
    root: &Path,
    state: &VoiceState,
    text: &str,
    request_id: &str,
) -> Result<SynthesizedAudio, String> {
    if !valid_id(request_id)
        || text.trim().is_empty()
        || text.len() > MAX_TEXT_BYTES
        || text.contains('\0')
    {
        return Err("Speech request needs an ID and 1–1500 UTF-8 bytes of plain text".into());
    }
    let generation = state.generation.load(Ordering::SeqCst);
    let _lock = state
        .operation
        .lock()
        .map_err(|_| "Voice worker unavailable")?;
    state.check_generation(generation)?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Err("Desktop Piper synthesis is unavailable on this platform; no system/cloud fallback is used".into())
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};
        let runtime = piper_binary(root)?;
        let selected = selection(root)?.ok_or("Select an installed offline voice first")?;
        let dir = root.join("installed").join(selected.voice_id);
        let manifest = load_manifest(&dir.join("manifest.json"))?;
        if manifest.runtime != "piper-onnx-v1" {
            return Err("Selected model is not compatible with Piper".into());
        }
        validate_package(&dir, &manifest)?;
        let cache = root
            .join("audio")
            .canonicalize()
            .map_err(|_| "Cannot resolve private audio directory")?;
        let mut cached: Vec<_> = fs::read_dir(&cache)
            .map_err(|_| "Cannot list private audio")?
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
            .collect();
        cached.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
        while cached.len() >= CACHE_CHUNKS {
            fs::remove_file(cached.remove(0).path())
                .map_err(|_| "Cannot bound private audio cache")?;
        }
        let file_name = format!("{}.wav", uuid::Uuid::new_v4());
        let work = Staging(cache.join(format!("speech-work-{}", uuid::Uuid::new_v4())));
        fs::create_dir(&work.0).map_err(|_| "Cannot create bounded speech working directory")?;
        let output = work.0.join("output.wav");
        // Fixed executable and argument vector; book text is only written to stdin.
        // The worker cannot use networking, even if an installed runtime tries.
        let runtime_mount: Vec<std::ffi::OsString> = runtime
            .directory
            .as_ref()
            .map(|directory| {
                vec![
                    "--ro-bind".into(),
                    directory.as_os_str().into(),
                    directory.as_os_str().into(),
                ]
            })
            .unwrap_or_default();
        let mut child = Command::new("/usr/bin/bwrap")
            .args([
                "--unshare-net",
                "--unshare-pid",
                "--unshare-ipc",
                "--die-with-parent",
                "--new-session",
                "--ro-bind",
                "/usr",
                "/usr",
                "--ro-bind",
                "/lib",
                "/lib",
                "--ro-bind-try",
                "/lib64",
                "/lib64",
                "--ro-bind-try",
                "/etc/ld.so.cache",
                "/etc/ld.so.cache",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
            ])
            .args(runtime_mount)
            .arg("--ro-bind")
            .arg(&dir)
            .arg(&dir)
            .arg("--bind")
            .arg(&work.0)
            .arg(&work.0)
            .arg("--chdir")
            .arg(&work.0)
            .arg("--")
            .arg(&runtime.executable)
            .arg("--model")
            .arg(dir.join(&manifest.artifact("model")?.path))
            .arg("--config")
            .arg(dir.join(&manifest.artifact("config")?.path))
            .arg("--output_file")
            .arg(&output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin")
            .env("LANG", "C.UTF-8")
            .env("HOME", &work.0)
            .env("XDG_CACHE_HOME", &work.0)
            .env("TMPDIR", &work.0)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .spawn()
            .map_err(|_| "Offline sandbox/Piper could not start; no fallback is used")?;
        let result = (|| {
            child
                .stdin
                .take()
                .ok_or("Speech worker input is unavailable")?
                .write_all(text.as_bytes())
                .map_err(|_| "Speech worker rejected its input")?;
            let start = Instant::now();
            loop {
                state.check_generation(generation)?;
                if start.elapsed() > Duration::from_secs(120) {
                    return Err("Offline speech timed out".into());
                }
                if fs::metadata(&output).is_ok_and(|m| m.len() > MAX_AUDIO_BYTES) {
                    return Err("Generated speech exceeds the audio size limit".into());
                }
                if let Some(exit) = child
                    .try_wait()
                    .map_err(|_| "Cannot wait for speech worker")?
                {
                    if !exit.success() {
                        return Err("Offline voice failed to load or synthesize in the network-isolated worker".into());
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let bytes = read_bounded(&output, MAX_AUDIO_BYTES)?;
            validate_wav(&bytes, manifest.sample_rate)?;
            state.check_generation(generation)?;
            fs::rename(&output, cache.join(&file_name))
                .map_err(|_| "Cannot publish generated private audio")?;
            Ok(SynthesizedAudio {
                request_id: request_id.into(),
                file_name,
                sample_rate: manifest.sample_rate,
                bytes: bytes.len() as u64,
            })
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(output);
        }
        result
    }
}

#[cfg(any(target_os = "linux", test))]
fn validate_wav(bytes: &[u8], sample_rate: u32) -> Result<(), String> {
    if bytes.len() < 44
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || &bytes[12..16] != b"fmt "
        || u16::from_le_bytes([bytes[20], bytes[21]]) != 1
        || u16::from_le_bytes([bytes[22], bytes[23]]) != 1
        || u32::from_le_bytes(bytes[24..28].try_into().unwrap()) != sample_rate
        || u16::from_le_bytes([bytes[34], bytes[35]]) != 16
    {
        return Err("Speech worker produced incompatible PCM WAV audio".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::current_dir()
                .unwrap()
                .join("target")
                .join(format!("voice-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn manifest() -> VoiceManifest {
        VoiceManifest {
            schema_version: 1,
            id: "voice-v1".into(),
            name: "Voice".into(),
            language: "zh-Hant-TW".into(),
            runtime: "piper-onnx-v1".into(),
            license: "CC-BY-4.0".into(),
            license_url: "https://example.org/model-card".into(),
            sample_rate: 22050,
            artifacts: ["model", "config", "license"]
                .into_iter()
                .map(|role| VoiceArtifact {
                    path: match role {
                        "model" => "voice.onnx".into(),
                        "config" => "voice.onnx.json".into(),
                        _ => format!("{role}.json"),
                    },
                    role: role.into(),
                    bytes: 32,
                    sha256: "a".repeat(64),
                    url: None,
                })
                .collect(),
        }
    }

    #[test]
    fn arbitrary_languages_but_no_paths_or_commands() {
        assert!(manifest().validate().is_ok());
        for lang in ["fr-CA", "ar-JO", "sw-CD", "sr-Latn", "x-custom"] {
            assert!(valid_language(lang));
        }
        for lang in ["", "../en", "en;sh", "en_US", "en--GB"] {
            assert!(!valid_language(lang));
        }
        for path in [
            "../model.onnx",
            "/model",
            "a/../b",
            "a//b",
            "https://x.y/m",
            "a\\b",
        ] {
            assert!(relative_file(path).is_err(), "{path}");
        }
    }

    #[test]
    fn model_metadata_limits_and_license_are_enforced() {
        let mut value = manifest();
        value.artifacts[0].sha256 = "bad".into();
        assert!(value.validate().is_err());
        value = manifest();
        value.license.clear();
        assert!(value.validate().is_err());
        value = manifest();
        value.artifacts[0].bytes = MAX_MODEL_BYTES + 1;
        assert!(value.validate().is_err());
        value = manifest();
        value.runtime = "curl https://bad.example".into();
        assert!(value.validate().is_err());
        value = manifest();
        value.artifacts.push(value.artifacts[0].clone());
        assert!(value.validate().is_err());
    }

    #[test]
    fn download_urls_cannot_include_credentials_book_queries_or_local_endpoints() {
        assert!(download_url("https://example.org/voice.onnx").is_ok());
        for url in [
            "http://example.org/v",
            "file:///book.epub",
            "https://example.org/v?text=private",
            "https://user:pass@example.org/v",
            "https://127.0.0.1/v",
            "https://localhost/v",
            "https://nas.local/v",
        ] {
            assert!(download_url(url).is_err());
        }
    }

    #[test]
    fn cancellation_and_text_bounds_precede_runtime_invocation() {
        let state = VoiceState::default();
        assert!(state.check_generation(0).is_ok());
        state.cancel();
        assert!(state.check_generation(0).is_err());
        assert!(synthesize(
            Path::new("."),
            &state,
            &"x".repeat(MAX_TEXT_BYTES + 1),
            "request"
        )
        .is_err());
        assert!(synthesize(Path::new("."), &state, "Hello", "../request").is_err());
    }

    #[test]
    fn offline_import_verifies_integrity_and_never_replaces_existing_voice() {
        let test = TestDir::new();
        let source = test.0.join("source");
        fs::create_dir(&source).unwrap();
        let root = test.0.join("voices");
        let mut metadata = manifest();
        let config = br#"{"audio":{"sample_rate":22050},"phoneme_type":"espeak","phoneme_id_map":{"a":[1]},"espeak":{"voice":"zh"}}"#;
        let artifacts: [&[u8]; 3] = [
            &[8, 8, 58, 2, 10, 0],
            config,
            b"Test fixture license, not a distributable voice",
        ];
        for (artifact, bytes) in metadata.artifacts.iter_mut().zip(artifacts) {
            artifact.bytes = bytes.len() as u64;
            artifact.sha256 = format!("{:x}", Sha256::digest(bytes));
            fs::write(source.join(&artifact.path), bytes).unwrap();
        }
        atomic_json(&source.join("manifest.json"), &metadata).unwrap();
        assert!(import(&root, &source.join("manifest.json")).is_ok());
        assert_eq!(installed(&root).unwrap().len(), 1);
        assert!(import(&root, &source.join("manifest.json")).is_err());
        let state = VoiceState::default();
        assert!(select(&root, &state, "voice-v1", "en-US").is_err());
        assert!(select(&root, &state, "voice-v1", "zh-Hant-TW").is_ok());
        assert_eq!(selection(&root).unwrap().unwrap().voice_id, "voice-v1");
        metadata.id = "voice-corrupt".into();
        metadata.artifacts[0].sha256 = "0".repeat(64);
        atomic_json(&source.join("manifest.json"), &metadata).unwrap();
        assert!(import(&root, &source.join("manifest.json"))
            .unwrap_err()
            .contains("SHA-256"));
        assert!(!root.join("installed/voice-corrupt").exists());
        assert!(!fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("install-")));
    }

    #[test]
    fn onnx_structure_rejects_arbitrary_files_and_unsupported_ir() {
        let test = TestDir::new();
        let file = test.0.join("model.onnx");
        for bytes in [
            b"<html>download error</html>".as_slice(),
            &[8, 99, 58, 2, 10, 0],
            &[8, 8, 58, 100, 10, 0],
        ] {
            fs::write(&file, bytes).unwrap();
            assert!(validate_onnx(&file).is_err());
        }
        fs::write(&file, [8, 8, 58, 2, 10, 0]).unwrap();
        assert!(validate_onnx(&file).is_ok());
    }

    #[test]
    fn sherpa_rejects_raw_piper_and_missing_phonemizer_before_native_load() {
        let mut voice = manifest();
        voice.runtime = "sherpa-vits-v1".into();
        let metadata = std::collections::HashMap::from([
            ("sample_rate".into(), "22050".into()),
            ("n_speakers".into(), "1".into()),
            ("comment".into(), "piper".into()),
            ("language".into(), "zh".into()),
            ("voice".into(), "cmn".into()),
        ]);
        assert!(validate_sherpa_metadata(&voice, &metadata).is_err());
        for path in ["phontab", "phonindex", "phondata"] {
            voice.artifacts.push(VoiceArtifact {
                path: format!("espeak-ng-data/{path}"),
                role: "espeak".into(),
                bytes: 1,
                sha256: "0".repeat(64),
                url: None,
            });
        }
        assert!(validate_sherpa_metadata(&voice, &metadata).is_ok());
        for key in [
            "add_blank",
            "speaker_id",
            "version",
            "num_emotions",
            "jieba",
            "blank_id",
            "bos_id",
            "eos_id",
            "use_eos_bos",
            "pad_id",
            "has_g2pw",
        ] {
            for value in ["-1", "2147483648", "12x", "", "1\0"] {
                let mut invalid = metadata.clone();
                invalid.insert(key.into(), value.into());
                assert!(validate_sherpa_metadata(&voice, &invalid).is_err());
            }
        }
        assert!(validate_sherpa_metadata(&voice, &std::collections::HashMap::new()).is_err());
        assert_eq!(
            onnx_metadata_pair(&[10, 1, b'x', 18, 1, b'y']).unwrap(),
            ("x".into(), "y".into())
        );
        assert!(onnx_metadata_pair(&[10, 100, b'x']).is_err());
    }

    #[test]
    fn shared_package_validation_is_read_only_and_rejects_incompatible_models() {
        let test = TestDir::new();
        let root = test.0.join("voices");
        let directory = root.join("staging-test");
        fs::create_dir_all(directory.join("espeak-ng-data")).unwrap();
        let mut model = vec![8, 8, 58, 2, 10, 0];
        for (key, value) in [
            ("sample_rate", "22050"),
            ("n_speakers", "1"),
            ("comment", "piper"),
            ("language", "zh"),
            ("voice", "cmn"),
        ] {
            let mut entry = vec![10, key.len() as u8];
            entry.extend_from_slice(key.as_bytes());
            entry.extend_from_slice(&[18, value.len() as u8]);
            entry.extend_from_slice(value.as_bytes());
            assert!(entry.len() < 128);
            model.extend_from_slice(&[114, entry.len() as u8]);
            model.extend_from_slice(&entry);
        }
        let mut voice = manifest();
        voice.runtime = "sherpa-vits-v1".into();
        voice.artifacts.clear();
        for (path, role, bytes) in [
            ("voice.onnx", "model", model.as_slice()),
            ("tokens.txt", "tokens", b"a 1\n".as_slice()),
            (
                "LICENSE",
                "license",
                b"Synthetic fixture license".as_slice(),
            ),
            ("espeak-ng-data/phontab", "espeak", b"x".as_slice()),
            ("espeak-ng-data/phonindex", "espeak", b"x".as_slice()),
            ("espeak-ng-data/phondata", "espeak", b"x".as_slice()),
        ] {
            fs::write(directory.join(path), bytes).unwrap();
            voice.artifacts.push(VoiceArtifact {
                path: path.into(),
                role: role.into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                url: None,
            });
        }
        voice.validate().unwrap();
        validate_package(&directory, &voice).unwrap();
        assert_eq!(fs::read(directory.join("voice.onnx")).unwrap(), model);
        assert!(!root.join("narration.json").exists());
        let mut mismatch = voice.clone();
        mismatch.sample_rate = 24000;
        assert!(validate_package(&directory, &mismatch)
            .unwrap_err()
            .contains("raw Piper ONNX"));
        let mut corrupted = voice.clone();
        corrupted.artifacts[0].sha256 = "0".repeat(64);
        assert!(validate_package(&directory, &corrupted)
            .unwrap_err()
            .contains("SHA-256"));
        let raw_piper = [8, 8, 58, 2, 10, 0];
        fs::write(directory.join("voice.onnx"), raw_piper).unwrap();
        voice.artifacts[0].bytes = raw_piper.len() as u64;
        voice.artifacts[0].sha256 = format!("{:x}", Sha256::digest(raw_piper));
        assert!(validate_package(&directory, &voice)
            .unwrap_err()
            .contains("raw Piper ONNX"));
    }

    #[tokio::test]
    async fn download_requires_consent_before_files_or_network() {
        assert!(
            download(Path::new("/must-not-be-created"), manifest(), false)
                .await
                .unwrap_err()
                .contains("authorization")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires explicitly provisioned Piper and licensed synthetic-test model"]
    fn real_piper_synthesizes_and_cancels_inside_offline_sandbox() {
        let runtime = PathBuf::from(
            std::env::var("GRAFIUM_VOICE_TEST_RUNTIME").expect("Explicit runtime fixture required"),
        );
        let model = PathBuf::from(
            std::env::var("GRAFIUM_VOICE_TEST_MODEL")
                .expect("Explicit licensed model fixture required"),
        );
        let test = TestDir::new();
        let source = test.0.join("source");
        fs::create_dir(&source).unwrap();
        let mut metadata = VoiceManifest {
            schema_version: 1, id: "synthetic-ljspeech".into(), name: "Synthetic validation only".into(),
            language: "en-US".into(), runtime: "piper-onnx-v1".into(),
            license: "MIT repository; public-domain LJSpeech dataset (see model card)".into(),
            license_url: "https://huggingface.co/rhasspy/piper-voices/blob/main/en/en_US/ljspeech/medium/MODEL_CARD".into(),
            sample_rate: 22050, artifacts: Vec::new(),
        };
        for (name, role) in [
            ("voice.onnx", "model"),
            ("voice.onnx.json", "config"),
            ("MODEL_CARD", "license"),
        ] {
            let bytes = fs::read(model.join(name)).unwrap();
            fs::write(source.join(name), &bytes).unwrap();
            metadata.artifacts.push(VoiceArtifact {
                path: name.into(),
                role: role.into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                url: None,
            });
        }
        atomic_json(&source.join("manifest.json"), &metadata).unwrap();
        let root = test.0.join("voices");
        let state = std::sync::Arc::new(VoiceState::default());
        configure_runtime(&root, &state, &runtime).unwrap();
        import(&root, &source.join("manifest.json")).unwrap();
        select(&root, &state, &metadata.id, "en-US").unwrap();
        let audio = synthesize(
            &root,
            &state,
            "This is synthetic offline speech. No private book was opened.",
            "real-test",
        )
        .unwrap();
        let bytes = read_audio(&audio_path(&root, &audio.file_name).unwrap()).unwrap();
        assert!(
            bytes.len() > 22050,
            "Expected meaningful synthesized audio, not an empty WAV"
        );
        assert!(bytes[44..]
            .chunks_exact(2)
            .any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 100));
        println!(
            "Verified real network-isolated synthetic speech: {} bytes at {} Hz",
            bytes.len(),
            audio.sample_rate
        );
        let worker_state = state.clone();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            synthesize(
                &worker_root,
                &worker_state,
                &"Synthetic cancellation validation. ".repeat(35),
                "cancel-test",
            )
        });
        let waiting = Instant::now();
        while !fs::read_dir(root.join("audio")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("speech-work-")
        }) {
            assert!(
                waiting.elapsed() < Duration::from_secs(10),
                "Speech worker did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        let cancelled_at = Instant::now();
        state.cancel();
        assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
        assert!(cancelled_at.elapsed() < Duration::from_secs(5));
        assert_eq!(fs::read_dir(root.join("audio")).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn selected_runtime_requires_dedicated_piper_environment() {
        use std::os::unix::fs::symlink;
        let test = TestDir::new();
        let root = test.0.join("environment");
        fs::create_dir_all(root.join("bin")).unwrap();
        let executable = root.join("bin/piper");
        fs::write(
            &executable,
            "#!/usr/bin/python3\nfrom piper.__main__ import main\nmain()\n",
        )
        .unwrap();
        assert!(validate_runtime(&executable).is_err());
        fs::write(
            root.join("pyvenv.cfg"),
            "include-system-site-packages = false\n",
        )
        .unwrap();
        symlink("/usr/bin/python3", root.join("bin/python")).unwrap();
        assert!(validate_runtime(&executable).is_ok());
        let pipx_launcher = test.0.join("piper");
        symlink(&executable, &pipx_launcher).unwrap();
        assert_eq!(validate_runtime(&pipx_launcher).unwrap().directory, root);
        fs::write(&executable, "#!/bin/sh\ncurl https://example.org\n").unwrap();
        assert!(validate_runtime(&executable).is_err());
        assert!(validate_runtime(Path::new("piper; curl example.org")).is_err());
    }

    #[test]
    fn wav_header_requires_local_pcm_shape() {
        assert!(validate_wav(b"not wave", 22050).is_err());
        let mut bytes = vec![0; 44];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes[12..16].copy_from_slice(b"fmt ");
        bytes[20] = 1;
        bytes[22] = 1;
        bytes[24..28].copy_from_slice(&22050u32.to_le_bytes());
        bytes[34] = 16;
        assert!(validate_wav(&bytes, 22050).is_ok());
        assert!(validate_wav(&bytes, 24000).is_err());
    }
}
