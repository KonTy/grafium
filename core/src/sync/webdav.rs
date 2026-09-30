use super::backend::{revision_changed, FileMetadata, FileSnapshot, SyncBackend};
use crate::error::{CoreError, Result};
use std::collections::HashSet;

/// WebDAV-based sync backend. Failed or incomplete inventories are never
/// authoritative evidence of deletion.
pub struct WebDavBackend {
    base_url: String,
    username: String,
    password: String,
    name: String,
    client: reqwest::blocking::Client,
}

fn protocol_error(message: impl std::fmt::Display) -> CoreError {
    CoreError::Other(format!("WebDAV: {message}"))
}

fn dav<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace() == Some("DAV:")
        && node.tag_name().name() == name
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Result<roxmltree::Node<'a, 'input>> {
    let mut found = node.children().filter(|node| dav(*node, name));
    let result = found
        .next()
        .ok_or_else(|| protocol_error(format!("Missing DAV:{name}")))?;
    if found.next().is_some() {
        return Err(protocol_error(format!("Duplicate DAV:{name}")));
    }
    Ok(result)
}

fn status_ok(node: roxmltree::Node<'_, '_>) -> Result<()> {
    let status = node.text().unwrap_or("").trim();
    let parts: Vec<_> = status.split_whitespace().collect();
    if parts.len() < 2
        || !parts[0].starts_with("HTTP/")
        || !parts[1]
            .parse::<u16>()
            .is_ok_and(|code| (200..300).contains(&code))
    {
        return Err(protocol_error(format!("Incomplete multistatus: {status}")));
    }
    Ok(())
}

fn decoded_segments(path: &str) -> Result<Vec<String>> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|segment| {
            let bytes = segment.as_bytes();
            for (i, byte) in bytes.iter().enumerate() {
                if *byte == b'%'
                    && (i + 2 >= bytes.len()
                        || !bytes[i + 1].is_ascii_hexdigit()
                        || !bytes[i + 2].is_ascii_hexdigit())
                {
                    return Err(protocol_error("Invalid percent encoding in href"));
                }
            }
            let decoded = urlencoding::decode(segment)
                .map_err(protocol_error)?
                .into_owned();
            if decoded == ".."
                || decoded
                    .chars()
                    .any(|c| c == '/' || c == '\\' || c.is_control())
            {
                return Err(protocol_error("Unsafe href path segment"));
            }
            Ok(decoded)
        })
        .collect()
}

impl WebDavBackend {
    pub fn new(base_url: String, username: String, password: String, name: String) -> Result<Self> {
        let base = url::Url::parse(&base_url).map_err(protocol_error)?;
        if !matches!(base.scheme(), "http" | "https")
            || base.query().is_some()
            || base.fragment().is_some()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path().contains("//")
        {
            return Err(protocol_error(
                "The graph URL must be an HTTP(S) directory without credentials, empty path segments, query, or fragment",
            ));
        }
        decoded_segments(base.path())?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            // A login redirect must not become a successful, empty inventory.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(protocol_error)?;
        Ok(Self {
            base_url: base.as_str().trim_end_matches('/').to_string(),
            username,
            password,
            name,
            client,
        })
    }

    fn file_url(&self, rel_path: &str) -> String {
        let encoded = rel_path
            .split('/')
            .map(|segment| urlencoding::encode(segment).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        format!("{}/{}", self.base_url, encoded)
    }

    fn href_to_rel_path(&self, href: &str) -> Result<String> {
        // Check before Url::join, which would otherwise erase dot segments.
        decoded_segments(href.split(['?', '#']).next().unwrap_or(""))?;
        let base = url::Url::parse(&format!("{}/", self.base_url)).map_err(protocol_error)?;
        let resolved = base.join(href).map_err(protocol_error)?;
        if resolved.origin() != base.origin()
            || resolved.query().is_some()
            || resolved.fragment().is_some()
            || !resolved.username().is_empty()
            || resolved.password().is_some()
            || resolved.path().contains("//")
        {
            return Err(protocol_error("href is not in the configured graph"));
        }
        let root = decoded_segments(base.path())?;
        let path = decoded_segments(resolved.path())?;
        if !path.starts_with(&root) {
            return Err(protocol_error("href is outside the configured graph"));
        }
        Ok(path[root.len()..].join("/"))
    }

    fn parse_propfind_response(&self, xml: &str, requested: &str) -> Result<Vec<FileMetadata>> {
        let doc = roxmltree::Document::parse(xml).map_err(protocol_error)?;
        let root = doc.root_element();
        if !dav(root, "multistatus") {
            return Err(protocol_error("Expected DAV:multistatus"));
        }
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        let mut saw_requested_collection = false;
        for response in root.children().filter(|node| node.is_element()) {
            if !dav(response, "response") {
                return Err(protocol_error("Unexpected element in multistatus"));
            }
            let href = child(response, "href")?
                .text()
                .ok_or_else(|| protocol_error("Empty href"))?;
            let rel_path = self.href_to_rel_path(href.trim())?;
            if rel_path != requested && !rel_path.starts_with(&format!("{requested}/")) {
                return Err(protocol_error(
                    "Inventory contains a resource outside the requested collection",
                ));
            }
            if !seen.insert(rel_path.clone()) {
                return Err(protocol_error("Duplicate resource in multistatus"));
            }
            if response.children().any(|n| dav(n, "status")) {
                status_ok(child(response, "status")?)?;
            }
            if response.children().any(|n| dav(n, "error")) {
                return Err(protocol_error("Resource error in multistatus"));
            }
            let mut properties = Vec::new();
            for propstat in response.children().filter(|n| dav(*n, "propstat")) {
                status_ok(child(propstat, "status")?)?;
                properties.extend(
                    child(propstat, "prop")?
                        .children()
                        .filter(|n| n.is_element()),
                );
            }
            let property = |name: &str| -> Result<roxmltree::Node<'_, '_>> {
                let mut matching = properties.iter().copied().filter(|n| dav(*n, name));
                let node = matching
                    .next()
                    .ok_or_else(|| protocol_error(format!("Missing property {name}")))?;
                if matching.next().is_some() {
                    return Err(protocol_error(format!("Duplicate property {name}")));
                }
                Ok(node)
            };
            let resource_type = property("resourcetype")?;
            let collection = resource_type.children().any(|n| dav(n, "collection"));
            if rel_path == requested {
                if !collection {
                    return Err(protocol_error("Requested directory is not a collection"));
                }
                saw_requested_collection = true;
            }
            if collection {
                continue;
            }
            if resource_type.children().any(|n| n.is_element()) {
                return Err(protocol_error("Unsupported resource type"));
            }
            let size = property("getcontentlength")?
                .text()
                .unwrap_or("")
                .trim()
                .parse::<u64>()
                .map_err(protocol_error)?;
            let modified_at = parse_http_date(property("getlastmodified")?.text().unwrap_or(""))
                .ok_or_else(|| protocol_error("Invalid getlastmodified"))?;
            if super::engine::SyncEngine::is_syncable_path(&rel_path) {
                files.push(FileMetadata {
                    rel_path,
                    size,
                    modified_at,
                    hash: None,
                });
            }
        }
        if !saw_requested_collection {
            return Err(protocol_error("Inventory omitted the requested collection"));
        }
        Ok(files)
    }

    fn create_parents(&self, rel_path: &str) -> Result<()> {
        let parts: Vec<_> = rel_path.split('/').collect();
        for count in 1..parts.len() {
            let response = self
                .client
                .request(
                    reqwest::Method::from_bytes(b"MKCOL").unwrap(),
                    self.file_url(&parts[..count].join("/")),
                )
                .basic_auth(&self.username, Some(&self.password))
                .send()
                .map_err(protocol_error)?;
            if !response.status().is_success() && response.status().as_u16() != 405 {
                return Err(protocol_error(format!(
                    "MKCOL failed: {}",
                    response.status()
                )));
            }
        }
        Ok(())
    }
}

impl SyncBackend for WebDavBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_available(&self) -> bool {
        self.client
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                &self.base_url,
            )
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .send()
            .is_ok_and(|r| r.status().as_u16() == 207)
    }

    fn list_files(&self) -> Result<Vec<FileMetadata>> {
        let mut files = Vec::new();
        for subdir in ["pages", "journals", "knowledge", "assets", "books"] {
            let response = self.client
                .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), self.file_url(subdir))
                .basic_auth(&self.username, Some(&self.password))
                .header("Depth", "infinity").header("Content-Type", "application/xml")
                .body(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getcontentlength/><d:getlastmodified/><d:resourcetype/></d:prop></d:propfind>"#)
                .send().map_err(protocol_error)?;
            match response.status().as_u16() {
                404 => continue,
                207 => {
                    files.extend(self.parse_propfind_response(
                        &response.text().map_err(protocol_error)?,
                        subdir,
                    )?)
                }
                _ => {
                    return Err(protocol_error(format!(
                        "PROPFIND {subdir} failed: {}",
                        response.status()
                    )))
                }
            }
        }
        Ok(files)
    }

    fn stat_file(&self, rel_path: &str) -> Result<FileMetadata> {
        let response = self
            .client
            .head(self.file_url(rel_path))
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(protocol_error)?;
        if response.status().as_u16() == 404 {
            return Err(CoreError::NotFound(rel_path.into()));
        }
        if !response.status().is_success() {
            return Err(protocol_error(format!(
                "HEAD {rel_path} failed: {}",
                response.status()
            )));
        }
        let size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| protocol_error("HEAD omitted a valid Content-Length"))?;
        let modified_at = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_http_date)
            .unwrap_or(0);
        Ok(FileMetadata {
            rel_path: rel_path.into(),
            size,
            modified_at,
            hash: None,
        })
    }

    fn read_snapshot(&self, rel_path: &str) -> Result<FileSnapshot> {
        let response = self
            .client
            .get(self.file_url(rel_path))
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(protocol_error)?;
        if response.status().as_u16() == 404 {
            return Ok(FileSnapshot {
                content: None,
                etag: None,
                mutation_fence: None,
            });
        }
        if response.status().as_u16() != 200 {
            return Err(protocol_error(format!(
                "GET {rel_path} failed: {}",
                response.status()
            )));
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(FileSnapshot {
            content: Some(response.bytes().map_err(protocol_error)?.to_vec()),
            etag,
            mutation_fence: None,
        })
    }

    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        self.read_snapshot(rel_path)?
            .content
            .ok_or_else(|| CoreError::NotFound(rel_path.into()))
    }

    fn write_file(&self, rel_path: &str, content: &[u8]) -> Result<()> {
        let expected = self.read_snapshot(rel_path)?;
        self.publish_if_unchanged(rel_path, &expected, Some(content))
    }

    fn delete_file(&self, rel_path: &str) -> Result<()> {
        let expected = self.read_snapshot(rel_path)?;
        self.publish_if_unchanged(rel_path, &expected, None)
    }

    fn publish_if_unchanged(
        &self,
        rel_path: &str,
        expected: &FileSnapshot,
        content: Option<&[u8]>,
    ) -> Result<()> {
        if expected.content.is_none() && content.is_none() {
            if self.read_snapshot(rel_path)?.content.is_some() {
                return Err(revision_changed(rel_path));
            }
            return Ok(());
        }
        let etag = if expected.content.is_some() {
            Some(
                expected
                    .etag
                    .as_deref()
                    .filter(|tag| {
                        tag.len() >= 2
                            && tag.starts_with('"')
                            && tag.ends_with('"')
                            && !tag[1..tag.len() - 1]
                                .chars()
                                .any(|c| c == '"' || c.is_control())
                    })
                    .ok_or_else(|| {
                        protocol_error(
                            "Server omitted a strong ETag; refusing an unsafe overwrite/delete",
                        )
                    })?,
            )
        } else {
            None
        };
        if content.is_some() {
            self.create_parents(rel_path)?;
        }
        let method = if content.is_some() {
            reqwest::Method::PUT
        } else {
            reqwest::Method::DELETE
        };
        let mut request = self
            .client
            .request(method, self.file_url(rel_path))
            .basic_auth(&self.username, Some(&self.password));
        request = match etag {
            Some(etag) => request.header(reqwest::header::IF_MATCH, etag),
            None => request.header(reqwest::header::IF_NONE_MATCH, "*"),
        };
        if let Some(content) = content {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(content.to_vec());
        }
        let response = request.send().map_err(protocol_error)?;
        if matches!(response.status().as_u16(), 404 | 412) {
            return Err(revision_changed(rel_path));
        }
        // DELETE 207 is not a success without inspecting every response.
        if !matches!(response.status().as_u16(), 200 | 201 | 204) {
            return Err(protocol_error(format!(
                "Conditional publication {rel_path} failed: {}",
                response.status()
            )));
        }
        if self.read_snapshot(rel_path)?.content.as_deref() != content {
            return Err(protocol_error(format!(
                "Publication verification failed for {rel_path}"
            )));
        }
        Ok(())
    }
}

fn parse_http_date(date_str: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(date_str.trim())
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(root: &str) -> WebDavBackend {
        WebDavBackend::new(root.into(), "user".into(), "pw".into(), "test".into()).unwrap()
    }

    fn response(prefix: &str, href: &str, collection: bool) -> String {
        let collection = if collection {
            format!("<{prefix}collection/>")
        } else {
            String::new()
        };
        format!("<{prefix}response><{prefix}href>{href}</{prefix}href><{prefix}propstat><{prefix}prop><{prefix}resourcetype>{collection}</{prefix}resourcetype><{prefix}getcontentlength>12</{prefix}getcontentlength><{prefix}getlastmodified>Mon, 01 Jan 2024 12:00:00 GMT</{prefix}getlastmodified></{prefix}prop><{prefix}status>HTTP/1.1 200 OK</{prefix}status></{prefix}propstat></{prefix}response>")
    }

    #[test]
    fn namespace_prefixes_and_original_books() {
        let backend = backend("https://dav.example/notes");
        let id = uuid::Uuid::new_v4();
        for prefix in ["D:", "arbitrary:", ""] {
            let namespace = if prefix.is_empty() {
                "xmlns".into()
            } else {
                format!("xmlns:{}", prefix.trim_end_matches(':'))
            };
            let mut xml = format!(
                "<{prefix}multistatus {namespace}=\"DAV:\">{}",
                response(prefix, "/notes/books/", true)
            );
            for file in [
                "book.json",
                "position.json",
                "original.azw3",
                ".extract-cache/converted.epub",
                "cached.txt",
            ] {
                xml.push_str(&response(
                    prefix,
                    &format!("/notes/books/{id}/{file}"),
                    false,
                ));
            }
            xml.push_str(&format!("</{prefix}multistatus>"));
            assert_eq!(
                backend
                    .parse_propfind_response(&xml, "books")
                    .unwrap()
                    .len(),
                3
            );
        }
    }

    #[test]
    fn rejects_partial_malformed_or_failed_multistatus() {
        let backend = backend("https://dav.example/notes");
        let xml = format!(
            "<d:multistatus xmlns:d=\"DAV:\">{}{}</d:multistatus>",
            response("d:", "/notes/pages/", true),
            response("d:", "/notes/pages/note.md", false)
        );
        for bad in [
            xml.replace("200 OK", "403 Forbidden"),
            xml.replace("200 OK", "507 Insufficient Storage"),
            xml.replace("DAV:", "not-DAV"),
            xml.replace("getcontentlength", "unknown"),
            xml.replace("</d:multistatus>", ""),
            "<d:multistatus xmlns:d=\"DAV:\"/>".into(),
            format!(
                "<d:multistatus xmlns:d=\"DAV:\">{}</d:multistatus>",
                response("d:", "/notes/pages/a.md", false)
            ),
        ] {
            assert!(
                backend.parse_propfind_response(&bad, "pages").is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn href_is_relative_to_exact_configured_root() {
        let backend = backend("https://dav.example/dav/notebooks/pages/graph%20space");
        for href in [
            "/dav/notebooks/pages/graph%20space/books/id/original.pdf",
            "https://dav.example/dav/notebooks/pages/graph%20space/books/id/original.pdf",
            "books/id/original.pdf",
            "./books/id/original.pdf",
        ] {
            assert_eq!(
                backend.href_to_rel_path(href).unwrap(),
                "books/id/original.pdf"
            );
        }
        assert_eq!(
            backend
                .href_to_rel_path("pages/My%20Tasks%20%23urgent.md")
                .unwrap(),
            "pages/My Tasks #urgent.md"
        );
        for href in [
            "../other/pages/note.md",
            "pages/%2e%2e/note.md",
            "pages/%2Fetc",
            "pages/%5c..",
            "pages/bad%GG",
            "https://other.example/pages/note.md",
            "/dav/notebooks/pages/graph%20spaces/pages/note.md",
            "/pages/note.md",
        ] {
            assert!(backend.href_to_rel_path(href).is_err(), "{href}");
        }
    }

    #[test]
    fn file_url_encodes_reserved_characters() {
        assert_eq!(
            backend("https://dav.example/notes").file_url("pages/My Tasks #urgent.md"),
            "https://dav.example/notes/pages/My%20Tasks%20%23urgent.md"
        );
    }

    #[test]
    fn http_errors_never_become_empty_inventory() {
        use std::io::{Read, Write};
        for status in [403, 500, 501, 200, 302, 207] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            assert!(
                backend(&format!("http://{address}/notes"))
                    .list_files()
                    .is_err(),
                "{status}"
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn only_explicit_not_found_collections_are_empty() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        assert!(backend(&format!("http://{address}/notes"))
            .list_files()
            .unwrap()
            .is_empty());
        server.join().unwrap();
    }

    #[test]
    fn later_collection_failure_discards_an_earlier_success() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for (status, body) in [
                (
                    207,
                    format!(
                        "<d:multistatus xmlns:d=\"DAV:\">{}{}</d:multistatus>",
                        response("d:", "/notes/pages", true),
                        response("d:", "/notes/pages/good.md", false)
                    ),
                ),
                (500, String::new()),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        assert!(backend(&format!("http://{address}/notes"))
            .list_files()
            .is_err());
        server.join().unwrap();
    }

    #[test]
    fn conditional_put_and_delete_require_matching_etag() {
        use std::io::{Read, Write};
        for content in [Some(b"chosen bytes".as_slice()), None] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                if content.is_some() {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = [0; 4096];
                    let len = stream.read(&mut request).unwrap();
                    assert!(String::from_utf8_lossy(&request[..len]).starts_with("MKCOL "));
                    write!(stream, "HTTP/1.1 405 Already exists\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let len = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..len]).to_lowercase();
                assert!(request.starts_with(if content.is_some() { "put " } else { "delete " }));
                assert!(request.contains("if-match: \"original-etag\""), "{request}");
                write!(stream, "HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            let expected = FileSnapshot {
                content: Some(b"original".to_vec()),
                etag: Some("\"original-etag\"".into()),
                mutation_fence: None,
            };
            assert!(backend(&format!("http://{address}/notes"))
                .publish_if_unchanged("pages/doc.md", &expected, content)
                .is_err());
            server.join().unwrap();
        }
    }

    #[test]
    fn missing_or_weak_etag_cannot_authorize_an_overwrite() {
        for etag in [
            None,
            Some("W/\"weak\"".into()),
            Some("\"first\", \"second\"".into()),
        ] {
            let expected = FileSnapshot {
                content: Some(b"original".to_vec()),
                etag,
                mutation_fence: None,
            };
            // No request is sent: refusing is safer than a hash-check/PUT race.
            assert!(backend("http://127.0.0.1:0/notes")
                .publish_if_unchanged("pages/doc.md", &expected, Some(b"replacement"))
                .unwrap_err()
                .to_string()
                .contains("strong ETag"));
        }
    }

    #[test]
    fn failed_or_partial_get_is_not_a_missing_snapshot() {
        use std::io::{Read, Write};
        for status in [403, 500, 206] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            assert!(backend(&format!("http://{address}/notes"))
                .read_snapshot("pages/doc.md")
                .is_err());
            server.join().unwrap();
        }
    }
}
