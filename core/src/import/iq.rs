use crate::{error::Result, iq::IqPack, CoreError};
use std::{fs::File, io::Read, path::Path};

const MAX_PACK_BYTES: u64 = 32 * 1024 * 1024;

/// IQ-marked archives never fall back to recall review if their descriptor is invalid.
pub fn read_iq_pack(path: &Path) -> Result<Option<IqPack>> {
    let mut file = File::open(path)?;
    let json = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        if file.metadata()?.len() > MAX_PACK_BYTES {
            return Err(CoreError::Other("IQ pack exceeds 32 MiB".into()));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_PACK_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PACK_BYTES {
            return Err(CoreError::Other("IQ pack exceeds 32 MiB".into()));
        }
        bytes
    } else {
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| CoreError::Other(format!("Invalid flashcard archive: {e}")))?;
        let mut entry = match archive.by_name("grafium-iq.json") {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => {
                return Err(CoreError::Other(format!(
                    "Could not read IQ descriptor: {e}"
                )))
            }
        };
        let mut bytes = Vec::new();
        (&mut entry)
            .take(MAX_PACK_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PACK_BYTES {
            return Err(CoreError::Other("IQ descriptor exceeds 32 MiB".into()));
        }
        bytes
    };
    let pack: IqPack = serde_json::from_slice(&json)?;
    pack.validate()?;
    Ok(Some(pack))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn iq_marker_selects_assessment_and_invalid_marker_never_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.apkg");
        for content in [
            serde_json::to_vec(&crate::iq::tests::pack()).unwrap(),
            b"{\"kind\":\"iq\"}".to_vec(),
        ] {
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            zip.start_file("grafium-iq.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&content).unwrap();
            zip.finish().unwrap();
            if content.len() > 100 {
                assert!(read_iq_pack(&path).unwrap().is_some());
            } else {
                assert!(read_iq_pack(&path).is_err());
            }
        }
        let zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.finish().unwrap();
        assert!(read_iq_pack(&path).unwrap().is_none());
    }
}
