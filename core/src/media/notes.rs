//! Turns a fetched [`Transcript`] into a Grafium markdown page.
//!
//! Deliberately the one place that knows what a "video/audio transcript
//! note" looks like on disk, so both the caption-scraping path
//! (`captions`) and the Whisper-fallback path (`transcribe`) produce
//! identical-looking notes regardless of which one supplied the
//! [`Transcript`] — the only difference a reader sees is the
//! `transcript_source` frontmatter field.

use crate::ai::references::PageSummary;
#[cfg(test)]
use crate::ai::references::TopicSummary;
use crate::media::captions::VideoMetadata;
use crate::media::types::{Transcript, TranscriptSource};

/// How many milliseconds of transcript to group under one timestamp
/// heading. Keeps long transcripts navigable (Grafium renders each
/// `**[mm:ss]**` marker as a jump point) without creating one block per
/// caption cue, which would be far too granular to read.
const TIMESTAMP_GROUP_MS: i64 = 30_000;

/// Builds the full markdown content for a transcript note in Grafium's
/// Logseq-style outliner format: YAML frontmatter, then a nested block tree
/// where the page title, "Source" line, "## Summary" (with one topic per
/// child), and "## Transcript" (with one child per `~30s` timestamp group)
/// are all indented under a single root bullet.
///
/// The prior flat-markdown output made every heading, summary paragraph,
/// and transcript line a top-level sibling, so nothing referenced the
/// heading it belonged to — links, backlinks, and hashtag attribution
/// couldn't tell that a transcript segment "belonged to" the video's
/// summary or title. Producing an outliner tree instead of raw prose fixes
/// that at the source, so every downstream index (search, references,
/// backlinks, block references) sees the intended parentage.
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
        let all_tags = summary.all_tags();
        if !all_tags.is_empty() {
            let tags_yaml = all_tags
                .iter()
                .map(|tag| format!("\"{}\"", escape_yaml(tag)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("tags: [{tags_yaml}]\n"));
        }
    }
    out.push_str("---\n\n");

    // Root block: the page title (or the URL as fallback so there's always
    // exactly one root to reparent everything else under).
    let root_label = metadata
        .title
        .as_deref()
        .map(|t| format!("# {t}"))
        .unwrap_or_else(|| format!("# {url}"));
    push_bullet(&mut out, 0, &root_label);

    // Source URL as first child of the root.
    push_bullet(&mut out, 1, &format!("Source: {url}"));

    if let Some(summary) = summary {
        push_bullet(&mut out, 1, "## Summary");
        if let Some(title_answer) = &summary.title_answer {
            push_bullet(&mut out, 2, &format!("**{title_answer}**"));
        }
        // One heading + paragraph per distinct topic, rather than a single
        // blended summary, so a long multi-subject recording (e.g. a
        // podcast covering many topics) keeps every topic distinguishable
        // once the transcript below is eventually deleted and this
        // section becomes the only record of what was discussed.
        for topic in &summary.topics {
            push_bullet(&mut out, 2, &format!("### {}", topic.topic.trim()));
            push_bullet(&mut out, 3, topic.summary.trim());
            if !topic.tags.is_empty() {
                let hashtags = topic
                    .tags
                    .iter()
                    .map(|tag| format!("#{}", tag.label().replace(' ', "_")))
                    .collect::<Vec<_>>()
                    .join(" ");
                push_bullet(&mut out, 3, &hashtags);
            }
        }
    }

    push_bullet(&mut out, 1, "## Transcript");

    if transcript.segments.is_empty() {
        let text = transcript.full_text.trim();
        if !text.is_empty() {
            push_bullet(&mut out, 2, text);
        }
        return out;
    }

    let mut group_start_ms = transcript.segments[0].start_ms;
    let mut group_text = String::new();
    let mut wrote_any_group = false;

    let flush_group = |out: &mut String, group_start_ms: i64, group_text: &str| {
        if group_text.trim().is_empty() {
            return;
        }
        push_bullet(
            out,
            2,
            &format!(
                "**[{}]** {}",
                format_timestamp(group_start_ms),
                group_text.trim()
            ),
        );
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

/// Emits one bullet at `depth` (2 spaces per level, Logseq's on-disk convention).
/// A multi-line body is folded into a single block by joining with spaces —
/// Grafium's parser splits blocks on blank lines / new bullets, not newlines
/// inside a bullet, but keeping content on a single line is the simplest
/// guarantee that a paragraph won't accidentally split into siblings.
fn push_bullet(out: &mut String, depth: usize, content: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str("- ");
    let single_line = content
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    out.push_str(&single_line);
    out.push('\n');
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
    use crate::parser::TagTerm;

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
        assert!(md.contains("- # My Video\n"));
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
        // Transcript groups are children of the "## Transcript" heading, which
        // is itself a child of the page's root "#" title bullet. Two levels of
        // indent (4 spaces) with a leading "- " puts each group as a sibling
        // block linkable to its parent transcript heading.
        assert!(
            md.contains("    - **[0:00]** Welcome to the video. Today we'll talk about Rust.\n")
        );
        assert!(md.contains("    - **[0:32]** Let's get started.\n"));
    }

    #[test]
    fn transcript_body_is_a_child_of_the_transcript_heading_not_a_sibling() {
        // Regression: everything under "## Transcript" must live under it in
        // the outliner tree so backlinks and block references can attribute
        // a segment to its video. Prior to the outliner rewrite, headings and
        // segments were all flat top-level blocks with no parent-child link.
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata {
                title: Some("Rust Talk".to_string()),
                ..Default::default()
            },
            &sample_transcript(),
            TranscriptSource::Whisper,
            None,
        );
        let title_line = md.lines().find(|l| l.contains("# Rust Talk")).unwrap();
        let transcript_line = md.lines().find(|l| l.contains("## Transcript")).unwrap();
        let segment_line = md.lines().find(|l| l.contains("**[0:00]**")).unwrap();
        assert!(title_line.starts_with("- "), "title is the root bullet");
        assert!(
            transcript_line.starts_with("  - "),
            "Transcript heading is a child of the title"
        );
        assert!(
            segment_line.starts_with("    - "),
            "Segment is a child of the Transcript heading"
        );
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
        // Even the segment-less fallback body must live under the Transcript
        // heading, not as a top-level sibling.
        assert!(md.contains("    - Just some text, no timing info.\n"));
        assert!(!md.contains("**["));
    }

    #[test]
    fn renders_summary_section_with_title_answer_and_hashtags_before_transcript() {
        let summary = PageSummary {
            title_answer: Some("Yes, magnesium helps with sleep.".to_string()),
            topics: vec![TopicSummary {
                topic: "Magnesium and sleep".to_string(),
                summary: "The video covers magnesium's role in sleep and insulin sensitivity."
                    .to_string(),
                tags: vec!["magnesium".into(), "insulin_resistance".into()],
            }],
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::Whisper,
            Some(&summary),
        );
        assert!(md.contains("tags: [\"magnesium\", \"insulin_resistance\"]"));
        assert!(md.contains("  - ## Summary\n"));
        assert!(md.contains("    - **Yes, magnesium helps with sleep.**\n"));
        assert!(md.contains("    - ### Magnesium and sleep\n"));
        assert!(md.contains(
            "      - The video covers magnesium's role in sleep and insulin sensitivity.\n"
        ));
        assert!(md.contains("      - #magnesium #insulin_resistance\n"));
        // Summary must come before the transcript body.
        assert!(md.find("## Summary").unwrap() < md.find("## Transcript").unwrap());
    }

    #[test]
    fn renders_qualified_tag_as_underscored_hashtag() {
        // A disambiguated tag (e.g. "absorption" -> "body absorption")
        // should render as a single underscored hashtag, since hashtags
        // can't contain spaces, while the frontmatter tags list keeps the
        // qualified label with its spaces intact (YAML strings can).
        let summary = PageSummary {
            title_answer: None,
            topics: vec![TopicSummary {
                topic: "Magnesium absorption".to_string(),
                summary: "The gut's absorption of magnesium was studied.".to_string(),
                tags: vec![TagTerm {
                    term: "absorption".to_string(),
                    qualified: Some("body absorption".to_string()),
                }],
            }],
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::Whisper,
            Some(&summary),
        );
        assert!(md.contains("tags: [\"body absorption\"]"));
        assert!(md.contains("#body_absorption"));
    }

    #[test]
    fn renders_multiple_topics_as_separate_headed_paragraphs() {
        let summary = PageSummary {
            title_answer: None,
            topics: vec![
                TopicSummary {
                    topic: "Magnesium and sleep".to_string(),
                    summary: "Magnesium glycinate can improve sleep onset.".to_string(),
                    tags: vec!["magnesium".into()],
                },
                TopicSummary {
                    topic: "Insulin resistance".to_string(),
                    summary: "Cutting refined carbs helps insulin sensitivity.".to_string(),
                    tags: vec!["insulin_resistance".into()],
                },
            ],
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::Whisper,
            Some(&summary),
        );
        assert!(md.contains("    - ### Magnesium and sleep\n"));
        assert!(md.contains("      - Magnesium glycinate can improve sleep onset.\n"));
        assert!(md.contains("    - ### Insulin resistance\n"));
        assert!(md.contains("      - Cutting refined carbs helps insulin sensitivity.\n"));
        // Both topics' tags should be merged into the frontmatter tags list.
        assert!(md.contains("tags: [\"magnesium\", \"insulin_resistance\"]"));
        assert!(md.find("Magnesium and sleep").unwrap() < md.find("Insulin resistance").unwrap());
    }
}
