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

/// Formats `text` as a real nested child block at `depth` levels under its
/// parent (a top-level heading like `# Title` / `## Summary` /
/// `## Transcript`).
///
/// Grafium's markdown parser infers block nesting purely from leading-space
/// depth (Logseq-style outline, 2 spaces per level) — but critically, a
/// line only becomes a genuine **child block** if it starts with a `- `
/// bullet marker; a plainly-indented heading/paragraph with no bullet is
/// instead swallowed as a *continuation line* of whatever block precedes
/// it (or, if separated by a blank line, becomes its own unrelated
/// top-level sibling). So every logical child needs its own `- ` bullet at
/// `depth * 2` leading spaces; any additional wrapped lines within that
/// same bullet's text are indented one level deeper still (matching the
/// convention used by hand-written/other-generated Grafium pages) so they
/// stay part of the bullet instead of becoming new blocks.
fn bullet_child(text: &str, depth: usize) -> String {
    let bullet_prefix = "  ".repeat(depth);
    let continuation_prefix = "  ".repeat(depth + 1);
    let mut lines = text.lines();
    let mut out = match lines.next() {
        Some(first) => format!("{bullet_prefix}- {first}"),
        None => format!("{bullet_prefix}- "),
    };
    for line in lines {
        out.push('\n');
        if line.trim().is_empty() {
            continue;
        }
        out.push_str(&continuation_prefix);
        out.push_str(line);
    }
    out
}

/// Builds the full markdown content for a transcript note: a `# Title`
/// heading followed by human-readable metadata (source URL, uploader,
/// duration, where the transcript came from, when it was fetched, tags —
/// each its own visible bullet, not `key:: value` properties or `---` YAML
/// frontmatter, since Grafium's page parser doesn't recognize YAML and the
/// UI never renders hidden block/page properties anywhere), an optional
/// AI-generated "## Summary" section — one `###`-headed paragraph +
/// `#hashtag` tags per distinct topic discussed, plus a title-answer line —
/// when `summary` is available, followed by the transcript body, grouped
/// into `~30s` chunks each prefixed with a `**[mm:ss]**` timestamp marker.
///
/// Everything after the title — properties, the summary, and the
/// transcript — is nested as a genuine **child block** of the title block
/// (real `parent_id` links once imported), not emitted as page-level
/// frontmatter or top-level sibling blocks. This matches how the rest of
/// a Grafium page reads (an outline rooted at the page's main heading)
/// and means metadata like the source URL/uploader/duration is visible
/// directly in the outline instead of hidden in a `properties` field the
/// UI never renders.
pub fn transcript_to_markdown(
    url: &str,
    metadata: &VideoMetadata,
    transcript: &Transcript,
    source: TranscriptSource,
    summary: Option<&PageSummary>,
) -> String {
    let mut out = String::new();

    if let Some(title) = &metadata.title {
        out.push_str(&format!("# {}\n\n", single_line(title)));
    }

    // Human-readable metadata, one property per visible child bullet of
    // the title — a real block each, not a hidden `key:: value` property
    // line (those are never rendered anywhere in the UI, so a reader
    // could never actually see the source URL/uploader/duration if they
    // were stored that way).
    out.push_str(&bullet_child(&format!("Source: {url}"), 1));
    out.push_str("\n\n");
    if let Some(uploader) = &metadata.uploader {
        out.push_str(&bullet_child(&format!("Uploader: {}", single_line(uploader)), 1));
        out.push_str("\n\n");
    }
    if let Some(duration) = metadata.duration_seconds {
        out.push_str(&bullet_child(
            &format!("Duration: {}", format_timestamp((duration * 1000.0) as i64)),
            1,
        ));
        out.push_str("\n\n");
    }
    out.push_str(&bullet_child(&format!("Transcript source: {}", source.label()), 1));
    out.push_str("\n\n");
    out.push_str(&bullet_child(
        &format!("Imported: {}", chrono::Utc::now().to_rfc3339()),
        1,
    ));
    out.push_str("\n\n");
    if let Some(summary) = summary {
        let all_tags = summary.all_tags();
        if !all_tags.is_empty() {
            out.push_str(&bullet_child(&format!("Tags: {}", all_tags.join(", ")), 1));
            out.push_str("\n\n");
        }
    }

    if let Some(summary) = summary {
        // "## Summary" itself nests one level under the title (depth 1),
        // and everything inside it nests one level deeper still (depth
        // 2/3) than it used to when Summary was a top-level sibling of
        // the title.
        out.push_str(&bullet_child("## Summary", 1));
        out.push_str("\n\n");
        if let Some(title_answer) = &summary.title_answer {
            out.push_str(&bullet_child(&format!("**{title_answer}**"), 2));
            out.push_str("\n\n");
        }
        // One heading + paragraph per distinct topic, rather than a single
        // blended summary, so a long multi-subject recording (e.g. a
        // podcast covering many topics) keeps every topic distinguishable
        // once the transcript below is eventually deleted and this
        // section becomes the only record of what was discussed.
        //
        // Each `### Topic` heading nests one level under `## Summary`, and
        // its paragraph + hashtags nest one level further under that
        // heading — so the outline reads Title > Summary > Topic >
        // (text, tags) instead of unrelated sibling blocks.
        for topic in &summary.topics {
            out.push_str(&bullet_child(&format!("### {}", topic.topic.trim()), 2));
            out.push_str("\n\n");
            out.push_str(&bullet_child(topic.summary.trim(), 3));
            out.push_str("\n\n");
            if !topic.tags.is_empty() {
                let hashtags = topic
                    .tags
                    .iter()
                    .map(|tag| format!("#{}", tag.label().replace(' ', "_")))
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&bullet_child(&hashtags, 3));
                out.push_str("\n\n");
            }
        }
    }

    // "## Transcript" nests one level under the title too, so the full
    // outline reads Title > (Properties, Summary, Transcript).
    out.push_str(&bullet_child("## Transcript", 1));
    out.push_str("\n\n");

    if transcript.segments.is_empty() {
        out.push_str(&bullet_child(transcript.full_text.trim(), 2));
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
        let line = format!(
            "**[{}]** {}",
            format_timestamp(group_start_ms),
            group_text.trim()
        );
        out.push_str(&bullet_child(&line, 2));
        out.push_str("\n\n");
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

/// Collapses a property value onto a single line, since the page parser's
/// `key:: value` property syntax only reads up to the end of the line.
fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
    fn includes_metadata_as_visible_child_bullets_when_present() {
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
        assert!(md.starts_with("# My Video\n"));
        assert!(md.contains("Source: https://youtu.be/abc123"));
        assert!(md.contains("Uploader: Some Channel"));
        assert!(md.contains("Duration: 0:40"));
        assert!(md.contains("Transcript source: youtube_captions"));
        assert!(md.contains("Imported: "));
        // Metadata must never be emitted as hidden `key:: value` properties,
        // since the UI never renders block/page properties anywhere — a
        // reader could never actually see the source URL/uploader/duration
        // if they were stored that way instead of as visible bullets.
        assert!(!md.contains("source_url::"));
        assert!(!md.contains("uploader::"));
        assert!(!md.contains("duration_seconds::"));
        assert!(!md.contains("transcript_source::"));
        assert!(!md.contains("fetched_at::"));
    }

    #[test]
    fn omits_optional_metadata_bullets_when_missing() {
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata::default(),
            &sample_transcript(),
            TranscriptSource::AutoCaptions,
            None,
        );
        assert!(!md.contains("Uploader:"));
        assert!(!md.contains("Duration:"));
        assert!(!md.contains("Tags:"));
        assert!(md.contains("Transcript source: youtube_auto_captions"));
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
    fn collapses_newlines_in_title_to_a_single_heading_line() {
        let metadata = VideoMetadata {
            title: Some("A weird\ntitle\nwith\nlinebreaks".to_string()),
            ..Default::default()
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &metadata,
            &sample_transcript(),
            TranscriptSource::Whisper,
            None,
        );
        assert!(md.starts_with("# A weird title with linebreaks\n"));
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
        assert!(md.contains("Tags: magnesium, insulin_resistance"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("**Yes, magnesium helps with sleep.**"));
        assert!(md.contains("### Magnesium and sleep"));
        assert!(md.contains(
            "The video covers magnesium's role in sleep and insulin sensitivity."
        ));
        assert!(md.contains("#magnesium #insulin_resistance"));
        // Summary must come before the transcript body.
        assert!(md.find("## Summary").unwrap() < md.find("## Transcript").unwrap());
    }

    #[test]
    fn renders_qualified_tag_as_underscored_hashtag() {
        // A disambiguated tag (e.g. "absorption" -> "body absorption")
        // should render as a single underscored hashtag, since hashtags
        // can't contain spaces, while the visible "Tags:" bullet keeps
        // the qualified label with its spaces intact.
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
        assert!(md.contains("Tags: body absorption"));
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
        assert!(md.contains("### Magnesium and sleep"));
        assert!(md.contains("Magnesium glycinate can improve sleep onset."));
        assert!(md.contains("### Insulin resistance"));
        assert!(md.contains("Cutting refined carbs helps insulin sensitivity."));
        // Both topics' tags should be merged into the "Tags:" bullet.
        assert!(md.contains("Tags: magnesium, insulin_resistance"));
        assert!(md.find("Magnesium and sleep").unwrap() < md.find("Insulin resistance").unwrap());
    }

    /// Regression test: content under a generated heading must actually
    /// import as a *child* block of that heading (Logseq-style outline
    /// nesting), not as a same-level sibling block on the page. Verified by
    /// round-tripping the generated markdown back through the real page
    /// parser and inspecting the resulting block tree, rather than just
    /// checking string content/order.
    ///
    /// Also asserts the title is the *only* top-level block — properties,
    /// "## Summary", and "## Transcript" must all be nested underneath it
    /// (real `parent_id` links once imported), not left as top-level
    /// siblings.
    #[test]
    fn generated_headings_actually_nest_their_content_when_reparsed() {
        let summary = PageSummary {
            title_answer: Some("Yes, magnesium helps with sleep.".to_string()),
            topics: vec![TopicSummary {
                topic: "Magnesium and sleep".to_string(),
                summary: "Magnesium glycinate can improve sleep onset.".to_string(),
                tags: vec!["magnesium".into()],
            }],
        };
        let md = transcript_to_markdown(
            "https://youtu.be/abc123",
            &VideoMetadata {
                title: Some("My Video".to_string()),
                ..Default::default()
            },
            &sample_transcript(),
            TranscriptSource::Whisper,
            Some(&summary),
        );

        let parsed = crate::parser::parse_page(&md, "my-video.md");
        // The title must be the *only* top-level block — properties,
        // Summary, and Transcript are all nested underneath it.
        let top_level_contents: Vec<&str> =
            parsed.blocks.iter().map(|b| b.content.as_str()).collect();
        assert_eq!(
            top_level_contents,
            vec!["# My Video"],
            "unexpected top-level siblings: {:?}",
            top_level_contents
        );

        let title_block = &parsed.blocks[0];
        assert!(
            title_block.children.iter().any(|c| c.content.contains("Source:")),
            "expected 'Source: ...' to be a child of the title block, got children: {:?}",
            title_block.children
        );
        assert!(
            title_block
                .children
                .iter()
                .any(|c| c.content.contains("Transcript source:")),
            "expected 'Transcript source: ...' to be a child of the title block, got children: {:?}",
            title_block.children
        );

        let summary_block = title_block
            .children
            .iter()
            .find(|c| c.content == "## Summary")
            .expect("expected '## Summary' to be a child of the title block");
        let topic_block = summary_block
            .children
            .iter()
            .find(|c| c.content.starts_with("### Magnesium and sleep"))
            .expect("expected the topic heading to be a child of ## Summary");
        assert!(
            topic_block
                .children
                .iter()
                .any(|c| c.content.contains("Magnesium glycinate can improve sleep onset.")),
            "expected the topic's paragraph to be a child of its own heading, got: {:?}",
            topic_block.children
        );
        assert!(
            topic_block.children.iter().any(|c| c.content.contains("#magnesium")),
            "expected the topic's hashtags to be a child of its own heading, got: {:?}",
            topic_block.children
        );

        let transcript_block = title_block
            .children
            .iter()
            .find(|c| c.content == "## Transcript")
            .expect("expected '## Transcript' to be a child of the title block");
        assert!(
            !transcript_block.children.is_empty(),
            "expected timestamped transcript chunks to nest under ## Transcript"
        );
        assert!(transcript_block
            .children
            .iter()
            .any(|c| c.content.contains("**[0:00]**")));
    }
}
