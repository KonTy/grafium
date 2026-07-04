//! Import Anki `.apkg` decks into a grafium graph.
//!
//! An `.apkg` file is a ZIP archive containing:
//!   - `collection.anki2` (or `collection.anki21`): a SQLite database with the
//!     `notes` (fields) and `col` (note-type models) tables.
//!   - `media`: a JSON map of `{"<number>": "<real filename>"}`.
//!   - numbered files (`0`, `1`, ...): the actual media bytes.
//!
//! We convert one deck into a single markdown page under `pages/`, where each
//! note becomes a `Front :: Back` flashcard bullet tagged with the deck topic.
//! Referenced media (`[sound:x.mp3]`, `<img src="y">`) is copied into
//! `assets/anki/<deck>/` and rewritten to markdown media links so grafium's
//! renderer can play/show it.

use crate::error::{CoreError, Result};
use crate::graph::Graph;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Result of importing a single `.apkg` file.
#[derive(Debug, Clone, Serialize)]
pub struct AnkiImportSummary {
    /// Human-readable deck name (from the file stem).
    pub deck: String,
    /// The markdown page title the cards were written to.
    pub page_title: String,
    /// The topic tag applied to every imported card.
    pub topic: String,
    /// Number of notes read from the deck.
    pub note_count: usize,
    /// Number of `Front :: Back` flashcards produced.
    pub card_count: usize,
    /// Number of media files copied into the graph.
    pub media_count: usize,
}

/// Progress update emitted during an import so callers can drive a progress UI.
#[derive(Debug, Clone, Serialize)]
pub struct ImportProgress {
    /// Coarse phase: "reading", "media", "indexing", or "done".
    pub phase: String,
    /// Items processed so far in this phase (0 when not applicable).
    pub current: u64,
    /// Total items in this phase (0 = indeterminate).
    pub total: u64,
}

/// Delete a temp file when dropped.
struct TmpFile(PathBuf);
impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Import an Anki `.apkg` into `graph`, creating one markdown page of flashcards.
pub fn import_apkg(graph: &Graph, apkg_path: &Path) -> Result<AnkiImportSummary> {
    import_apkg_with_progress(graph, apkg_path, &mut |_| {})
}

/// Like [`import_apkg`], but reports coarse progress through `on_progress` so a
/// caller (e.g. the Tauri command) can drive a progress bar.
pub fn import_apkg_with_progress(
    graph: &Graph,
    apkg_path: &Path,
    on_progress: &mut dyn FnMut(ImportProgress),
) -> Result<AnkiImportSummary> {
    on_progress(ImportProgress { phase: "reading".into(), current: 0, total: 0 });

    let deck = apkg_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Anki Import")
        .to_string();
    let topic = slugify(&deck);

    let file = fs::File::open(apkg_path)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Other(format!("Not a valid .apkg (zip) file: {e}")))?;

    // 1) media map: number -> real filename
    let media_map: HashMap<String, String> = match zip.by_name("media") {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            serde_json::from_str(&s).unwrap_or_default()
        }
        Err(_) => HashMap::new(),
    };

    // 2) extract the collection SQLite DB to a temp file (rusqlite opens by path)
    let db_bytes = read_first_zip_entry(&mut zip, &["collection.anki21", "collection.anki2"])
        .ok_or_else(|| CoreError::Other("No collection.anki2/anki21 found in .apkg".into()))?;
    let tmp_db = std::env::temp_dir().join(format!("grafium_anki_{}.sqlite", uuid::Uuid::new_v4()));
    fs::write(&tmp_db, &db_bytes)?;
    let _guard = TmpFile(tmp_db.clone());

    let conn = Connection::open(&tmp_db)?;

    // 3) note-type models: mid -> (ordered field names, sort-field index)
    let models_json: String = conn.query_row("SELECT models FROM col LIMIT 1", [], |r| r.get(0))?;
    let model_fields = parse_models(&models_json);

    // 4) prepare media output dir
    let assets_dir = graph.root_dir.join("assets").join("anki").join(&topic);
    fs::create_dir_all(&assets_dir)?;

    // 5) iterate notes -> card lines
    let mut referenced: HashSet<String> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    let mut note_count = 0usize;

    {
        let mut stmt = conn.prepare("SELECT mid, flds FROM notes")?;
        let rows = stmt.query_map([], |r| {
            let mid: i64 = r.get(0)?;
            let flds: String = r.get(1)?;
            Ok((mid.to_string(), flds))
        })?;

        for row in rows {
            let (mid, flds) = row?;
            let fields: Vec<&str> = flds.split('\u{1f}').collect();
            let (names, sortf) = model_fields
                .get(&mid)
                .cloned()
                .unwrap_or_else(|| (Vec::new(), 0));
            let _ = &names; // field names currently unused; kept for future labeling

            let front_idx = if sortf < fields.len() { sortf } else { 0 };
            let front = clean_field(fields.get(front_idx).copied().unwrap_or(""), &topic, &mut referenced);

            let mut back_parts: Vec<String> = Vec::new();
            for (i, f) in fields.iter().enumerate() {
                if i == front_idx {
                    continue;
                }
                let cleaned = clean_field(f, &topic, &mut referenced);
                if !cleaned.trim().is_empty() {
                    back_parts.push(cleaned);
                }
            }
            let back = back_parts.join("  ·  ");

            if front.trim().is_empty() && back.trim().is_empty() {
                continue;
            }
            let line = if back.trim().is_empty() {
                format!("- {front}  #{topic}")
            } else {
                format!("- {front} :: {back}  #{topic}")
            };
            lines.push(line);
            note_count += 1;
        }
    }

    // 6) copy referenced media (invert map: real name -> number)
    let name_to_num: HashMap<&String, &String> =
        media_map.iter().map(|(num, name)| (name, num)).collect();
    let mut media_count = 0usize;
    let to_extract: Vec<(String, String)> = referenced
        .iter()
        .filter_map(|name| name_to_num.get(name).map(|num| ((*num).clone(), name.clone())))
        .collect();
    let media_total = to_extract.len() as u64;
    on_progress(ImportProgress { phase: "media".into(), current: 0, total: media_total });
    for (i, (num, name)) in to_extract.into_iter().enumerate() {
        if let Ok(mut entry) = zip.by_name(&num) {
            let mut buf = Vec::new();
            if entry.read_to_end(&mut buf).is_ok() {
                let dest = assets_dir.join(sanitize_filename(&name));
                if fs::write(&dest, &buf).is_ok() {
                    media_count += 1;
                }
            }
        }
        // Throttle updates to ~every 64 files to avoid flooding the IPC channel.
        if media_total > 0 && (i as u64 % 64 == 0 || i as u64 + 1 == media_total) {
            on_progress(ImportProgress {
                phase: "media".into(),
                current: i as u64 + 1,
                total: media_total,
            });
        }
    }

    // 7) build and write the page
    let page_title = deck.clone();
    let card_count = lines.iter().filter(|l| l.contains(" :: ")).count();

    let mut content = String::new();
    content.push_str(&format!("# {page_title}\n\n"));
    content.push_str(&format!(
        "Imported from Anki — {card_count} flashcards. Study them in **Flashcards** (sidebar): they belong to the `#{topic}` topic.\n\n"
    ));
    for l in &lines {
        content.push_str(l);
        content.push('\n');
    }

    let safe_title = page_title.replace('/', "_");
    let file_path = graph.pages_dir.join(format!("{safe_title}.md"));
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&file_path, &content)?;
    on_progress(ImportProgress {
        phase: "indexing".into(),
        current: 0,
        total: card_count as u64,
    });
    graph.index_file(&file_path)?;

    on_progress(ImportProgress { phase: "done".into(), current: 0, total: 0 });

    Ok(AnkiImportSummary {
        deck,
        page_title,
        topic,
        note_count,
        card_count,
        media_count,
    })
}

/// Parse Anki `col.models` JSON into mid -> (ordered field names, sort field idx).
fn parse_models(models_json: &str) -> HashMap<String, (Vec<String>, usize)> {
    let mut out = HashMap::new();
    let models: serde_json::Value = match serde_json::from_str(models_json) {
        Ok(v) => v,
        Err(_) => return out,
    };
    if let Some(obj) = models.as_object() {
        for (mid, m) in obj {
            let sortf = m.get("sortf").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let mut flds: Vec<(u64, String)> = Vec::new();
            if let Some(arr) = m.get("flds").and_then(|v| v.as_array()) {
                for f in arr {
                    let ord = f.get("ord").and_then(|v| v.as_u64()).unwrap_or(0);
                    let name = f
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    flds.push((ord, name));
                }
            }
            flds.sort_by_key(|(o, _)| *o);
            let names = flds.into_iter().map(|(_, n)| n).collect();
            out.insert(mid.clone(), (names, sortf));
        }
    }
    out
}

/// Convert one Anki field (HTML + media refs) into a single-line markdown string.
/// Media references are recorded in `referenced` and rewritten to `../assets/...`.
fn clean_field(raw: &str, topic: &str, referenced: &mut HashSet<String>) -> String {
    let mut s = raw.to_string();

    // [sound:FILE] -> markdown media link (audio/video by extension)
    s = replace_sound_tags(&s, topic, referenced);
    // <img src="FILE"> -> markdown image link
    s = replace_img_tags(&s, topic, referenced);

    // Line breaks -> separators (keep everything on ONE physical line: grafium
    // splits each newline into its own block).
    s = s.replace("<br>", " / ").replace("<br/>", " / ").replace("<br />", " / ");

    // Strip any remaining HTML tags.
    s = strip_html_tags(&s);
    // Decode a few common HTML entities.
    s = decode_entities(&s);
    // Collapse whitespace/newlines.
    s = collapse_ws(&s);
    // Never let a stray `::` be mistaken for the flashcard separator.
    s = s.replace("::", "—");
    // Neutralize `#` so field text (e.g. an etymology's `#NAME?` artifact or a
    // number like `#4804`) can't create stray grafium #tags. The ONLY tag on an
    // imported card must be the deck topic appended by the caller, so the whole
    // deck stays a single clickable topic. Fullwidth `＃` reads the same.
    s = s.replace('#', "＃");

    s.trim().to_string()
}

/// Replace `[sound:FILE]` with a markdown media link, recording FILE.
fn replace_sound_tags(s: &str, topic: &str, referenced: &mut HashSet<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("[sound:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "[sound:".len()..];
        if let Some(end) = after.find(']') {
            let file = after[..end].trim().to_string();
            if !file.is_empty() {
                referenced.insert(file.clone());
                out.push_str(&format!(" ![]({}) ", media_rel_path(topic, &file)));
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Replace `<img ... src="FILE" ...>` with a markdown image link, recording FILE.
fn replace_img_tags(s: &str, topic: &str, referenced: &mut HashSet<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<img") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(end) = after.find('>') {
            let tag = &after[..=end];
            if let Some(src) = extract_attr(tag, "src") {
                let file = src.trim();
                if !file.is_empty() {
                    referenced.insert(file.to_string());
                    out.push_str(&format!(" ![]({}) ", media_rel_path(topic, file)));
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(after);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Relative markdown path (from a page in pages/) to an imported media file.
fn media_rel_path(topic: &str, file: &str) -> String {
    format!("../assets/anki/{}/{}", topic, sanitize_filename(file))
}

/// Extract an HTML attribute value (single or double quoted) from a tag string.
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let key = format!("{}=", attr);
    let idx = lower.find(&key)? + key.len();
    let bytes = tag.as_bytes();
    let quote = *bytes.get(idx)?;
    if quote == b'"' || quote == b'\'' {
        let start = idx + 1;
        let end = tag[start..].find(quote as char)? + start;
        Some(tag[start..end].to_string())
    } else {
        // unquoted: read until whitespace or >
        let end = tag[idx..]
            .find(|c: char| c.is_whitespace() || c == '>')
            .map(|e| e + idx)
            .unwrap_or(tag.len());
        Some(tag[idx..end].to_string())
    }
}

/// Remove all remaining `<...>` HTML tags.
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Decode a handful of common HTML entities.
fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Collapse runs of whitespace (including newlines) into single spaces.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

/// Keep only the basename and strip characters unsafe for a filesystem path.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0' => '_',
            _ => c,
        })
        .collect()
}

/// Lowercase alphanumeric slug (used as the topic tag and asset subdir).
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "anki".to_string()
    } else {
        trimmed
    }
}

/// Read the first matching entry from the zip into memory.
fn read_first_zip_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    names: &[&str],
) -> Option<Vec<u8>> {
    for name in names {
        if let Ok(mut f) = zip.by_name(name) {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_ok() {
                return Some(buf);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("GRE"), "gre");
        assert_eq!(slugify("Chinese HSK 1"), "chinese-hsk-1");
        assert_eq!(slugify("!!!"), "anki");
    }

    #[test]
    fn test_clean_field_sound() {
        let mut refs = HashSet::new();
        let out = clean_field("abandon [sound:abandon.mp3]", "gre", &mut refs);
        assert!(out.contains("![](../assets/anki/gre/abandon.mp3)"));
        assert!(refs.contains("abandon.mp3"));
    }

    #[test]
    fn test_clean_field_html() {
        let mut refs = HashSet::new();
        let out = clean_field("<b>bold</b><br>next &amp; more", "d", &mut refs);
        assert_eq!(out, "bold / next & more");
    }

    #[test]
    fn test_clean_field_escapes_double_colon() {
        let mut refs = HashSet::new();
        let out = clean_field("a :: b", "d", &mut refs);
        assert!(!out.contains("::"));
    }

    #[test]
    fn test_clean_field_neutralizes_hash() {
        let mut refs = HashSet::new();
        // `#NAME?` and `#4804` must NOT survive as parseable grafium tags.
        let out = clean_field("etymology #NAME? see #4804", "d", &mut refs);
        assert!(!out.contains('#'), "stray # should be neutralized: {out}");
        assert!(out.contains("NAME?"), "text content preserved: {out}");
    }

    #[test]
    fn test_extract_attr() {
        assert_eq!(
            extract_attr("<img src=\"a.png\" alt=x>", "src"),
            Some("a.png".to_string())
        );
        assert_eq!(
            extract_attr("<img src='b.jpg'>", "src"),
            Some("b.jpg".to_string())
        );
    }
}
