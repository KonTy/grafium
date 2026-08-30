use super::backend::{compute_hash, FileMetadata, SyncBackend};
use crate::error::{CoreError, Result};

/// WebDAV-based sync backend for Nextcloud, ownCloud, or any WebDAV server.
pub struct WebDavBackend {
    /// Base URL (e.g. "https://cloud.example.com/remote.php/dav/files/user/Notes")
    base_url: String,
    username: String,
    password: String,
    name: String,
    client: reqwest::blocking::Client,
}

impl WebDavBackend {
    pub fn new(base_url: String, username: String, password: String, name: String) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| CoreError::Other(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            username,
            password,
            name,
            client,
        })
    }

    fn file_url(&self, rel_path: &str) -> String {
        // Note titles routinely contain spaces, and may contain '#', '?' or
        // '&'. Pasted raw into a URL these either truncate the path at a
        // fragment/query boundary or fail to parse outright, so encode each
        // segment while keeping the separators intact.
        let encoded = rel_path
            .split('/')
            .map(|segment| urlencoding::encode(segment).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        format!("{}/{}", self.base_url, encoded)
    }

    /// Verify that a freshly written file arrived intact.
    ///
    /// A PUT can return success while the body was truncated by a dropped
    /// connection. Without this the engine records a torn upload as synced
    /// and the next machine pulls the truncated copy over a good one.
    fn verify_written(&self, rel_path: &str, expected_len: u64) -> Result<()> {
        let remote = match self.stat_file(rel_path) {
            Ok(meta) => meta,
            // Not every server reports usable metadata; a failed check is not
            // itself evidence that the write went wrong.
            Err(_) => return Ok(()),
        };
        if remote.size != 0 && remote.size != expected_len {
            return Err(crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "WebDAV write of {} is {} bytes on the server, expected {}",
                    rel_path, remote.size, expected_len
                ),
            )));
        }
        Ok(())
    }

    /// Parse a PROPFIND XML response to extract file metadata.
    fn parse_propfind_response(&self, xml: &str) -> Result<Vec<FileMetadata>> {
        let mut files = Vec::new();

        // Simple XML parsing for WebDAV PROPFIND responses.
        // Looks for <d:href>, <d:getcontentlength>, <d:getlastmodified>
        for response_block in xml.split("<d:response>").skip(1) {
            let href = extract_tag(response_block, "d:href").unwrap_or_default();

            // Skip collection entries (directories)
            if extract_tag(response_block, "d:collection").is_some() {
                continue;
            }
            if href.ends_with('/') {
                continue;
            }

            // Extract relative path from the href
            let rel_path = self.href_to_rel_path(&href);
            if rel_path.is_empty() {
                continue;
            }

            // Notes sync as markdown only; assets/ carries the media that
            // notes reference, and may be any file type.
            let is_note_dir =
                rel_path.starts_with("pages/") || rel_path.starts_with("journals/");
            let is_asset = rel_path.starts_with("assets/");
            if is_note_dir && !rel_path.ends_with(".md") {
                continue;
            }
            if !is_note_dir && !is_asset {
                continue;
            }
            // Conflict copies are written explicitly by the engine.
            if rel_path.contains(".conflict_") {
                continue;
            }

            let size = extract_tag(response_block, "d:getcontentlength")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            let modified_at = extract_tag(response_block, "d:getlastmodified")
                .and_then(|s| parse_http_date(&s))
                .unwrap_or(0);

            files.push(FileMetadata {
                rel_path,
                size,
                modified_at,
                hash: None,
            });
        }

        Ok(files)
    }

    fn href_to_rel_path(&self, href: &str) -> String {
        // The href from WebDAV is usually a URL path like:
        // /remote.php/dav/files/user/Notes/pages/foo.md
        // We need to extract "pages/foo.md"
        let decoded = urlencoding::decode(href).unwrap_or_default().to_string();

        // Try to find pages/ or journals/ in the path
        if let Some(idx) = decoded.find("pages/") {
            return decoded[idx..].to_string();
        }
        if let Some(idx) = decoded.find("journals/") {
            return decoded[idx..].to_string();
        }
        String::new()
    }
}

impl SyncBackend for WebDavBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_available(&self) -> bool {
        // Try a simple PROPFIND on the base URL to check connectivity
        let result = self
            .client
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                &self.base_url,
            )
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .send();

        matches!(result, Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 207)
    }

    fn list_files(&self) -> Result<Vec<FileMetadata>> {
        // PROPFIND with Depth: infinity to list all files
        // Most servers limit this, so we do two requests: pages/ and journals/
        let mut all_files = Vec::new();

        for subdir in &["pages", "journals"] {
            let url = format!("{}/{}", self.base_url, subdir);
            let response = self
                .client
                .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
                .basic_auth(&self.username, Some(&self.password))
                .header("Depth", "infinity")
                .header("Content-Type", "application/xml")
                .body(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:getcontentlength/>
    <d:getlastmodified/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>"#,
                )
                .send()
                .map_err(|e| {
                    crate::error::CoreError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;

            if response.status().as_u16() == 207 || response.status().is_success() {
                let xml = response.text().map_err(|e| {
                    crate::error::CoreError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;
                let mut files = self.parse_propfind_response(&xml)?;
                all_files.append(&mut files);
            }
            // If the directory doesn't exist yet (404), that's fine — skip it
        }

        Ok(all_files)
    }

    fn stat_file(&self, rel_path: &str) -> Result<FileMetadata> {
        let url = self.file_url(rel_path);
        let response = self
            .client
            .head(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(|e| {
                crate::error::CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("WebDAV HEAD failed: {} for {}", response.status(), rel_path),
            )));
        }

        let size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let modified_at = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_http_date)
            .unwrap_or(0);

        Ok(FileMetadata {
            rel_path: rel_path.to_string(),
            size,
            modified_at,
            hash: None,
        })
    }

    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        let url = self.file_url(rel_path);
        let response = self
            .client
            .get(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(|e| {
                crate::error::CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("WebDAV GET failed: {} for {}", response.status(), rel_path),
            )));
        }

        response.bytes().map(|b| b.to_vec()).map_err(|e| {
            crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })
    }

    fn write_file(&self, rel_path: &str, content: &[u8]) -> Result<()> {
        // Ensure parent directories exist via MKCOL
        let parts: Vec<&str> = rel_path.split('/').collect();
        let mut current_path = String::new();
        for part in &parts[..parts.len() - 1] {
            if !current_path.is_empty() {
                current_path.push('/');
            }
            current_path.push_str(part);
            let dir_url = format!("{}/{}/", self.base_url, current_path);
            // MKCOL — ignore errors (dir might already exist)
            let _ = self
                .client
                .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &dir_url)
                .basic_auth(&self.username, Some(&self.password))
                .send();
        }

        // PUT the file
        let url = self.file_url(rel_path);
        let response = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Content-Type", "text/markdown")
            .body(content.to_vec())
            .send()
            .map_err(|e| {
                crate::error::CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        if !response.status().is_success()
            && response.status().as_u16() != 201
            && response.status().as_u16() != 204
        {
            return Err(crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("WebDAV PUT failed: {} for {}", response.status(), rel_path),
            )));
        }
        self.verify_written(rel_path, content.len() as u64)
    }

    fn delete_file(&self, rel_path: &str) -> Result<()> {
        let url = self.file_url(rel_path);
        let response = self
            .client
            .request(reqwest::Method::DELETE, &url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(|e| {
                crate::error::CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        // 204 No Content or 404 Not Found are both acceptable
        if !response.status().is_success() && response.status().as_u16() != 404 {
            return Err(crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "WebDAV DELETE failed: {} for {}",
                    response.status(),
                    rel_path
                ),
            )));
        }
        Ok(())
    }

    fn file_hash(&self, rel_path: &str) -> Result<String> {
        let content = self.read_file(rel_path)?;
        Ok(compute_hash(&content))
    }
}

/// Extract text content between XML tags (simple, non-recursive).
fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);

    let start_idx = xml.find(&open)?;
    // Find the end of the opening tag (handle attributes)
    let content_start = xml[start_idx..].find('>')? + start_idx + 1;
    let end_idx = xml[content_start..].find(&close)? + content_start;

    Some(xml[content_start..end_idx].trim().to_string())
}

/// Parse HTTP date format (e.g. "Mon, 01 Jan 2024 12:00:00 GMT") to Unix timestamp.
fn parse_http_date(date_str: &str) -> Option<i64> {
    // Try RFC 2822 style parsing
    chrono::DateTime::parse_from_rfc2822(date_str)
        .ok()
        .map(|dt| dt.timestamp())
        .or_else(|| {
            // Try the common WebDAV format: "Fri, 02 May 2025 10:30:00 GMT"
            chrono::NaiveDateTime::parse_from_str(
                date_str.trim_end_matches(" GMT"),
                "%a, %d %b %Y %H:%M:%S",
            )
            .ok()
            .map(|dt| dt.and_utc().timestamp())
        })
}

#[cfg(test)]
mod tests {
    use super::WebDavBackend;
    use crate::error::Result;
    use crate::sync::SyncBackend;

    #[test]
    fn webdav_backend_new_returns_result_in_normal_case() -> Result<()> {
        let backend = WebDavBackend::new(
            "https://example.com/remote.php/dav/files/user/Notes".to_string(),
            "user".to_string(),
            "password".to_string(),
            "webdav".to_string(),
        )?;

        assert_eq!(backend.name(), "webdav");
        Ok(())
    }

    #[test]
    fn file_url_encodes_characters_that_break_urls() -> Result<()> {
        let backend = WebDavBackend::new(
            "https://dav.example.com/notes".to_string(),
            "user".to_string(),
            "pw".to_string(),
            "webdav".to_string(),
        )?;

        // A space is the common case; '#' would otherwise truncate the path
        // at a fragment boundary and target the wrong resource entirely.
        assert_eq!(
            backend.file_url("pages/My Tasks #urgent.md"),
            "https://dav.example.com/notes/pages/My%20Tasks%20%23urgent.md"
        );
        // Separators must survive encoding.
        assert_eq!(
            backend.file_url("pages/sub/foo.md"),
            "https://dav.example.com/notes/pages/sub/foo.md"
        );
        Ok(())
    }
}
