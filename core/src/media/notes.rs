//! Turns a fetched [`Transcript`] into a Grafium markdown page.
//!
//! Deliberately the one place that knows what a "video/audio transcript
//! note" looks like on disk, so both the caption-scraping path
//! (`captions`) and the Whisper-fallback path (`transcribe`) produce
//! identical-looking notes regardless of which one supplied the
//! [`Transcript`] — the only difference a reader sees is the
//! `transcript_source` frontmatter field.

use crate::ai::references::PageSummary;
use crate::media::captions::VideoMetadata;
use crate::media::types::{Transcript, TranscriptSource};

/// How many milliseconds of transcript to group under one timestamp
/// heading. Keeps long transcripts navigable (Grafium renders each
/// `**[mm:ss]**` marker as a jump point) without creating one block per
/// caption cue, which would be far too granular to read.
const TIMESTAMP_GROUP_MS: i64 = 30_000;

/// Builds the full markdown content for a transcript note: YAML
/// frontmatter (source URL, title, uploader, duration, where the
/// transcript came from, when it was fetched), an optional AI-generated
/// "## Summary" section (title-answer, prose summary, `#hashtag` tags) when
/// `summary` is available, followed by the transcript body, grouped into
/// `~30s` chunks each prefixed with a `**[mm:ss]**` timestamp marker.
pub fn transcript_to_markdown(
    url: &str,
    metadata: &VideoMetadata,
    transcript: &Transcript,
    source: TranscriptSource,
    summary: Option<&PageSummary>,
) -> String {
    let mut out = String::new();

    out.push_str("---\n");
    out.push_str(&format!("source_url: \"{}\"\n", escape_yaml(url)));
    if let Some(title) = &metadata.title {
        out.push_str(&format!("title: \"{}\"\n", escape_yaml(title)));
    }
    if let Some(uploader) = &metadata.uploader {
        out.push_str(&format!("uploader: \"{}\"\n", escape_yaml(uploader)));
    }
    if let Some(duration) = metadata.duration_seconds {
        out.push_str(&format!("duration_seconds: {duration}\n"));
    }
    out.push_str(&format!("transcript_source: {}\n", source.label()));
    out.push_str(&format!(
        "fetched_at: \"{}\"\n",
        chrono::Utc::now().to_rfc3339()
    ));
    if let Some(summary) = summary {
        if !summary.tags.is_empty() {
            let tags_yaml = summary
                .tags
                .iter()
                .map(|tag| format!("\"{}\"", escape_yaml(tag)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("tags: [{tags_yaml}]\n"));
        }
    }
    out.push_str("---\n\n");

    if let Some(title) = &metadata.title {
        out.push_str(&format!("# {title}\n\n"));
    }
    out.push_str(&format!("Source: {url}\n\n"));

    if let Some(summary) = summary {
        out.push_str("## Summary\n\n");
        if let Some(title_answer) = &summary.title_answer {
            out.push_str(&format!("**{title_answer}**\n\n"));
        }
        out.push_str(summary.summary.trim());
        out.push_str("\n\n");
        if !summary.tags.is_empty() {
            let hashtags = summary
                .tags
                .iter()
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&hashtags);
            out.push_str("\n\n");
        }
    }

    out.push_str("## Transcript\n\n");

    if transcript.segments.is_empty() {
        out.push_str(transcript.full_text.trim());
        out.push('\n');
        return out;
    }

    let mut group_start_ms = transcript.segments[0].start_ms;
    let mut group_text = String::new();
    let mut wrote_any_group = false;

    let flush_group = |out: &mut String, group_start_ms: i64, group_text: &str| {
        if group_text.trim().is_empty() {
            return;
        }
        out.push_str(&format!(
            "**[{}]** {}\n\n",
            format_timestamp(group_start_ms),
            group_text.trim()
        ));
    };

    for segment in &transcript.segments {
        if segment.start_ms - group_start_ms >= TIMESTAMP_GROUP_MS && wrote_any_group {
            flush_group(&mut out, group_start_ms, &group_text);
            group_text.clear();
            group_start_ms = segment.start_ms;
        } else if group_text.is_empty() {
            group_start_ms = segment.start_ms;
        }
        if !group_text.is_empty() {
            group_text.push(' ');
        }
        group_text.push_str(segment.text.trim());
        wrote_any_group = true;
    }
    flush_group(&mut out, group_start_ms, &group_text);

    out
}

/// Formats a millisecond offset as `mm:ss` (or `h:mm:ss` past an hour).
fn format_timestamp(ms: i64) -> String {
    let total_seconds = ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Minimal YAML string escaping sufficient for titles/URLs (escape
/// backslashes and double quotes) — good enough since these are
/// human-authored video titles, not arbitrary untrusted YAML.
fn escape_yaml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::TranscriptSegment;

    fn sample_transcript() -> Transcript {
        Transcript::from_segments(vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: 5000,
                text: "Welcome to the video.".to_string(),
            },
            TranscriptSegment {
                start_ms: 5000,
                end_ms: 32_000,
                text: "Today we'll talk about Rust.".to_string(),
            },
            TranscriptSegment {
                start_ms: 32_000,
                end_ms: 40_000,
                text: "Let's get started.".to_string(),
            },
        ])
    }

    #[test]
    fn includes_frontmatter_fields_when_present() {
        let metadata = VideoMetadata {
            title: Some("My Video".to_string()),
            uploader: Some("Some Channel".to_string()),
            duration_seconds: Some(40.0),
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &metadata,
            &sample_transcript(),
            TranscriptSource::CreatorCaptions,
            None,
        );
        assert!(md.starts_with("---\n"));
        assert!(md.contains("source_url: \"https://youtu.be/abc123\""));
        assert!(md.contains("title: \"My Video\""));
        assert!(md.contains("uploader: \"Some Channel\""));
        assert!(md.contains("duration_seconds: 40"));
        assert!(md.contains("transcript_source: youtube_captions"));
        assert!(md.contains("# My Video"));
    }

    #[test]
    fn omits_frontmatter_fields_when_metadata_missing() {
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::AutoCaptions,
            None,
        );
        assert!(!md.contains("title:"));
        assert!(!md.contains("uploader:"));
        assert!(!md.contains("duration_seconds:"));
        assert!(md.contains("transcript_source: youtube_auto_captions"));
    }

    #[test]
    fn groups_segments_into_timestamp_chunks() {
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::Whisper,
            None,
        );
        // First two segments (0ms, 5000ms) are within one 30s group starting
        // at 0:00; the third segment starts at 32s, past the 30s window, so
        // it starts a new group at 0:32.
        assert!(md.contains("**[0:00]** Welcome to the video. Today we'll talk about Rust."));
        assert!(md.contains("**[0:32]** Let's get started."));
    }

    #[test]
    fn format_timestamp_handles_hours_minutes_seconds() {
        assert_eq!(format_timestamp(0), "0:00");
        assert_eq!(format_timestamp(65_000), "1:05");
        assert_eq!(format_timestamp(3_665_000), "1:01:05");
    }

    #[test]
    fn escapes_quotes_and_backslashes_in_title() {
        let metadata = VideoMetadata {
            title: Some(r#"A "quoted" title \ with backslash"#.to_string()),
            ..Default::default()
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &metadata,
            &sample_transcript(),
            TranscriptSource::Whisper,
            None,
        );
        assert!(md.contains(r#"title: "A \"quoted\" title \\ with backslash""#));
    }

    #[test]
    fn falls_back_to_plain_full_text_when_no_segments() {
        let transcript = Transcript {
            segments: vec![],
            full_text: "Just some text, no timing info.".to_string(),
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &transcript,
            TranscriptSource::Whisper,
            None,
        );
        assert!(md.contains("Just some text, no timing info."));
        assert!(!md.contains("**["));
    }

    #[test]
    fn renders_summary_section_with_title_answer_and_hashtags_before_transcript() {
        let summary = PageSummary {
            title_answer: Some("Yes, magnesium helps with sleep.".to_string()),
            summary: "The video covers magnesium's role in sleep and insulin sensitivity."
                .to_string(),
            tags: vec!["magnesium".to_string(), "insulin_resistance".to_string()],
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::Whisper,
            Some(&summary),
        );
        assert!(md.contains("tags: [\"magnesium\", \"insulin_resistance\"]"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("**Yes, magnesium helps with sleep.**"));
        assert!(md.contains(
            "The video covers magnesium's role in sleep and insulin sensitivity."
        ));
        assert!(md.contains("#magnesium #insulin_resistance"));
        // Summary must come before the transcript body.
        assert!(md.find("## Summary").unwrap() < md.find("## Transcript").unwrap());
    }
}
