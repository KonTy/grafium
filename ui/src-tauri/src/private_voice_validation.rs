use crate::private_voice::VoiceManifest;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageRequest {
    directory: PathBuf,
    manifest: VoiceManifest,
}

fn validate(root: &Path, command: &str, args: &str) -> Result<(), String> {
    if command != "validatePackage" {
        return Err("Only voice package validation is supported".into());
    }
    if args.len() > 512 * 1024
        || !root.is_absolute()
        || !root.ends_with("no_backup/private-reader/voices")
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("Voice validation requires application-private no-backup storage".into());
    }
    let root = root
        .canonicalize()
        .map_err(|_| "Private voice storage is unavailable")?;
    let request: PackageRequest =
        serde_json::from_str(args).map_err(|_| "Invalid voice validation request")?;
    if !request.directory.is_absolute() || request.manifest.runtime != "sherpa-vits-v1" {
        return Err("Android requires a private, absolute sherpa-vits-v1 package directory".into());
    }
    let directory = request
        .directory
        .canonicalize()
        .map_err(|_| "Voice package directory is unavailable")?;
    if directory == root || !directory.starts_with(&root) {
        return Err("Voice package must remain inside private voice storage".into());
    }
    request.manifest.validate()?;
    for artifact in &request.manifest.artifacts {
        let path = directory
            .join(&artifact.path)
            .canonicalize()
            .map_err(|_| "Voice artifact is missing")?;
        if !path.starts_with(&directory) {
            return Err("Voice artifact escapes its package directory".into());
        }
    }
    crate::private_voice::validate_package(&directory, &request.manifest)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_grafium_app_ReaderOfflineSpeech_nativeVoice<'local>(
    mut env: jni::JNIEnv<'local>,
    _receiver: jni::objects::JObject<'local>,
    root: jni::objects::JString<'local>,
    command: jni::objects::JString<'local>,
    args: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let result = (|| {
        let root: String = env
            .get_string(&root)
            .map_err(|_| "Invalid voice storage argument")?
            .into();
        let command: String = env
            .get_string(&command)
            .map_err(|_| "Invalid validation command")?
            .into();
        let args: String = env
            .get_string(&args)
            .map_err(|_| "Invalid voice package argument")?
            .into();
        validate(Path::new(&root), &command, &args)
    })();
    let response = match result {
        Ok(()) => serde_json::json!({ "value": { "validated": true } }),
        Err(error) => serde_json::json!({ "error": error }),
    };
    env.new_string(response.to_string())
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(directory: &Path) -> String {
        serde_json::json!({
            "directory": directory,
            "manifest": {
                "schema_version": 1, "id": "fixture", "name": "Fixture", "language": "en-US",
                "runtime": "sherpa-vits-v1", "sample_rate": 22050,
                "license": "Synthetic test fixture", "license_url": "https://example.org/LICENSE",
                "artifacts": [
                    {"path":"model.onnx","role":"model","bytes":1,"sha256":"0".repeat(64)},
                    {"path":"tokens.txt","role":"tokens","bytes":1,"sha256":"0".repeat(64)},
                    {"path":"LICENSE","role":"license","bytes":1,"sha256":"0".repeat(64)}
                ]
            }
        })
        .to_string()
    }

    #[test]
    fn validation_never_exposes_queue_commands_or_outside_packages() {
        let fixture = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = fixture.path().join("no_backup/private-reader/voices");
        std::fs::create_dir_all(&root).unwrap();
        let queue = root.join("narration.json");
        std::fs::write(&queue, b"unchanged").unwrap();
        assert!(validate(&root, "prepare", "{}")
            .unwrap_err()
            .contains("Only voice package"));
        assert!(validate(&root, "validatePackage", &request(fixture.path()))
            .unwrap_err()
            .contains("inside private voice storage"));
        assert_eq!(std::fs::read(queue).unwrap(), b"unchanged");
    }

    #[test]
    fn confined_packages_still_require_actual_verified_artifacts() {
        let fixture = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = fixture.path().join("no_backup/private-reader/voices");
        let package = root.join("staging-fixture");
        std::fs::create_dir_all(&package).unwrap();
        assert!(validate(&root, "validatePackage", &request(&package))
            .unwrap_err()
            .contains("Voice artifact is missing"));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_links_cannot_escape_the_private_package() {
        let fixture = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = fixture.path().join("no_backup/private-reader/voices");
        let package = root.join("staging-fixture");
        std::fs::create_dir_all(&package).unwrap();
        let outside = fixture.path().join("outside.onnx");
        std::fs::write(&outside, b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, package.join("model.onnx")).unwrap();
        assert!(validate(&root, "validatePackage", &request(&package))
            .unwrap_err()
            .contains("escapes its package directory"));
        assert_eq!(std::fs::read(outside).unwrap(), b"outside");
    }
}
