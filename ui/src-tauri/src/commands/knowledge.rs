//! Tauri commands for the Knowledge Engine — AI, references, vector search, schemas.

use grafium_core::ai::config::{
    AiConfig, AiMode, CloudConfig, LocalConfig, LocalEmbeddingSettings, LocalLlmSettings,
    ProviderType, ReferenceConfig,
};
use grafium_core::ai::references::{chunk_blocks_by_content_size, PageReferencesMeta};
use grafium_core::ai::traits::SearchResult;
use grafium_core::ai::web_research::Citation;
use grafium_core::db::PageKindFilter;
use grafium_core::knowledge::conversation::{self, ChatTurn};
use grafium_core::knowledge::engine::{AskStreamEvent, HealthStatus, IndexStatus, Source};
use grafium_core::knowledge::registry::{GraphType, RegisteredGraph};
use grafium_core::knowledge::schemas::Schema;
use grafium_core::knowledge::{detect_research_intent, KnowledgeEngine};
use grafium_core::model_library::LocalModelRef;
use grafium_core::models::Block;
pub use grafium_core::graph::{AiInsertSummaryResult, SummaryWrapChange};
use grafium_core::parser::links::ExtractedLink;
use grafium_core::parser::TagTerm;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;

use super::jobs::{JobLink, JobsState};

/// Shared state for the knowledge engine.
pub struct KnowledgeState {
    pub engine: Arc<RwLock<Option<KnowledgeEngine>>>,
    /// Per-request cancellation flags for in-flight streamed answers, so
    /// `ai_cancel_stream` can abort a slow local generation. Keyed by the
    /// UI-supplied `request_id`; entries are inserted when a stream starts and
    /// removed when it ends.
    pub cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

const AI_INDEX_BATCH_SIZE: i64 = 100;
const CONCEPT_EDGE_CHUNK_CHARS: usize = 4_000;
const CONCEPT_EDGE_MAX_CHUNKS_PER_RUN: usize = 12;
const CONCEPT_EDGE_MAX_SEED_TAGS: usize = 80;
const CONCEPT_EDGE_MAX_TAGS_PER_RUN: usize = 40;
const CONCEPT_EDGE_MAX_CANDIDATES_PER_RUN: usize = 1_000;
const LOW_SIGNAL_CONCEPT_EDGE_TERMS: &[&str] = &[
    "analogy",
    "analogies",
    "body",
    "bodies",
    "bible",
    "christianity",
    "consciousness",
    "energy",
    "concept",
    "concepts",
    "date",
    "dates",
    "deadline",
    "dose",
    "doses",
    "dosage",
    "example",
    "examples",
    "benefit",
    "benefits",
    "closed",
    "good",
    "idea",
    "ideas",
    "item",
    "items",
    "journey",
    "journeys",
    "god",
    "name",
    "names",
    "light",
    "life",
    "man",
    "men",
    "mind",
    "object",
    "objects",
    "matter",
    "path",
    "paths",
    "perception",
    "project",
    "projects",
    "realm",
    "realms",
    "subject",
    "subjects",
    "soul",
    "source",
    "spirit",
    "section",
    "sections",
    "scheduled",
    "supplement",
    "supplements",
    "term",
    "terms",
    "theme",
    "themes",
    "thing",
    "things",
    "topic",
    "topics",
    "type",
    "types",
    "vitamin",
    "vitamins",
    "woman",
    "women",
    "word",
    "words",
    "world",
    "unit",
    "units",
    "use",
    "uses",
];

const TITLE_CONNECTOR_WORDS: &[&str] = &[
    "and", "as", "for", "from", "in", "into", "of", "on", "the", "to", "with", "within",
];
const COMMON_SENTENCE_START_WORDS: &[&str] = &[
    "a", "an", "as", "at", "beyond", "by", "each", "every", "for", "from", "good", "helps",
    "however", "if", "in", "it", "let", "may", "on", "reduce", "reduces", "some", "support",
    "supports", "the", "these", "this", "those", "thus", "to", "unlike", "used", "when", "where",
    "while", "within",
];

/// Error message for commands that need semantic search (indexing, vector
/// search, "research this page" references) when the engine's LLM is fine
/// but no embedder is configured. Distinct from the plain "not ready at
/// all" case so the user gets an actionable explanation instead of a
/// generic "not ready" that reads like a bug even when the provider is
/// loaded and working fine for chat.
fn semantic_search_unavailable_error(engine: &KnowledgeEngine) -> String {
    if !engine.is_llm_ready() {
        "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge Engine."
            .to_string()
    } else {
        "This needs a search embedding model, but none is configured yet. If you're using the \
         Embedded (llama.cpp) provider, download a GGUF embedding model (e.g. \
         nomic-embed-text-v1.5-GGUF or bge-small-en-v1.5-gguf from Hugging Face) and select it \
         under \"Embedding Model File\" in Settings \u{2192} AI / Knowledge Engine — or switch \
         the local provider to Ollama / vLLM / OpenAI-compatible, or configure a cloud embedding \
         provider."
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConceptEdgesResult {
    pub tag_count: usize,
    pub suggested_edges: usize,
    pub limit_reached: bool,
    pub chunks_analyzed: usize,
    pub chunks_total: usize,
    pub chunks_failed: usize,
    pub tags: Vec<TagTerm>,
}

fn concept_edge_block_content(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("```")
        || trimmed.starts_with("![")
        || trimmed.starts_with("Source file:")
        || trimmed.starts_with("Format:")
        || trimmed.starts_with("Source SHA-256:")
        || trimmed.starts_with("Imported:")
        || trimmed == "Original file was not copied into this graph."
        || trimmed == "Import notes"
    {
        return None;
    }
    Some(trimmed)
}

#[derive(Debug, Clone)]
struct ConceptEdgeTextChunk {
    source_index: usize,
    block_count: usize,
    text: String,
}

fn concept_edge_text_chunks(blocks: &[Block]) -> Vec<ConceptEdgeTextChunk> {
    let blocks = grafium_core::knowledge::source_projection::project_source_blocks(blocks);
    let readable_blocks: Vec<(&str, &str)> = blocks
        .iter()
        .filter_map(|block| {
            concept_edge_block_content(&block.content).map(|content| (block.id.as_str(), content))
        })
        .collect();
    chunk_blocks_by_content_size(&readable_blocks, CONCEPT_EDGE_CHUNK_CHARS)
        .into_iter()
        .enumerate()
        .map(|(source_index, chunk)| ConceptEdgeTextChunk {
            source_index,
            block_count: chunk.len(),
            text: chunk
                .into_iter()
                .map(|(_, content)| content)
                .collect::<Vec<_>>()
                .join("\n\n"),
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ConceptEdgeSeedHit {
    tag: TagTerm,
    count: usize,
    first_seen: usize,
    specificity: usize,
}

fn document_concept_edge_seed_tags(blocks: &[Block]) -> Vec<TagTerm> {
    let mut hits: HashMap<String, ConceptEdgeSeedHit> = HashMap::new();
    let mut first_seen = 0usize;

    for block in grafium_core::knowledge::source_projection::project_source_blocks(blocks) {
        let Some(content) = concept_edge_block_content(&block.content) else {
            continue;
        };
        for phrase in leading_heading_phrases(content) {
            record_seed_tag(&mut hits, &mut first_seen, &phrase, true);
        }
        for phrase in definition_lead_phrases(content) {
            record_seed_tag(&mut hits, &mut first_seen, &phrase, true);
        }
        for phrase in titlecase_concept_phrases(content) {
            record_seed_tag(&mut hits, &mut first_seen, &phrase, false);
        }
    }

    let mut ranked = hits.into_values().collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| b.specificity.cmp(&a.specificity))
            .then_with(|| a.first_seen.cmp(&b.first_seen))
    });

    ranked
        .into_iter()
        .take(CONCEPT_EDGE_MAX_SEED_TAGS)
        .flat_map(|hit| std::iter::repeat_n(hit.tag, hit.count.min(4)))
        .collect()
}

fn record_seed_tag(
    hits: &mut HashMap<String, ConceptEdgeSeedHit>,
    first_seen: &mut usize,
    raw_phrase: &str,
    heading: bool,
) {
    let Some(tag) = seed_tag_from_phrase(raw_phrase) else {
        return;
    };
    if !meaningful_concept_edge_tag(&tag) {
        return;
    }
    let key = tag.label().trim().to_ascii_lowercase();
    if key.is_empty() {
        return;
    }
    let increment = if heading { 3 } else { 1 };
    hits.entry(key)
        .and_modify(|hit| hit.count += increment)
        .or_insert_with(|| {
            let hit = ConceptEdgeSeedHit {
                specificity: concept_edge_tag_specificity_score(&tag),
                tag,
                count: increment,
                first_seen: *first_seen,
            };
            *first_seen += 1;
            hit
        });
}

fn seed_tag_from_phrase(raw_phrase: &str) -> Option<TagTerm> {
    let cleaned = normalize_concept_phrase(raw_phrase);
    if cleaned.is_empty() {
        return None;
    }
    let term = remove_leading_the(&cleaned);
    if term.chars().count() < 3 {
        return None;
    }
    let qualified = canonical_concept_label(&term);
    Some(TagTerm {
        qualified: (!qualified.eq_ignore_ascii_case(&term)).then_some(qualified),
        term,
    })
}

fn normalize_concept_phrase(raw_phrase: &str) -> String {
    let phrase = raw_phrase
        .trim()
        .trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\''
                    | '`'
                    | '*'
                    | '#'
                    | ':'
                    | ';'
                    | ','
                    | '.'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '—'
                    | '-'
            )
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if phrase
        .chars()
        .all(|c| !c.is_alphabetic() || !c.is_lowercase())
    {
        title_case_concept_phrase(&phrase)
    } else {
        phrase
    }
}

fn remove_leading_the(phrase: &str) -> String {
    phrase
        .strip_prefix("The ")
        .or_else(|| phrase.strip_prefix("THE "))
        .unwrap_or(phrase)
        .trim()
        .to_string()
}

fn canonical_concept_label(term: &str) -> String {
    term.replace("Kabalistic", "Kabbalistic")
        .replace("Kabalah", "Kabbalah")
        .replace("Chocma", "Chokmah")
        .replace("Chocmah", "Chokmah")
        .replace("Chochma", "Chokmah")
}

fn title_case_concept_phrase(phrase: &str) -> String {
    phrase
        .split_whitespace()
        .enumerate()
        .map(|(index, word)| {
            let lower = word.to_ascii_lowercase();
            if index > 0 && TITLE_CONNECTOR_WORDS.contains(&lower.as_str()) {
                lower
            } else {
                let mut chars = lower.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_concept_word(raw: &str) -> String {
    raw.trim_matches(|c: char| {
        !(c.is_alphanumeric() || c == '\'' || c == '-' || c == '_' || c == '’')
    })
    .trim_matches(|c: char| c == '\'' || c == '’' || c == '-' || c == '_')
    .to_string()
}

fn concept_word_has_letter(word: &str) -> bool {
    word.chars().any(|c| c.is_alphabetic())
}

fn is_all_caps_concept_word(word: &str) -> bool {
    let letters = word
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect::<Vec<_>>();
    !letters.is_empty() && letters.iter().all(|c| c.is_uppercase())
}

fn is_titlecase_concept_word(word: &str) -> bool {
    let mut chars = word.chars().filter(|c| c.is_alphabetic());
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_uppercase() && chars.any(|c| c.is_lowercase())
}

fn significant_seed_words(phrase: &str) -> usize {
    phrase
        .split_whitespace()
        .map(clean_concept_word)
        .filter(|word| {
            let lower = word.to_ascii_lowercase();
            concept_word_has_letter(word) && !TITLE_CONNECTOR_WORDS.contains(&lower.as_str())
        })
        .count()
}

fn strip_definition_line_prefix(line: &str) -> &str {
    let mut trimmed = line.trim_start();
    loop {
        let next = trimmed
            .strip_prefix('>')
            .map(str::trim_start)
            .or_else(|| trimmed.strip_prefix("- ").map(str::trim_start))
            .or_else(|| trimmed.strip_prefix("* ").map(str::trim_start))
            .or_else(|| trimmed.strip_prefix("+ ").map(str::trim_start))
            .or_else(|| trimmed.strip_prefix('•').map(str::trim_start));
        if let Some(next) = next {
            trimmed = next;
            continue;
        }

        if let Some((number, rest)) = trimmed.split_once(". ") {
            if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
                trimmed = rest.trim_start();
                continue;
            }
        }

        return trimmed;
    }
}

fn markdown_bold_lead_phrase(line: &str) -> Option<String> {
    let trimmed = strip_definition_line_prefix(line);
    let body = trimmed.strip_prefix("**")?;
    let close = body.find("**")?;
    let phrase = body[..close].trim();
    let rest = body[close + 2..].trim_start();
    if rest.is_empty() || matches!(rest.chars().next(), Some('—' | '–' | '-' | ':')) {
        Some(phrase.to_string())
    } else {
        None
    }
}

fn delimited_definition_lead_phrase(line: &str) -> Option<String> {
    let trimmed = strip_definition_line_prefix(line);
    if trimmed.is_empty()
        || trimmed.starts_with('|')
        || trimmed.starts_with("![")
        || trimmed.starts_with('[')
        || trimmed.contains("://")
    {
        return None;
    }

    let delimiter_index = [" — ", " – ", " - ", ":", "："]
        .iter()
        .filter_map(|delimiter| trimmed.find(delimiter))
        .min()?;
    if delimiter_index == 0 || delimiter_index > 90 {
        return None;
    }

    Some(trimmed[..delimiter_index].trim().to_string())
}

fn standalone_definition_heading(line: &str) -> Option<String> {
    let trimmed = strip_definition_line_prefix(line).trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > 90
        || trimmed.starts_with('|')
        || trimmed.starts_with("![")
        || trimmed.starts_with('[')
        || trimmed.contains("://")
        || matches!(trimmed.chars().next_back(), Some('.' | '!' | '?'))
    {
        return None;
    }

    let words = trimmed
        .split_whitespace()
        .map(clean_concept_word)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() || words.len() > 5 {
        return None;
    }

    let first_lower = words[0].to_ascii_lowercase();
    if COMMON_SENTENCE_START_WORDS.contains(&first_lower.as_str()) {
        return None;
    }

    let has_named_word = words
        .iter()
        .any(|word| is_titlecase_concept_word(word) || is_all_caps_concept_word(word));
    has_named_word.then(|| trimmed.to_string())
}

fn definition_lead_phrases(content: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    for line in content.lines() {
        if let Some(phrase) = markdown_bold_lead_phrase(line)
            .or_else(|| delimited_definition_lead_phrase(line))
            .or_else(|| standalone_definition_heading(line))
        {
            phrases.push(phrase);
        }
    }
    phrases
}

fn leading_heading_phrases(content: &str) -> Vec<String> {
    let trimmed = content
        .trim()
        .trim_start_matches('#')
        .trim_start_matches(|c: char| c == '-' || c == '*' || c == '•' || c.is_whitespace());
    let mut words = Vec::new();
    let raw_words = trimmed.split_whitespace().take(10).collect::<Vec<_>>();
    for (index, raw) in raw_words.iter().enumerate() {
        let word = clean_concept_word(raw);
        if word.is_empty() {
            break;
        }
        let next_word = raw_words
            .get(index + 1)
            .map(|next| clean_concept_word(next));
        if is_quantity_unit_token(&word)
            || is_measurement_unit_token(&word)
            || (is_numeric_measurement_token(&word)
                && next_word.as_deref().is_some_and(is_measurement_unit_token))
        {
            break;
        }
        if is_all_caps_concept_word(&word) || word.chars().all(|c| c.is_ascii_digit()) {
            words.push(word);
        } else {
            break;
        }
    }

    if significant_seed_words(&words.join(" ")) >= 1 {
        vec![words.join(" ")]
    } else {
        Vec::new()
    }
}

fn titlecase_concept_phrases(content: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut connectors: Vec<String> = Vec::new();
    let mut single_counts: HashMap<String, usize> = HashMap::new();

    let flush =
        |current: &mut Vec<String>, connectors: &mut Vec<String>, phrases: &mut Vec<String>| {
            if significant_seed_words(&current.join(" ")) >= 2 {
                let phrase = current.join(" ");
                if (4..=90).contains(&phrase.chars().count()) {
                    phrases.push(phrase);
                }
            }
            current.clear();
            connectors.clear();
        };

    for raw in content.split_whitespace() {
        let word = clean_concept_word(raw);
        if word.is_empty() {
            flush(&mut current, &mut connectors, &mut phrases);
            continue;
        }
        if is_measurement_word(&word) {
            flush(&mut current, &mut connectors, &mut phrases);
            continue;
        }
        let lower = word.to_ascii_lowercase();
        let is_concept_word = is_titlecase_concept_word(&word) || is_all_caps_concept_word(&word);
        if is_concept_word {
            if !current.is_empty() && COMMON_SENTENCE_START_WORDS.contains(&lower.as_str()) {
                flush(&mut current, &mut connectors, &mut phrases);
            }
            if !COMMON_SENTENCE_START_WORDS.contains(&lower.as_str())
                && !low_signal_concept_edge_term(&lower)
                && concept_word_has_letter(&word)
            {
                *single_counts.entry(word.clone()).or_insert(0) += 1;
            }
            if current.is_empty() {
                current.push(word);
            } else {
                current.append(&mut connectors);
                current.push(word);
            }
        } else if !current.is_empty() && TITLE_CONNECTOR_WORDS.contains(&lower.as_str()) {
            connectors.push(word);
        } else {
            flush(&mut current, &mut connectors, &mut phrases);
        }
    }
    flush(&mut current, &mut connectors, &mut phrases);

    for (word, count) in single_counts {
        if count >= 2 || is_all_caps_concept_word(&word) {
            phrases.push(word);
        }
    }

    phrases
}

const QUANTITY_UNITS: &[&str] = &[
    "mcg", "ug", "mg", "kg", "g", "iu", "ml", "l", "oz", "lb", "lbs", "mm", "cm", "km", "m", "bpm",
    "hz", "khz", "mhz", "ghz", "kb", "mb", "gb", "tb",
];

fn normalize_measurement_token(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\''
                    | '`'
                    | '*'
                    | '#'
                    | ':'
                    | ';'
                    | ','
                    | '.'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
            )
        })
        .replace(',', "")
        .replace(['\u{00B5}', '\u{03BC}'], "u")
        .to_ascii_lowercase()
}

fn is_numeric_measurement_token(raw: &str) -> bool {
    let token = normalize_measurement_token(raw);
    let token = token
        .trim_start_matches(|c| c == '+' || c == '-')
        .trim_end_matches('%');
    let mut digits = 0usize;
    let mut dots = 0usize;
    for ch in token.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
        } else if ch == '.' {
            dots += 1;
            if dots > 1 {
                return false;
            }
        } else {
            return false;
        }
    }
    digits > 0
}

fn is_measurement_unit_token(raw: &str) -> bool {
    let token = normalize_measurement_token(raw);
    QUANTITY_UNITS.contains(&token.as_str())
}

fn is_quantity_unit_token(raw: &str) -> bool {
    let token = normalize_measurement_token(raw);
    if token.is_empty() {
        return false;
    }
    for unit in QUANTITY_UNITS {
        if let Some(number) = token.strip_suffix(unit) {
            if is_numeric_measurement_token(number) {
                return true;
            }
        }
    }
    false
}

fn is_measurement_word(raw: &str) -> bool {
    is_quantity_unit_token(raw)
        || is_numeric_measurement_token(raw)
        || is_measurement_unit_token(raw)
}

fn is_date_like_concept(text: &str) -> bool {
    fn all_digits(part: &str) -> bool {
        !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit())
    }

    let trimmed = text.trim();
    let dash_parts = trimmed.split('-').collect::<Vec<_>>();
    if dash_parts.len() == 3
        && dash_parts[0].len() == 4
        && (1..=2).contains(&dash_parts[1].len())
        && (1..=2).contains(&dash_parts[2].len())
        && dash_parts.iter().all(|part| all_digits(part))
    {
        return true;
    }

    let slash_parts = trimmed.split('/').collect::<Vec<_>>();
    slash_parts.len() == 3
        && slash_parts.iter().all(|part| all_digits(part))
        && slash_parts.iter().all(|part| part.len() <= 4)
}

fn contains_measurement_noise(text: &str) -> bool {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    if tokens.iter().any(|token| is_quantity_unit_token(token)) {
        return true;
    }
    tokens
        .windows(2)
        .any(|pair| is_numeric_measurement_token(pair[0]) && is_measurement_unit_token(pair[1]))
}

fn selected_concept_edge_chunks(chunks: &[ConceptEdgeTextChunk]) -> Vec<ConceptEdgeTextChunk> {
    if chunks.len() <= CONCEPT_EDGE_MAX_CHUNKS_PER_RUN {
        return chunks.to_vec();
    }

    if CONCEPT_EDGE_MAX_CHUNKS_PER_RUN <= 1 {
        return chunks.first().cloned().into_iter().collect();
    }

    let last = chunks.len() - 1;
    let denominator = CONCEPT_EDGE_MAX_CHUNKS_PER_RUN - 1;
    let mut selected = Vec::with_capacity(CONCEPT_EDGE_MAX_CHUNKS_PER_RUN);
    let mut seen = HashSet::new();
    for i in 0..CONCEPT_EDGE_MAX_CHUNKS_PER_RUN {
        let index = (i * last + denominator / 2) / denominator;
        if seen.insert(index) {
            selected.push(chunks[index].clone());
        }
    }
    selected
}

fn low_signal_concept_edge_term(term: &str) -> bool {
    let normalized = term
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if LOW_SIGNAL_CONCEPT_EDGE_TERMS.contains(&normalized.as_str()) {
        return true;
    }

    let mut words = normalized
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|word| !word.is_empty());
    let Some(word) = words.next() else {
        return true;
    };
    words.next().is_none()
        && LOW_SIGNAL_CONCEPT_EDGE_TERMS
            .contains(&word.trim_matches('_').to_ascii_lowercase().as_str())
}

fn meaningful_concept_edge_tag(tag: &TagTerm) -> bool {
    let term = tag.term.trim();
    if term.chars().count() < 3 || !term.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    let label = tag.label().trim();
    if label.chars().count() < 3 || !label.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    if low_signal_concept_edge_term(label) {
        return false;
    }
    if low_signal_concept_edge_term(term) && label.eq_ignore_ascii_case(term) {
        return false;
    }
    if is_date_like_concept(label)
        || is_date_like_concept(term)
        || contains_measurement_noise(label)
        || contains_measurement_noise(term)
    {
        return false;
    }

    true
}

fn concept_edge_tag_word_count(text: &str) -> usize {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|word| !word.is_empty())
        .count()
}

fn concept_edge_tag_specificity_score(tag: &TagTerm) -> usize {
    let term = tag.term.trim();
    let label = tag.label().trim();
    let term_words = concept_edge_tag_word_count(term);
    let label_words = concept_edge_tag_word_count(label);
    let qualified_bonus = usize::from(!label.eq_ignore_ascii_case(term)) * 8;
    let multiword_bonus = label_words.saturating_sub(1).min(5) * 4;
    let term_multiword_bonus = term_words.saturating_sub(1).min(4) * 2;
    let length_bonus = (label.chars().count() / 12).min(4);

    qualified_bonus + multiword_bonus + term_multiword_bonus + length_bonus
}

fn concept_edge_tags(tags_by_chunk: &[Vec<TagTerm>]) -> Vec<TagTerm> {
    let mut tags = Vec::new();
    for chunk_tags in tags_by_chunk {
        for tag in chunk_tags {
            if !meaningful_concept_edge_tag(tag) {
                continue;
            }
            tags.push(tag.clone());
        }
    }
    tags
}

fn concept_edge_link_key(label: &str) -> String {
    label.trim().to_ascii_lowercase()
}

fn wiki_page_link_keys(content: &str) -> HashSet<String> {
    grafium_core::parser::extract_links(content)
        .into_iter()
        .filter_map(|link| match link {
            ExtractedLink::Page(title) => Some(concept_edge_link_key(&title)),
            _ => None,
        })
        .filter(|key| !key.is_empty())
        .collect()
}

fn tag_already_linked(link_keys: &HashSet<String>, tag: &TagTerm) -> bool {
    let term = concept_edge_link_key(&tag.term);
    let label = concept_edge_link_key(tag.label());
    if term.is_empty() {
        return label.is_empty() || link_keys.contains(&label);
    }
    if !label.is_empty() && term != label {
        return link_keys.contains(&term);
    }
    link_keys.contains(&term)
}

fn concept_edge_tag_hit_key(tag: &TagTerm) -> String {
    format!(
        "{}\0{}",
        concept_edge_link_key(&tag.term),
        concept_edge_link_key(tag.label())
    )
}

fn existing_concept_edge_link_keys(blocks: &[Block]) -> HashSet<String> {
    let mut keys = HashSet::new();
    for block in blocks {
        keys.extend(wiki_page_link_keys(&block.content));
    }
    keys
}

#[derive(Debug, Clone)]
struct ConceptEdgeTagHit {
    tag: TagTerm,
    count: usize,
    first_seen: usize,
    specificity: usize,
}

fn concept_edge_hit_score(hit: &ConceptEdgeTagHit) -> usize {
    hit.count.min(8) * 16 + hit.specificity
}

fn unlinked_concept_edge_tags_from_chunks(
    tags_by_chunk: &[Vec<TagTerm>],
    blocks: &[Block],
) -> (Vec<TagTerm>, bool) {
    let existing_link_keys = existing_concept_edge_link_keys(blocks);
    let mut hits: HashMap<String, ConceptEdgeTagHit> = HashMap::new();
    let mut first_seen = 0usize;

    for tag in concept_edge_tags(tags_by_chunk) {
        if tag_already_linked(&existing_link_keys, &tag) {
            continue;
        }
        let key = concept_edge_tag_hit_key(&tag);
        if key.is_empty() {
            continue;
        }
        hits.entry(key)
            .and_modify(|hit| hit.count += 1)
            .or_insert_with(|| {
                let hit = ConceptEdgeTagHit {
                    specificity: concept_edge_tag_specificity_score(&tag),
                    tag,
                    count: 1,
                    first_seen,
                };
                first_seen += 1;
                hit
            });
    }

    let mut ranked = hits.into_values().collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        concept_edge_hit_score(b)
            .cmp(&concept_edge_hit_score(a))
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| b.specificity.cmp(&a.specificity))
            .then_with(|| {
                b.tag
                    .label()
                    .chars()
                    .count()
                    .cmp(&a.tag.label().chars().count())
            })
            .then_with(|| a.first_seen.cmp(&b.first_seen))
    });

    let limit_reached = ranked.len() > CONCEPT_EDGE_MAX_TAGS_PER_RUN;
    let tags = ranked
        .into_iter()
        .take(CONCEPT_EDGE_MAX_TAGS_PER_RUN)
        .map(|hit| hit.tag)
        .collect();
    (tags, limit_reached)
}

struct PageBatchCursor {
    batch_size: i64,
    offset: i64,
    finished: bool,
}

impl PageBatchCursor {
    fn new(batch_size: i64) -> Self {
        Self {
            batch_size,
            offset: 0,
            finished: false,
        }
    }

    fn next_batch<T, E>(
        &mut self,
        fetch: impl FnOnce(i64, i64) -> Result<Vec<T>, E>,
    ) -> Result<Option<Vec<T>>, E> {
        if self.finished {
            return Ok(None);
        }

        let batch = fetch(self.batch_size, self.offset)?;
        if batch.is_empty() {
            self.finished = true;
            return Ok(None);
        }

        self.offset += batch.len() as i64;
        Ok(Some(batch))
    }
}

// ─── Configuration ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct AiConfigPayload {
    pub enabled: bool,
    pub mode: String,
    pub local_provider: Option<String>,
    pub local_base_url: Option<String>,
    pub local_api_key: Option<String>,
    pub local_model_path: Option<String>,
    /// GGUF embedding model file for the Embedded (llama.cpp) local
    /// provider — resolved against `ModelKind::Embedding` rather than
    /// `ModelKind::Llm`, so an "Embedded" provider can do semantic search /
    /// "Research this page" on its own. See `LocalEmbeddingSettings`.
    pub local_embedding_model_path: Option<String>,
    /// Shared directory to search for local model files (embedded LLM
    /// GGUF, and in future Whisper) instead of Grafium's own managed
    /// `<data_dir>/models` folder — lets a user point at e.g.
    /// `~/Documents/models` shared with other apps. `None`/empty keeps the
    /// default.
    pub local_models_dir: Option<String>,
    pub llm_model: Option<String>,
    pub embedding_model: Option<String>,
    pub cloud_provider: Option<String>,
    pub cloud_base_url: Option<String>,
    pub cloud_llm_model: Option<String>,
    pub cloud_api_key: Option<String>,
    pub cloud_embedding_provider: Option<String>,
    pub cloud_embedding_base_url: Option<String>,
    pub cloud_embedding_api_key: Option<String>,
    pub cloud_embedding_model: Option<String>,
    pub concept_edge_prompt: Option<String>,
}

#[tauri::command]
pub async fn ai_get_config(state: State<'_, KnowledgeState>) -> Result<serde_json::Value, String> {
    let guard = state.engine.read().await;
    if let Some(engine) = guard.as_ref() {
        serde_json::to_value(engine.config()).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::to_value(AiConfig::default()).unwrap())
    }
}

#[tauri::command]
pub fn ai_default_concept_edge_prompt() -> String {
    grafium_core::ai::references::DEFAULT_CONCEPT_EDGE_PROMPT.to_string()
}

#[tauri::command]
pub async fn ai_set_config(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    payload: AiConfigPayload,
) -> Result<(), String> {
    fn parse_provider(name: &str) -> ProviderType {
        match name {
            "anthropic" => ProviderType::Anthropic,
            "openai_compatible" | "openaicompatible" | "vllm" => ProviderType::OpenAiCompatible,
            "ollama" => ProviderType::Ollama,
            "huggingface" | "huggingface_local" => ProviderType::HuggingFace,
            _ => ProviderType::OpenAi,
        }
    }

    let mode = match payload.mode.as_str() {
        "cloud" => AiMode::Cloud,
        "hybrid" => AiMode::Hybrid,
        _ => AiMode::Local,
    };

    let local_provider = payload
        .local_provider
        .as_deref()
        .map(parse_provider)
        .unwrap_or(ProviderType::OpenAiCompatible);

    let local_base_url_default = match local_provider {
        ProviderType::Ollama => "http://localhost:11434",
        _ => "http://localhost:8000/v1",
    };

    let local = Some(LocalConfig {
        provider: local_provider,
        base_url: payload
            .local_base_url
            .unwrap_or_else(|| local_base_url_default.to_string()),
        api_key: payload.local_api_key,
        models_dir: payload
            .local_models_dir
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from),
        local_llm: LocalLlmSettings {
            model_ref: LocalModelRef {
                model: payload.local_model_path,
            },
            ..Default::default()
        },
        local_embedding: LocalEmbeddingSettings {
            model_ref: LocalModelRef {
                model: payload.local_embedding_model_path,
            },
        },
        llm_model: payload.llm_model.unwrap_or_else(|| "llama3.2".to_string()),
        embedding_model: payload
            .embedding_model
            .unwrap_or_else(|| "nomic-embed-text".to_string()),
    });

    let cloud = if let (Some(provider), Some(model)) =
        (&payload.cloud_provider, &payload.cloud_llm_model)
    {
        let provider_type = parse_provider(provider);
        let embedding_provider = payload
            .cloud_embedding_provider
            .as_deref()
            .map(parse_provider)
            .unwrap_or_else(|| {
                if provider_type == ProviderType::OpenAi {
                    ProviderType::OpenAi
                } else {
                    ProviderType::OpenAiCompatible
                }
            });

        Some(CloudConfig {
            llm_provider: provider_type.clone(),
            llm_model: model.clone(),
            llm_api_key: payload.cloud_api_key.clone(),
            llm_base_url: payload.cloud_base_url,
            embedding_provider,
            embedding_model: payload
                .cloud_embedding_model
                .unwrap_or_else(|| "text-embedding-3-small".to_string()),
            embedding_api_key: payload.cloud_embedding_api_key.or(payload.cloud_api_key),
            embedding_base_url: payload.cloud_embedding_base_url,
        })
    } else {
        None
    };
    let references = ReferenceConfig {
        concept_edge_prompt: payload
            .concept_edge_prompt
            .map(|prompt| prompt.trim().to_string())
            .filter(|prompt| !prompt.is_empty()),
        ..AiConfig::default().references
    };

    let config = AiConfig {
        enabled: payload.enabled,
        mode,
        local,
        cloud,
        references,
        ..AiConfig::default()
    };

    // Save config to disk.
    let config_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("knowledge");
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_path = config_dir.join("ai_config.json");
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, json).map_err(|e| e.to_string())?;

    // Reconfigure the engine.
    let mut guard = state.engine.write().await;
    if let Some(engine) = guard.as_mut() {
        engine.reconfigure(config).map_err(|e| e.to_string())?;
    } else {
        let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let engine = KnowledgeEngine::new_with_models_root(&config_dir, config, &app_data_dir)
            .map_err(|e| e.to_string())?;
        *guard = Some(engine);
    }

    Ok(())
}

// ─── Health ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_health_check(state: State<'_, KnowledgeState>) -> Result<HealthStatus, String> {
    let guard = state.engine.read().await;
    if let Some(engine) = guard.as_ref() {
        engine.health_check().await.map_err(|e| e.to_string())
    } else {
        Ok(HealthStatus {
            enabled: false,
            llm_available: false,
            embedder_available: false,
            vector_store_available: false,
            vector_count: 0,
            mode: AiMode::Local,
            // No engine at all is a different situation from an engine whose
            // model failed to load, and must not be reported as the latter.
            llm_load_error: None,
        })
    }
}

// ─── Indexing ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_index_status(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
) -> Result<IndexStatus, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let graph_id = snapshot.root_dir.to_string_lossy().to_string();
    let graph = crate::open_graph_snapshot(&snapshot)?;

    engine
        .index_status(&graph.db, &graph_id)
        .await
        .map_err(|e| e.to_string())
}

/// Reload the local chat model forcing full GPU offload. Backs Chat's "Retry
/// on GPU" action for the case where the free-VRAM heuristic landed on CPU
/// because VRAM was transiently busy at startup. Returns the refreshed
/// accelerator status so the UI can update its banner immediately.
#[tauri::command]
pub async fn ai_retry_llm_on_gpu(
    state: State<'_, KnowledgeState>,
) -> Result<Option<grafium_core::ai::traits::AcceleratorStatus>, String> {
    let mut guard = state.engine.write().await;
    let engine = guard
        .as_mut()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;
    engine.retry_llm_on_gpu().map_err(|e| e.to_string())?;
    Ok(engine.llm_accelerator_status())
}

#[tauri::command]
pub async fn ai_index_page(
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    page_id: String,
) -> Result<usize, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

    let (page, blocks, graph_id) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        let page = graph
            .db
            .get_page_by_id(&page_id)
            .map_err(|e| e.to_string())?;
        let blocks = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string())?;
        let graph_id = graph.root_dir.to_string_lossy().to_string();
        (page, blocks, graph_id)
    };

    engine
        .index_page(&page, &blocks, &graph_id)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexAllResult {
    /// Chunks embedded/updated across all pages.
    pub indexed_chunks: usize,
    /// Pages successfully processed (indexed or already up to date).
    pub pages_processed: usize,
    /// Pages that failed to index (errors were logged, not fatal).
    pub pages_failed: usize,
}

#[tauri::command]
pub async fn ai_index_all_pages(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
) -> Result<IndexAllResult, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let graph_id = snapshot.root_dir.to_string_lossy().to_string();
    let graph = crate::open_graph_snapshot(&snapshot)?;

    // Recover the hash cache from already-stored vectors so a restart doesn't
    // needlessly re-embed unchanged content.
    if let Err(e) = engine.restore_hash_cache(&graph_id).await {
        eprintln!("Failed to restore embedding hash cache: {e}");
    }

    let mut cursor = PageBatchCursor::new(AI_INDEX_BATCH_SIZE);
    let mut indexed_chunks = 0;
    let mut pages_processed = 0;
    let mut pages_failed = 0;

    while let Some(pages) = cursor.next_batch(|limit, offset| {
        graph
            .db
            .list_pages_window(limit, offset, false, PageKindFilter::All)
            .map_err(|e| e.to_string())
    })? {
        let mut pages_and_blocks = Vec::with_capacity(pages.len());
        for page in pages {
            let blocks = graph
                .db
                .list_blocks_for_page(&page.id)
                .map_err(|e| e.to_string())?;
            pages_and_blocks.push((page, blocks));
        }

        for (page, blocks) in &pages_and_blocks {
            match engine.index_page(page, blocks, &graph_id).await {
                Ok(count) => {
                    indexed_chunks += count;
                    pages_processed += 1;
                }
                Err(e) => {
                    pages_failed += 1;
                    eprintln!("Failed to index page '{}': {}", page.title, e);
                }
            }
        }
    }

    Ok(IndexAllResult {
        indexed_chunks,
        pages_processed,
        pages_failed,
    })
}

// ─── Search ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_search(
    state: State<'_, KnowledgeState>,
    query: String,
    top_k: Option<usize>,
    graph_id: Option<String>,
) -> Result<Vec<SearchResult>, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

    engine
        .search(&query, top_k.unwrap_or(10), graph_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ─── References ──────────────────────────────────────────────────────────────

struct SummaryOperation {
    id: String,
    flag: Arc<AtomicBool>,
    registry: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl SummaryOperation {
    fn register(state: &KnowledgeState, operation_id: Option<String>) -> Result<Self, String> {
        let id = operation_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if id.trim().is_empty() {
            return Err("AI operation ID cannot be empty".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        let mut registry = state.cancels.lock().map_err(|error| error.to_string())?;
        if registry.contains_key(&id) {
            return Err("An AI operation with this ID is already running".into());
        }
        registry.insert(id.clone(), flag.clone());
        Ok(Self { id, flag, registry: state.cancels.clone() })
    }

    async fn run<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, grafium_core::CoreError>>,
    ) -> Result<T, String> {
        let cancelled = async {
            while !self.flag.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        };
        tokio::select! {
            biased;
            _ = cancelled => Err("AI operation cancelled".into()),
            result = future => {
                if self.flag.load(Ordering::Acquire) {
                    Err("AI operation cancelled".into())
                } else {
                    result.map_err(|error| error.to_string())
                }
            }
        }
    }
}

impl Drop for SummaryOperation {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            if registry.get(&self.id).is_some_and(|flag| Arc::ptr_eq(flag, &self.flag)) {
                registry.remove(&self.id);
            }
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn ai_cancel_operation(
    state: State<'_, KnowledgeState>,
    operation_id: String,
) -> Result<(), String> {
    let registered = {
        let registry = state.cancels.lock().map_err(|error| error.to_string())?;
        if let Some(flag) = registry.get(&operation_id) {
            flag.store(true, Ordering::Release);
            Some(flag.clone())
        } else {
            None
        }
    };
    // An old toast must never abort an unrelated, newly started generation.
    if let Some(flag) = registered {
        let engine = state.engine.read().await;
        let registry = state.cancels.lock().map_err(|error| error.to_string())?;
        if registry.get(&operation_id).is_some_and(|current| Arc::ptr_eq(current, &flag)) {
            if let Some(llm) = engine.as_ref().and_then(|engine| engine.llm_provider()) {
                llm.abort_in_flight();
            }
        }
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn ai_generate_references(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    page_id: String,
    operation_id: Option<String>,
    graph_path: Option<String>,
    expected_blocks: Option<Vec<Block>>,
) -> Result<PageReferencesMeta, String> {
    let operation = SummaryOperation::register(&state, operation_id)?;
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err("AI summary is not ready — configure a language model in Settings".into());
    }

    let (page_title, blocks_data, graph_id, source_hash) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        let page = graph
            .db
            .get_page_by_id(&page_id)
            .map_err(|e| e.to_string())?;
        let blocks = graph
            .validate_summary_source(&page_id, graph_path.as_deref(), expected_blocks.as_deref())
            .map_err(|e| e.to_string())?;
        let graph_id = graph.root_dir.to_string_lossy().to_string();

        let originals: Vec<_> = blocks.iter().map(|b| (b.id.clone(), b.content.clone())).collect();
        let source_hash = grafium_core::graph::Graph::summary_source_hash(&originals);
        let blocks_data = reference_source_projection(&blocks);

        (page.title, blocks_data, graph_id, source_hash)
    };

    // "Research this page" can take minutes on a local CPU-bound model, so
    // the UI needs live status instead of an unexplained "Analyzing..."
    // that reads like a hang.
    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-reference-progress", message);
    };

    if engine.is_ready() {
        let mut result = operation.run(engine.generate_references(
            &page_id,
            &page_title,
            &blocks_data,
            &graph_id,
            &mut emit_progress,
        )).await?;
        result.content_hash = source_hash;
        return Ok(result);
    }
    // Summarization itself is LLM-only; optional cross-reference retrieval
    // must not disable it when embeddings are absent.
    let text = blocks_data.iter().map(|(_, content)| content.as_str()).collect::<Vec<_>>().join("\n\n");
    let summary = operation.run(engine.summarize_text(&page_title, &text, &mut emit_progress)).await?;
    Ok(PageReferencesMeta {
        page_id,
        generated_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?.as_millis() as i64,
        content_hash: source_hash,
        reference_count: 0,
        references: Vec::new(),
        summary: Some(summary),
        summary_error: None,
    })
}

fn reference_source_projection(blocks: &[Block]) -> Vec<(String, String)> {
    grafium_core::knowledge::source_projection::project_source_blocks(blocks)
        .into_iter().map(|block| (block.id, block.content)).collect()
}

/// Analyzes arbitrary selected text (one or more selected blocks'
/// concatenated content) and returns a short AI summary + hashtag-style
/// topic tags — the same shape/prompt used for "Analyze this Page" and
/// media-import summaries, just applied to a text selection instead of a
/// whole page. The caller (`PageContent.svelte`'s "Analyze Selected"
/// action) inserts the result as a new block right after the selection.
#[tauri::command(rename_all = "camelCase")]
pub async fn ai_summarize_selection(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    text: String,
    title: Option<String>,
    operation_id: Option<String>,
) -> Result<grafium_core::ai::references::PageSummary, String> {
    let operation = SummaryOperation::register(&state, operation_id)?;
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge Engine."
                .to_string(),
        );
    }

    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-selection-summary-progress", message);
    };
    let title = title.unwrap_or_else(|| "Selected text".to_string());

    operation.run(engine.summarize_text(&title, &text, &mut emit_progress)).await
}

fn concept_edge_job_title(page_title: &str) -> String {
    let title = page_title.trim();
    let short = if title.chars().count() > 48 {
        let prefix = title.chars().take(45).collect::<String>();
        format!("{prefix}...")
    } else {
        title.to_string()
    };
    format!("Find concept edges: {short}")
}

fn concept_edge_job_details(result: &ConceptEdgesResult) -> String {
    let mut details = format!(
        "Concepts found: {}\nSuggestions added: {}\nText chunks analyzed: {}/{}",
        result.tag_count, result.suggested_edges, result.chunks_analyzed, result.chunks_total
    );
    if result.chunks_failed > 0 {
        details.push_str(&format!("\nText chunks failed: {}", result.chunks_failed));
    }
    if result.chunks_analyzed < result.chunks_total {
        details.push_str(
            "\nAnalyzed representative chunks across the page to stay within the model context.",
        );
    }
    if result.limit_reached {
        details.push_str("\nStopped at the safe per-run suggestion limit.");
    }
    if !result.tags.is_empty() {
        details.push_str("\n\nConcepts:");
        for tag in &result.tags {
            details.push_str("\n- ");
            details.push_str(tag.label());
        }
    }
    details
}

/// Starts a background job that chunks page text into context-safe pieces, asks
/// the LLM for semantic concept tags, and stores them as pending link
/// suggestions for the user to review. The job returns immediately so huge
/// imported books don't freeze the page that launched it.
#[tauri::command(rename_all = "camelCase")]
pub async fn ai_create_concept_edges(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    jobs: State<'_, JobsState>,
    page_id: String,
    resolve_uncertain: Option<bool>,
) -> Result<String, String> {
    let (page_title, blocks, source_root) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        let page = graph
            .db
            .get_page_by_id(&page_id)
            .map_err(|e| e.to_string())?;
        let blocks = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string())?;
        (page.title, blocks, graph.root_dir.clone())
    };
    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    if snapshot.root_dir != source_root {
        return Err("The active graph changed; run concept discovery again.".into());
    }
    let active_graph = app_state.graph.clone();
    let link = JobLink {
        page_id: page_id.clone(),
        page_title: Some(page_title.clone()),
        label: page_title.clone(),
    };
    let handle = jobs.registry.start_with_link(
        app.clone(),
        "ai_concept_edges",
        concept_edge_job_title(&page_title),
        true,
        Some(link.clone()),
    )?;
    let job_id = handle.id().to_string();
    let engine = state.engine.clone();

    tauri::async_runtime::spawn(async move {
        handle.progress(0, 3, "Preparing text chunks...");
        let all_chunks = concept_edge_text_chunks(&blocks);
        if all_chunks.is_empty() {
            handle.succeeded_with_details(
                "No readable text found for concept edge discovery",
                Some(link),
                Some("Image-only and import-metadata blocks are skipped."),
            );
            return;
        }
        if handle.is_cancelled() {
            handle.cancelled();
            return;
        }

        let selected_chunks = selected_concept_edge_chunks(&all_chunks);
        let selected_chunk_count = selected_chunks.len();
        let total_steps = selected_chunk_count + 2;
        let seed_tags = document_concept_edge_seed_tags(&blocks);
        let mut tag_batches = if seed_tags.is_empty() {
            Vec::new()
        } else {
            vec![seed_tags]
        };
        let mut chunks_analyzed = 0usize;
        let mut chunk_errors = Vec::new();
        {
            let guard = engine.read().await;
            let Some(engine) = guard.as_ref() else {
                handle.failed("Knowledge engine not initialized");
                return;
            };

            if !engine.is_llm_ready() {
                handle.failed(
                    "AI engine not ready — check configuration in Settings → AI / Knowledge Engine.",
                );
                return;
            }

            for (chunk_index, chunk) in selected_chunks.iter().enumerate() {
                if handle.is_cancelled() {
                    handle.cancelled();
                    return;
                }
                let step = chunk_index + 1;
                handle.progress(
                    step,
                    total_steps,
                    format!(
                        "Extracting graph concepts from text chunk {}/{} (source chunk {}/{}, {} block{})...",
                        step,
                        selected_chunk_count,
                        chunk.source_index + 1,
                        all_chunks.len(),
                        chunk.block_count,
                        if chunk.block_count == 1 { "" } else { "s" }
                    ),
                );

                let progress_handle = handle.clone();
                let mut emit_progress = move |message: &str| {
                    progress_handle.progress(
                        step,
                        total_steps,
                        format!("Chunk {step}/{selected_chunk_count}: {message}"),
                    );
                };
                let chunk_title = format!(
                    "{page_title} — text chunk {}/{}",
                    chunk.source_index + 1,
                    all_chunks.len()
                );
                let cancel_token = grafium_core::cancel::CancellationToken::new();
                let mut tags_future = Box::pin(engine.concept_edge_tags(
                    &chunk_title,
                    &chunk.text,
                    &mut emit_progress,
                    &cancel_token,
                ));
                let result = loop {
                    tokio::select! {
                        result = &mut tags_future => break result,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                            if handle.is_cancelled() {
                                cancel_token.cancel();
                            }
                        }
                    }
                };
                if handle.is_cancelled() {
                    handle.cancelled();
                    return;
                }
                match result {
                    Ok(tags) => {
                        chunks_analyzed += 1;
                        tag_batches.push(tags);
                    }
                    Err(error) => chunk_errors.push(format!(
                        "Chunk {}/{} failed: {error}",
                        chunk.source_index + 1,
                        all_chunks.len()
                    )),
                }
            }
        }
        if handle.is_cancelled() {
            handle.cancelled();
            return;
        }
        if chunks_analyzed == 0 && tag_batches.is_empty() {
            handle.failed_with_details(
                "Could not discover concept edges",
                Some(if chunk_errors.is_empty() {
                    "The AI did not return a usable concept-edge response for any text chunk."
                        .to_string()
                } else {
                    chunk_errors.join("\n")
                }),
            );
            return;
        }

        let (tags, tag_limit_reached) =
            unlinked_concept_edge_tags_from_chunks(&tag_batches, &blocks);
        if tags.is_empty() {
            handle.succeeded_with_details(
                "No new concept edge suggestions found",
                Some(link),
                Some(if chunk_errors.is_empty() {
                    format!(
                        "Analyzed {} of {} selected text chunk{}; the AI did not return strong unlinked graph concepts for this page.",
                        chunks_analyzed,
                        all_chunks.len(),
                        if all_chunks.len() == 1 { "" } else { "s" }
                    )
                } else {
                    format!(
                        "Analyzed {} of {} text chunk{}; no strong unlinked concepts were found.\n\n{}",
                        chunks_analyzed,
                        all_chunks.len(),
                        if all_chunks.len() == 1 { "" } else { "s" },
                        chunk_errors.join("\n")
                    )
                }),
            );
            return;
        }

        handle.progress(
            selected_chunk_count + 1,
            total_steps,
            format!("Preparing {} concept edge suggestions...", tags.len()),
        );
        let graph_for_resolution = active_graph.clone();
        let source_root_for_resolution = source_root.clone();
        let tags_for_resolution = tags.clone();
        let prepared = tauri::async_runtime::spawn_blocking(move || {
            let graph = graph_for_resolution.lock().map_err(|error| error.to_string())?;
            if graph.root_dir != source_root_for_resolution {
                return Err("The active graph changed; concept discovery was not applied.".to_string());
            }
            let resolutions = graph.db.resolve_tag_terms(&tags_for_resolution)
                .map_err(|error| error.to_string())?;
            let mut descriptions = Vec::new();
            for resolution in resolutions.iter()
                .filter(|resolution| resolution.decision == grafium_core::db::EntityDecision::Ambiguous)
                .take(12)
            {
                descriptions.extend(graph.db.entity_candidate_contexts(resolution)
                    .map_err(|error| error.to_string())?);
            }
            Ok::<_, String>((resolutions, descriptions))
        }).await;
        let (mut resolutions, descriptions) = match prepared {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(error)) => {
                handle.failed(error);
                return;
            }
            Err(error) => {
                handle.failed(format!("Entity shortlist preparation failed: {error}"));
                return;
            }
        };
        let resolution_cancel = grafium_core::cancel::CancellationToken::new();
        let mut identity_errors = Vec::new();
        if resolve_uncertain.unwrap_or(true) {
            let source_blocks = grafium_core::knowledge::source_projection::project_source_blocks(&blocks);
            let guard = engine.read().await;
            let provider = guard.as_ref().and_then(|engine| engine.llm_provider());
            let mut adjudicated = 0usize;
            for resolution in &mut resolutions {
                if resolution.decision != grafium_core::db::EntityDecision::Ambiguous
                    || resolution.candidates.is_empty() || adjudicated >= 12
                {
                    continue;
                }
                if handle.is_cancelled() {
                    resolution_cancel.cancel();
                    handle.cancelled();
                    return;
                }
                adjudicated += 1;
                handle.progress(selected_chunk_count + 1, total_steps,
                    format!("Reviewing bounded identity shortlist {adjudicated}/12..."));
                let needle = resolution.source_phrase.to_lowercase();
                let context = source_blocks.iter()
                    .filter(|block| block.content.to_lowercase().contains(&needle))
                    .take(2)
                    .map(|block| block.content.chars().take(1200).collect::<String>())
                    .collect::<Vec<_>>().join("\n");
                let mut future = Box::pin(grafium_core::db::adjudicate_entity_resolution(
                    resolution, &context, &descriptions, provider, &resolution_cancel));
                let result = loop {
                    tokio::select! {
                        result = &mut future => break result,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                            if handle.is_cancelled() {
                                resolution_cancel.cancel();
                            }
                        }
                    }
                };
                drop(future);
                if handle.is_cancelled() {
                    handle.cancelled();
                    return;
                }
                match result {
                    Ok(resolved) => *resolution = resolved,
                    Err(error) => {
                        resolution.reason = format!("Identity model could not resolve this shortlist: {error}");
                        identity_errors.push(resolution.reason.clone());
                    }
                }
            }
        }
        if handle.is_cancelled() {
            handle.cancelled();
            return;
        }
        let tags_for_insert = tags.clone();
        let page_id_for_insert = page_id.clone();
        let insert_cancel = resolution_cancel.clone();
        let mut insert_future = tauri::async_runtime::spawn_blocking(move || {
            let graph = active_graph.lock().map_err(|error| error.to_string())?;
            if graph.root_dir != source_root {
                return Err("The active graph changed; concept discovery was not applied.".to_string());
            }
            graph
                .db
                .discover_resolved_semantic_concept_candidates(
                    &page_id_for_insert,
                    &tags_for_insert,
                    &resolutions,
                    &blocks,
                    CONCEPT_EDGE_MAX_CANDIDATES_PER_RUN as i64,
                    &insert_cancel,
                )
                .map_err(|error| error.to_string())
        });
        let insert_result = loop {
            tokio::select! {
                result = &mut insert_future => break result,
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    if handle.is_cancelled() {
                        resolution_cancel.cancel();
                    }
                }
            }
        };

        let suggested_edges = match insert_result {
            Ok(Ok(count)) => count,
            Ok(Err(error)) => {
                handle.failed_with_details(
                    "Could not write concept edge suggestions",
                    Some(error.to_string()),
                );
                return;
            }
            Err(error) => {
                handle.failed(format!("Concept edge job failed: {error}"));
                return;
            }
        };
        if handle.is_cancelled() {
            handle.cancelled();
            return;
        }

        let result = ConceptEdgesResult {
            tag_count: tags.len(),
            suggested_edges,
            limit_reached: tag_limit_reached || selected_chunk_count < all_chunks.len(),
            chunks_analyzed,
            chunks_total: all_chunks.len(),
            chunks_failed: chunk_errors.len(),
            tags,
        };
        let message = if suggested_edges > 0 {
            format!(
                "Added {suggested_edges} concept edge suggestion{}",
                if suggested_edges == 1 { "" } else { "s" }
            )
        } else {
            "Found concepts, but none matched unlinked text".to_string()
        };
        let mut details = concept_edge_job_details(&result);
        if !identity_errors.is_empty() {
            details.push_str("\n\nIdentity reviews requiring manual resolution:\n");
            details.push_str(&identity_errors.join("\n"));
        }
        handle.succeeded_with_details(message, Some(link), Some(details));
    });

    Ok(job_id)
}

/// Actually researches `title`/`seed_text` on the open internet: plans
/// search queries, searches the web (a plain HTML scrape of Brave's
/// results page — no paid search API/keys involved), reads the most
/// relevant results, and synthesizes a topic-by-topic summary with inline
/// `[n]` citation markers pointing at real, clickable source URLs. Unlike
/// `ai_generate_references`/`ai_summarize_selection`, this can surface
/// information not already present anywhere in the user's graph, so the
/// result always carries its sources for the user to verify. Works with
/// whatever LLM provider is configured — local (embedded llama.cpp,
/// Ollama) or a remote OpenAI-compatible endpoint (e.g. vLLM reachable
/// over Tailscale/LAN) — since it only needs `engine.is_llm_ready()`, not
/// an embedder/vector store.
#[tauri::command(rename_all = "camelCase")]
pub async fn ai_research_web(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    title: String,
    seed_text: String,
    operation_id: Option<String>,
) -> Result<grafium_core::ai::web_research::WebResearchResult, String> {
    let operation = SummaryOperation::register(&state, operation_id)?;
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge Engine."
                .to_string(),
        );
    }

    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-web-research-progress", message);
    };

    operation.run(engine.research_web(&title, &seed_text, &mut emit_progress)).await
}

/// Wraps the first verbatim, whole-word occurrence of each term found in
/// `content` with `[[wiki-link]]` syntax (optionally substituting a
/// `qualified` disambiguation phrase — see
/// [`grafium_core::parser::TagTerm`]). Thin synchronous wrapper around
/// [`grafium_core::parser::wrap_known_terms_as_links`] — kept as a single
/// shared entry point so "Analyze Selected" and any other AI-tagging
/// caller wrap terms identically instead of re-implementing matching
/// logic per call site.
#[tauri::command(rename_all = "camelCase")]
pub fn text_wrap_known_terms(content: String, terms: Vec<grafium_core::parser::TagTerm>) -> String {
    grafium_core::parser::wrap_known_terms_as_links(&content, &terms)
}

/// Insert one guarded summary tree and return every reversible change.
#[tauri::command(rename_all = "camelCase")]
pub fn ai_insert_page_summary(
    app_state: State<'_, crate::AppState>,
    page_id: String,
    title_answer: Option<String>,
    topics: Vec<grafium_core::ai::references::TopicSummary>,
    after_block_id: Option<String>,
    graph_path: Option<String>,
    expected_blocks: Option<Vec<Block>>,
    wrap_existing: Option<bool>,
) -> Result<AiInsertSummaryResult, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    insert_summary_for_graph(
        &graph, &page_id, title_answer.as_deref(), &topics, after_block_id.as_deref(),
        graph_path.as_deref(), expected_blocks.as_deref(), wrap_existing.unwrap_or(false),
    )
}

fn insert_summary_for_graph(
    graph: &grafium_core::graph::Graph,
    page_id: &str,
    title_answer: Option<&str>,
    topics: &[grafium_core::ai::references::TopicSummary],
    after_block_id: Option<&str>,
    graph_path: Option<&str>,
    expected_blocks: Option<&[Block]>,
    wrap_existing: bool,
) -> Result<AiInsertSummaryResult, String> {
    let before = graph.validate_summary_source(
        page_id, graph_path, expected_blocks,
    ).map_err(|error| error.to_string())?;
    let tags = topics.iter().flat_map(|topic| topic.tags.iter().cloned()).collect::<Vec<_>>();
    let resolutions = graph.db.resolve_tag_terms(&tags).map_err(|error| error.to_string())?;
    let mut links = grafium_core::graph::SummaryLinkPlan::default();
    let mut accepted = Vec::with_capacity(resolutions.len());
    let mut seen_targets = HashSet::new();
    let mut seen_warnings = HashSet::new();
    for resolution in resolutions {
        let warning = if resolution.decision == grafium_core::db::EntityDecision::Ambiguous {
            Some(resolution.reason.clone())
        } else if grafium_core::parser::format_concept_link(&resolution.target_title).is_none() {
            Some("This target cannot be represented as a safe concept link.".into())
        } else if resolution.decision == grafium_core::db::EntityDecision::Reuse && resolution.target_page_id.is_none() {
            Some("The existing target has no stable page identity.".into())
        } else {
            None
        };
        if let Some(reason) = warning {
            if seen_warnings.insert((resolution.source_phrase.clone(), resolution.target_title.clone())) {
                links.unlinked_targets.push(grafium_core::graph::SummaryUnlinkedTarget {
                    source_phrase: resolution.source_phrase,
                    target_title: resolution.target_title,
                    reason,
                });
            }
            accepted.push(None);
            continue;
        }
        if seen_targets.insert(resolution.target_title.clone()) {
            if let Some(page_id) = resolution.target_page_id {
                links.resolved_targets.push(grafium_core::graph::SummaryLinkTarget {
                    page_id, title: resolution.target_title.clone(),
                });
            } else {
                links.new_target_titles.push(resolution.target_title.clone());
            }
        }
        accepted.push(Some(grafium_core::parser::ResolvedLinkTerm {
            source_phrase: resolution.source_phrase,
            target_title: resolution.target_title,
            display: grafium_core::parser::LinkDisplay::ConceptTag,
        }));
    }
    let terms = accepted.iter().flatten().cloned().collect::<Vec<_>>();
    let wrap = |content: &str| grafium_core::parser::links::wrap_resolved_terms_as_links(content, &terms);
    let wrapped_title = title_answer.map(wrap);
    let wrapped_topics = topics.iter().map(|topic| (wrap(&topic.topic), wrap(&topic.summary))).collect::<Vec<_>>();
    let mut offset = 0;
    for (topic, (heading, body)) in topics.iter().zip(&wrapped_topics) {
        let present = grafium_core::parser::extract_links(&format!("{heading}\n\n{body}"));
        let mut seen = HashSet::new();
        let standalone = accepted[offset..offset + topic.tags.len()].iter().flatten()
            .filter(|term| seen.insert(term.target_title.as_str()))
            .filter(|term| !present.iter().any(|link| matches!(
                link, ExtractedLink::Page(title) | ExtractedLink::Tag(title) if title == &term.target_title
            )))
            .filter_map(|term| grafium_core::parser::format_concept_link(&term.target_title))
            .collect::<Vec<_>>().join(" ");
        // A separate child keeps chips outside code fences and other protected
        // Markdown, and represents tags absent from the model's literal prose.
        links.topic_link_blocks.push(standalone);
        offset += topic.tags.len();
    }
    let wrap_changes = if wrap_existing {
        grafium_core::knowledge::source_projection::without_reading_notes(&before).iter().filter_map(|block| {
            let content = wrap(&block.content);
            (content != block.content).then(|| SummaryWrapChange {
                block_id: block.id.clone(),
                previous_content: block.content.clone(),
                new_content: content,
            })
        }).collect()
    } else {
        Vec::new()
    };
    graph.insert_research_summary(
        page_id, wrapped_title.as_deref(), &wrapped_topics, after_block_id,
        wrap_changes, graph_path, Some(&before), &links,
    ).map_err(|error| error.to_string())
}

// ─── RAG / Ask ───────────────────────────────────────────────────────────────

/// A structured citation surfaced to the UI so it can render source chips
/// and navigate to the originating page/block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDto {
    pub index: usize,
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    pub date: Option<String>,
}

impl From<Source> for SourceDto {
    fn from(s: Source) -> Self {
        SourceDto {
            index: s.index,
            page_id: s.page_id,
            page_title: s.page_title,
            block_id: s.block_id,
            date: s.date,
        }
    }
}

/// Answer plus structured sources returned by `ai_ask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResult {
    pub answer: String,
    pub sources: Vec<SourceDto>,
}

#[tauri::command]
pub async fn ai_ask(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    question: String,
    graph_id: Option<String>,
    history: Option<Vec<ChatTurn>>,
    context_target: Option<grafium_core::knowledge::scoped_context::AskContextTarget>,
) -> Result<AskResult, String> {
    let history = history.unwrap_or_default();
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI chat isn't ready — configure and save a Local or Cloud provider in Settings \
             \u{2192} AI / Knowledge Engine first."
                .to_string(),
        );
    }

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let resolved_graph_id =
        graph_id.unwrap_or_else(|| snapshot.root_dir.to_string_lossy().to_string());
    let graph = crate::open_graph_snapshot(&snapshot)?;

    let response = if let Some(target) = context_target.as_ref() {
        engine
            .ask_scoped(
                &graph.db,
                &question,
                Some(resolved_graph_id.as_str()),
                &history,
                target,
            )
            .await
    } else {
        engine
            .ask(
                &graph.db,
                &question,
                Some(resolved_graph_id.as_str()),
                &history,
            )
            .await
    }
    .map_err(|e| e.to_string())?;

    Ok(AskResult {
        answer: response.answer,
        sources: response.sources.into_iter().map(SourceDto::from).collect(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskStreamChunk {
    pub request_id: String,
    pub delta: String,
    /// The current answering phase (`retrieving`, `processing_prompt`,
    /// `thinking`, `generating`, and — in the two-part web-research flow —
    /// `searching_web`, `reading_sources`), when this chunk reports a phase
    /// transition. `None` for a pure text delta or the terminal `done` event.
    /// Drives the UI's evidence-based status indicator; reasoning is never sent
    /// as `delta`, only reflected as the `thinking` phase.
    pub phase: Option<String>,
    /// A transient human-readable progress note for the current phase — e.g.
    /// "Reading source 2/5: …" during a web-research pass. Shown verbatim under
    /// the status label and then discarded; never appended to the answer text.
    #[serde(default)]
    pub note: Option<String>,
    pub done: bool,
    pub error: Option<String>,
}

/// A web source cited by the "From the web" research section, surfaced to the
/// UI so it can render clickable external-link chips distinct from the graph
/// page chips in [`SourceDto`]. `number` matches the inline `[n]` marker in the
/// streamed summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSourceDto {
    pub number: usize,
    pub title: String,
    pub url: String,
}

impl From<Citation> for WebSourceDto {
    fn from(c: Citation) -> Self {
        WebSourceDto {
            number: c.number,
            title: c.title,
            url: c.url,
        }
    }
}

/// Emitted once per request on `ai://chat_sources`, carrying the structured
/// citations for the answer. Kept as a separate event so the existing
/// `AskStreamChunk` shape on `ai://chat_stream` is unchanged and backward
/// compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskSourcesPayload {
    pub request_id: String,
    pub sources: Vec<SourceDto>,
    /// Web sources for the "From the web" section — empty for an ordinary
    /// graph-only answer, so existing consumers are unaffected. Rendered as
    /// clickable external links, distinct from the graph `sources` chips.
    #[serde(default)]
    pub web_sources: Vec<WebSourceDto>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatScope {
    #[default]
    Local,
    Internet,
}

impl ChatScope {
    pub fn require_internet(self) -> Result<(), String> {
        match self {
            Self::Internet => Ok(()),
            Self::Local => Err("Select Internet scope to use Research.".to_string()),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatPreferences {
    scope: ChatScope,
    research: bool,
}

fn chat_preferences_path() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|dir| dir.join("grafium").join("chat.json"))
        .ok_or_else(|| "Could not locate the app configuration directory.".to_string())
}

#[tauri::command]
pub fn get_chat_preferences() -> Result<Option<ChatPreferences>, String> {
    match std::fs::read(chat_preferences_path()?) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| format!("Could not read Chat preferences: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("Could not read Chat preferences: {err}")),
    }
}

#[tauri::command]
pub fn set_chat_preferences(preferences: ChatPreferences) -> Result<(), String> {
    let bytes = serde_json::to_vec(&preferences).map_err(|err| err.to_string())?;
    grafium_core::fsutil::atomic_write(&chat_preferences_path()?, &bytes)
        .map_err(|err| format!("Could not save Chat preferences: {err}"))
}

fn scoped_web_question(scope: ChatScope, question: &str, history: &[ChatTurn]) -> Option<String> {
    if scope != ChatScope::Internet {
        return None;
    }
    let cleaned = detect_research_intent(question)
        .map(|intent| intent.cleaned_question)
        .unwrap_or_else(|| question.to_string());
    Some(conversation::resolve_research_followup(&cleaned, history))
}

#[tauri::command]
pub async fn ai_ask_stream(
    state: State<'_, KnowledgeState>,
    app: tauri::AppHandle,
    app_state: State<'_, crate::AppState>,
    question: String,
    graph_id: Option<String>,
    request_id: String,
    history: Option<Vec<ChatTurn>>,
    scope: Option<ChatScope>,
) -> Result<(), String> {
    let history = history.unwrap_or_default();
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI chat isn't ready — configure and save a Local or Cloud provider in Settings \
             \u{2192} AI / Knowledge Engine first."
                .to_string(),
        );
    }

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let resolved_graph_id =
        graph_id.unwrap_or_else(|| snapshot.root_dir.to_string_lossy().to_string());
    let graph = crate::open_graph_snapshot(&snapshot)?;

    // Register a cancellation flag so `ai_cancel_stream` can stop a slow local
    // generation instead of leaving the user staring at a frozen pane.
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = state.cancels.lock() {
        map.insert(request_id.clone(), cancel.clone());
    }

    // Only the selected scope permits web access; prompt wording and model
    // classification must never silently override a Local graph selection.
    let research = scoped_web_question(scope.unwrap_or_default(), &question, &history);
    let effective_question = research.clone().unwrap_or_else(|| question.clone());

    // Forward real token deltas and phase transitions as they happen. Phase
    // events let the UI show *what* the model is doing (and prove it's alive);
    // reasoning is surfaced only as the `thinking` phase, never as `delta`. In
    // the research flow, `Note` carries the per-source progress line.
    let app_for_events = app.clone();
    let rid = request_id.clone();
    let mut on_event = move |ev: AskStreamEvent<'_>| {
        let (delta, phase, note) = match ev {
            AskStreamEvent::Delta(d) => (d.to_string(), None, None),
            AskStreamEvent::Phase(p) => (String::new(), Some(p.as_str().to_string()), None),
            AskStreamEvent::Note(n) => (String::new(), None, Some(n.to_string())),
        };
        let _ = app_for_events.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: rid.clone(),
                delta,
                phase,
                note,
                done: false,
                error: None,
            },
        );
    };

    let outcome = if research.is_some() {
        engine
            .ask_stream_with_web(
                &graph.db,
                &effective_question,
                Some(resolved_graph_id.as_str()),
                Some(cancel),
                &mut on_event,
            )
            .await
    } else {
        engine
            .ask_stream(
                &graph.db,
                &effective_question,
                Some(resolved_graph_id.as_str()),
                &history,
                Some(cancel),
                &mut on_event,
            )
            .await
    };

    // Deregister the cancel flag regardless of outcome.
    if let Ok(mut map) = state.cancels.lock() {
        map.remove(&request_id);
    }

    let outcome = outcome.map_err(|e| e.to_string())?;

    // Emit the structured citations now that the answer is complete — graph
    // page chips and, for a research answer, the clickable web sources.
    app.emit(
        "ai://chat_sources",
        AskSourcesPayload {
            request_id: request_id.clone(),
            sources: outcome.sources.into_iter().map(SourceDto::from).collect(),
            web_sources: outcome
                .web_citations
                .into_iter()
                .map(WebSourceDto::from)
                .collect(),
        },
    )
    .map_err(|e| e.to_string())?;

    // If the model produced only reasoning (budget exhausted with no answer),
    // show the explanatory message in place of an answer rather than an empty
    // pane or raw chain-of-thought.
    if let Some(message) = outcome.trailing_message {
        app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta: message,
                phase: None,
                note: None,
                done: false,
                error: None,
            },
        )
        .map_err(|e| e.to_string())?;
    }

    app.emit(
        "ai://chat_stream",
        AskStreamChunk {
            request_id,
            delta: String::new(),
            phase: None,
            note: None,
            done: true,
            error: None,
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Cancel an in-flight streamed answer started by `ai_ask_stream`. Flips the
/// request's cancellation flag; the local generation loop checks it and stops,
/// returning what it has so far. A no-op if the request already finished.
#[tauri::command]
pub async fn ai_cancel_stream(
    state: State<'_, KnowledgeState>,
    request_id: String,
) -> Result<(), String> {
    if let Ok(map) = state.cancels.lock() {
        if let Some(flag) = map.get(&request_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    // Flipping the flag alone is cooperative, and the local model does not
    // cooperate: llama.cpp's generation loop checks nothing between tokens, so
    // the flag is only noticed once generation finishes on its own — which for
    // a model that has effectively hung is never. Killing the worker is the
    // only thing that actually stops it, so Stop means stop.
    let engine_guard = state.engine.read().await;
    if let Some(engine) = engine_guard.as_ref() {
        if let Some(llm) = engine.llm_provider() {
            llm.abort_in_flight();
        }
    }
    Ok(())
}

// ─── Graph Registry ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_list_registered_graphs(
    state: State<'_, KnowledgeState>,
) -> Result<Vec<RegisteredGraph>, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    let registry = engine.registry().await;
    Ok(registry.list().into_iter().cloned().collect())
}

#[tauri::command]
pub async fn ai_register_graph(
    state: State<'_, KnowledgeState>,
    name: String,
    path: String,
    graph_type: String,
) -> Result<(), String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    let gtype = match graph_type.as_str() {
        "reference" => GraphType::Reference,
        "ingested" => GraphType::Ingested,
        "archive" => GraphType::Archive,
        _ => GraphType::Primary,
    };

    let graph = RegisteredGraph {
        id: grafium_core::knowledge::GraphRegistry::generate_id(),
        name,
        path: PathBuf::from(path),
        graph_type: gtype,
        last_indexed: None,
        page_count: None,
        vector_count: None,
        cross_searchable: true,
        description: None,
    };

    let mut registry = engine.registry_mut().await;
    registry.register(graph).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        concept_edge_tags, concept_edge_text_chunks, document_concept_edge_seed_tags,
        selected_concept_edge_chunks, unlinked_concept_edge_tags_from_chunks, ConceptEdgesResult,
        PageBatchCursor, CONCEPT_EDGE_CHUNK_CHARS, CONCEPT_EDGE_MAX_CHUNKS_PER_RUN,
        CONCEPT_EDGE_MAX_TAGS_PER_RUN,
    };
    use grafium_core::models::{Block, BlockType};
    use grafium_core::parser::TagTerm;

    #[test]
    fn reading_annotations_are_not_summary_or_concept_sources_and_snapshot_stays_complete() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = grafium_core::Graph::open(directory.path()).unwrap();
        let page = graph.create_page_with_content("Book", false, "- # Cobalt Therapy\n- Cobalt Therapy improves memory.[^1]\n").unwrap();
        let note = graph.reading_note_create("f847c8c3-98ca-4496-a239-fd278aaedfb7", &page.id, None, "# Contradictory Annotation\nUSER-ANNOTATION: Cobalt Therapy never improves memory.").unwrap();
        let before = graph.db.list_blocks_for_page(&page.id).unwrap();
        let projected = super::reference_source_projection(&before);
        assert_eq!(projected.len() + 1, before.len());
        assert!(projected.iter().all(|(id, text)| Some(id) != note.note_block_id.as_ref() && !text.contains("USER-ANNOTATION") && !text.contains("grafium-note-")));
        let chunks = concept_edge_text_chunks(&before);
        assert_eq!(chunks.iter().map(|c| c.block_count).sum::<usize>(), projected.len());
        assert!(chunks.iter().any(|c| c.text.contains("improves memory.[^1]")));
        assert!(chunks.iter().all(|c| !c.text.contains("USER-ANNOTATION") && !c.text.contains("grafium-note-")));
        let tags = document_concept_edge_seed_tags(&before);
        assert!(tags.iter().any(|tag| tag.label() == "Cobalt Therapy"));
        assert!(tags.iter().all(|tag| !tag.label().contains("Contradictory")));
        let after = graph.db.list_blocks_for_page(&page.id).unwrap();
        assert_eq!(after.len(), before.len());
        assert!(after.iter().any(|b| b.content.contains("USER-ANNOTATION")));
        assert!(after.iter().any(|b| b.content.contains("[^grafium-note-")));
    }

    #[test]
    fn chat_scope_defaults_local_and_never_promotes_prompt_intent_to_web() {
        assert_eq!(super::ChatScope::default(), super::ChatScope::Local);
        for question in [
            "Search the web for Rust releases",
            "Look it up online",
            "Latest news today",
        ] {
            assert!(super::scoped_web_question(super::ChatScope::Local, question, &[]).is_none());
        }
        assert!(super::ChatScope::Local.require_internet().is_err());
        assert!(super::ChatScope::Internet.require_internet().is_ok());
    }

    #[test]
    fn chat_scope_internet_uses_web_without_a_research_trigger() {
        assert_eq!(
            super::scoped_web_question(super::ChatScope::Internet, "Rust releases", &[]),
            Some("Rust releases".to_string())
        );
    }

    #[test]
    fn chat_scope_wire_values_are_explicit() {
        assert_eq!(
            serde_json::from_str::<super::ChatScope>("\"local\"").unwrap(),
            super::ChatScope::Local
        );
        assert_eq!(
            serde_json::from_str::<super::ChatScope>("\"internet\"").unwrap(),
            super::ChatScope::Internet
        );
        assert!(serde_json::from_str::<super::ChatScope>("\"auto\"").is_err());
    }

    #[test]
    fn chat_scope_preferences_round_trip_and_safe_defaults() {
        let defaults = serde_json::from_str::<super::ChatPreferences>("{}").unwrap();
        assert_eq!(defaults.scope, super::ChatScope::Local);
        assert!(!defaults.research);
        let saved = super::ChatPreferences {
            scope: super::ChatScope::Internet,
            research: true,
        };
        let bytes = serde_json::to_vec(&saved).unwrap();
        let restored = serde_json::from_slice::<super::ChatPreferences>(&bytes).unwrap();
        assert_eq!(restored.scope, saved.scope);
        assert!(restored.research);
        assert!(serde_json::from_str::<super::ChatPreferences>(r#"{"scope":"auto"}"#).is_err());
    }

    #[test]
    fn page_batch_cursor_streams_past_legacy_ten_thousand_cap() {
        let total_pages = 10_050usize;
        let batch_size = 128i64;
        let mut cursor = PageBatchCursor::new(batch_size);
        let mut requested_windows = Vec::new();
        let mut processed = 0usize;

        while let Some(batch) = cursor
            .next_batch(|limit, offset| {
                requested_windows.push((limit, offset));
                let start = offset as usize;
                if start >= total_pages {
                    return Ok::<Vec<usize>, &'static str>(Vec::new());
                }
                let end = (start + limit as usize).min(total_pages);
                Ok((start..end).collect())
            })
            .expect("cursor should paginate cleanly")
        {
            assert!(
                batch.len() <= batch_size as usize,
                "batch should stay memory-bounded"
            );
            processed += batch.len();
        }

        assert_eq!(processed, total_pages);
        assert!(
            requested_windows
                .iter()
                .any(|(_, offset)| *offset >= 10_000),
            "cursor should continue beyond the old 10k preload cap"
        );
    }

    fn test_block(id: &str, content: &str) -> Block {
        Block {
            id: id.to_string(),
            page_id: "page".to_string(),
            parent_id: None,
            order_index: 0,
            content: content.to_string(),
            block_type: BlockType::Text,
            properties: serde_json::json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn concept_edge_text_chunks_skip_import_metadata() {
        let blocks = vec![
            test_block("source", "Source SHA-256: `abc`"),
            test_block("format", "Format: PDF"),
            test_block("image", "![Page 2 figure](assets/page-2-figure.png)"),
            test_block(
                "real",
                "Kabbalah and the Tree of Life organize the chapter's argument.",
            ),
        ];

        let text = concept_edge_text_chunks(&blocks)
            .into_iter()
            .map(|chunk| chunk.text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Kabbalah"));
        assert!(!text.contains("Source SHA-256"));
        assert!(!text.contains("page-2-figure"));
    }

    #[test]
    fn concept_edge_text_chunks_split_long_pages_by_content_size() {
        let content = "wisdom ".repeat((CONCEPT_EDGE_CHUNK_CHARS / "wisdom ".len()) + 1);
        let blocks = (0..3)
            .map(|i| test_block(&format!("block-{i}"), &content))
            .collect::<Vec<_>>();

        let chunks = concept_edge_text_chunks(&blocks);

        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.block_count == 1));
    }

    #[test]
    fn selected_concept_edge_chunks_sample_across_long_books() {
        let chunks = (0..(CONCEPT_EDGE_MAX_CHUNKS_PER_RUN + 8))
            .map(|i| super::ConceptEdgeTextChunk {
                source_index: i,
                block_count: 1,
                text: format!("chunk {i}"),
            })
            .collect::<Vec<_>>();

        let selected = selected_concept_edge_chunks(&chunks);

        assert_eq!(selected.len(), CONCEPT_EDGE_MAX_CHUNKS_PER_RUN);
        assert_eq!(selected.first().map(|chunk| chunk.source_index), Some(0));
        assert_eq!(
            selected.last().map(|chunk| chunk.source_index),
            Some(chunks.len() - 1)
        );
        assert!(selected
            .windows(2)
            .all(|pair| pair[0].source_index < pair[1].source_index));
    }

    #[test]
    fn concept_edge_tags_require_qualified_labels_for_generic_terms() {
        let tag_batches = vec![vec![
            TagTerm {
                term: "concept".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "topic".to_string(),
                qualified: Some("writing topics".to_string()),
            },
            TagTerm {
                term: "Kabbalah".to_string(),
                qualified: None,
            },
        ]];

        let tags = concept_edge_tags(&tag_batches);
        let labels = tags.iter().map(TagTerm::label).collect::<Vec<_>>();

        assert_eq!(labels, vec!["writing topics", "Kabbalah"]);
    }

    #[test]
    fn concept_edge_tags_reject_bare_broad_book_words() {
        let tag_batches = vec![vec![
            TagTerm {
                term: "energy".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "CONSCIOUSNESS".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "woman".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "number 10".to_string(),
                qualified: Some("Number 10 in Kabbalah".to_string()),
            },
            TagTerm {
                term: "Kabbalistic Tree of Life".to_string(),
                qualified: None,
            },
        ]];

        let tags = concept_edge_tags(&tag_batches);
        let labels = tags.iter().map(TagTerm::label).collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec!["Number 10 in Kabbalah", "Kabbalistic Tree of Life"]
        );
    }

    #[test]
    fn concept_edge_tags_accept_supplements_but_reject_doses_and_dates() {
        let tag_batches = vec![vec![
            TagTerm {
                term: "Magnesium Taurate".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "Niacin".to_string(),
                qualified: Some("Niacin (Nicotinic Acid)".to_string()),
            },
            TagTerm {
                term: "Trimethylglycine".to_string(),
                qualified: Some("Trimethylglycine (TMG)".to_string()),
            },
            TagTerm {
                term: "Vitamin D3".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "Vitamin K2 MK-7".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "Zinc".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "126mg".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "1g".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "90\u{00B5}g".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "2024-06-15".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "Vitamin D3 1,000IU".to_string(),
                qualified: None,
            },
        ]];

        let tags = concept_edge_tags(&tag_batches);
        let labels = tags.iter().map(TagTerm::label).collect::<Vec<_>>();

        assert!(labels.contains(&"Magnesium Taurate"));
        assert!(labels.contains(&"Niacin (Nicotinic Acid)"));
        assert!(labels.contains(&"Trimethylglycine (TMG)"));
        assert!(labels.contains(&"Vitamin D3"));
        assert!(labels.contains(&"Vitamin K2 MK-7"));
        assert!(labels.contains(&"Zinc"));
        assert!(!labels.contains(&"126mg"));
        assert!(!labels.contains(&"1g"));
        assert!(!labels.contains(&"90\u{00B5}g"));
        assert!(!labels.contains(&"2024-06-15"));
        assert!(!labels.contains(&"Vitamin D3 1,000IU"));
    }

    #[test]
    fn document_concept_edge_seed_tags_strip_supplement_doses() {
        let blocks = vec![test_block(
            "supplements",
            "Magnesium Taurate 126mg\nVitamin D3 1,000IU\nVitamin K2 MK-7 90\u{00B5}g\nTrimethylglycine TMG 1g",
        )];

        let tags = document_concept_edge_seed_tags(&blocks);
        let labels = tags.iter().map(TagTerm::label).collect::<Vec<_>>();

        assert!(labels.contains(&"Magnesium Taurate"));
        assert!(labels.contains(&"Vitamin D3"));
        assert!(labels.contains(&"Vitamin K2 MK-7"));
        assert!(labels.contains(&"Trimethylglycine TMG"));
        assert!(!labels.iter().any(|label| label.contains("126mg")));
        assert!(!labels.iter().any(|label| label.contains("1,000IU")));
        assert!(!labels.iter().any(|label| label.contains("90\u{00B5}g")));
        assert!(!labels.iter().any(|label| label.contains("1g")));
    }

    #[test]
    fn document_concept_edge_seed_tags_include_definition_list_entities() {
        let blocks = vec![
            test_block(
                "definitions",
                "**Natto (fermented soybeans)** — Natural source of vitamin K2 (MK-7) and nattokinase\n**Vitamin K2 (MK-7)** — Directs calcium to bones and away from arteries\n**Niacin (Vitamin B3)** — Raises HDL and reduces small dense LDL\n**Omega-3 fatty acids** — Anti-inflammatory support for lipid profile\n**Aged garlic extract** — Helps reduce plaque and blood pressure\n**Nattokinase (supplement)** — Enzyme that may reduce clot formation\n**CoQ10 (ubiquinol)** — Mitochondrial and heart energy support\n**Alpha-lipoic acid (ALA)** — Antioxidant; improves insulin sensitivity\n**Uses**:",
            ),
            test_block(
                "standalone",
                "Potassium\nOmega-3\nNiacin (Vitamin B3) :\nGood for energy\nReduces cholesterol levels",
            ),
        ];

        let tags = document_concept_edge_seed_tags(&blocks);
        let labels = tags.iter().map(TagTerm::label).collect::<Vec<_>>();

        for expected in [
            "Natto (fermented soybeans)",
            "Vitamin K2 (MK-7)",
            "Niacin (Vitamin B3)",
            "Omega-3 fatty acids",
            "Aged garlic extract",
            "Nattokinase (supplement)",
            "CoQ10 (ubiquinol)",
            "Alpha-lipoic acid (ALA)",
            "Potassium",
            "Omega-3",
        ] {
            assert!(labels.contains(&expected), "missing seed tag {expected}");
        }
        assert!(!labels.contains(&"Uses"));
        assert!(!labels.contains(&"Good for energy"));
        assert!(!labels.contains(&"Reduces cholesterol levels"));
    }

    #[test]
    fn document_concept_edge_seed_tags_include_main_book_concepts() {
        let blocks = vec![
            test_block(
                "intro",
                "THE KABALISTIC TREE OF LIFE The Kabbalistic Tree of Life maps divine manifestation.",
            ),
            test_block(
                "practice",
                "PRACTICAL KABALAH The Kabbalah tradition is applied to the Tree of Personal Life.",
            ),
            test_block(
                "paths",
                "Kabbalah emphasizes the 22 Learning Paths and the ten sephirot.",
            ),
        ];

        let tags = document_concept_edge_seed_tags(&blocks);
        let labels = tags.iter().map(TagTerm::label).collect::<Vec<_>>();

        assert!(labels.contains(&"Kabbalah"));
        assert!(labels.contains(&"Kabbalistic Tree of Life"));
        assert!(labels.contains(&"Practical Kabbalah"));
        assert!(labels.contains(&"Tree of Personal Life"));
    }

    #[test]
    fn unlinked_concept_edge_tags_rank_repeated_main_concepts_before_one_off_details() {
        let tag_batches = vec![
            vec![
                TagTerm {
                    term: "Kabbalah".to_string(),
                    qualified: None,
                },
                TagTerm {
                    term: "Kabbalah".to_string(),
                    qualified: None,
                },
                TagTerm {
                    term: "Kabbalah".to_string(),
                    qualified: None,
                },
            ],
            vec![TagTerm {
                term: "mental dominion over the energy field".to_string(),
                qualified: Some("Mental Dominion Over the Energy Field".to_string()),
            }],
        ];
        let blocks = vec![test_block(
            "book",
            "Kabbalah frames the argument. Kabbalah appears throughout. Kabbalah matters more here than one local phrase about mental dominion over the energy field.",
        )];

        let (selected, _limit_reached) =
            unlinked_concept_edge_tags_from_chunks(&tag_batches, &blocks);

        assert_eq!(selected.first().map(TagTerm::label), Some("Kabbalah"));
    }

    #[test]
    fn unlinked_concept_edge_tags_keep_alias_terms_for_same_target() {
        let tag_batches = vec![vec![
            TagTerm {
                term: "Niacin".to_string(),
                qualified: None,
            },
            TagTerm {
                term: "Nicin".to_string(),
                qualified: Some("Niacin".to_string()),
            },
            TagTerm {
                term: "Vitamin B3".to_string(),
                qualified: Some("Niacin".to_string()),
            },
        ]];
        let blocks = vec![test_block(
            "supplements",
            "Already linked [[Niacin]]. Nicin is probably a typo. Vitamin B3 is another alias.",
        )];

        let (selected, _limit_reached) =
            unlinked_concept_edge_tags_from_chunks(&tag_batches, &blocks);
        let mut terms = selected
            .iter()
            .map(|tag| (tag.term.as_str(), tag.label()))
            .collect::<Vec<_>>();
        terms.sort_unstable();

        assert_eq!(terms, vec![("Nicin", "Niacin"), ("Vitamin B3", "Niacin")]);
    }

    #[test]
    fn unlinked_concept_edge_tags_skip_existing_links_and_apply_per_run_budget() {
        let tags = (0..=CONCEPT_EDGE_MAX_TAGS_PER_RUN + 1)
            .map(|i| TagTerm {
                term: format!("Concept {i}"),
                qualified: None,
            })
            .collect::<Vec<_>>();
        let blocks = vec![test_block("existing", "Already linked [[Concept 0]].")];

        let (selected, limit_reached) = unlinked_concept_edge_tags_from_chunks(&[tags], &blocks);
        let labels = selected.iter().map(TagTerm::label).collect::<Vec<_>>();

        assert_eq!(selected.len(), CONCEPT_EDGE_MAX_TAGS_PER_RUN);
        assert!(!labels.contains(&"Concept 0"));
        assert!(labels.contains(&"Concept 1"));
        assert!(limit_reached);
    }

    #[test]
    fn concept_edges_result_serializes_camel_case_fields() {
        let result = ConceptEdgesResult {
            tag_count: 2,
            suggested_edges: 3,
            limit_reached: true,
            chunks_analyzed: 4,
            chunks_total: 8,
            chunks_failed: 1,
            tags: vec![TagTerm {
                term: "Kabbalah".to_string(),
                qualified: None,
            }],
        };
        let value = serde_json::to_value(result).expect("result should serialize");
        assert_eq!(value["tag_count"], 2);
        assert_eq!(value["tag_count"], 2);
        assert_eq!(value["suggested_edges"], 3);
        assert_eq!(value["limit_reached"], true);
        assert_eq!(value["chunks_analyzed"], 4);
        assert_eq!(value["chunks_total"], 8);
        assert_eq!(value["chunks_failed"], 1);
    }
}

// ─── Schemas ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_list_schemas(app_state: State<'_, crate::AppState>) -> Result<Vec<Schema>, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    let manager =
        grafium_core::knowledge::SchemaManager::load(&graph.root_dir).map_err(|e| e.to_string())?;
    Ok(manager.list().into_iter().cloned().collect())
}

#[tauri::command]
pub async fn ai_save_schema(
    app_state: State<'_, crate::AppState>,
    schema: Schema,
) -> Result<(), String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    let mut manager =
        grafium_core::knowledge::SchemaManager::load(&graph.root_dir).map_err(|e| e.to_string())?;
    manager.save_schema(schema).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ai_create_default_schemas(
    app_state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    let mut manager =
        grafium_core::knowledge::SchemaManager::load(&graph.root_dir).map_err(|e| e.to_string())?;
    manager.create_defaults().map_err(|e| e.to_string())
}

// ─── Ported from the AI-isolation branch ───────────────────────────────────

/// Reverse the complete insertion delta, rejecting concurrent changes.
#[tauri::command(rename_all = "camelCase")]
pub fn ai_undo_summary_insert(
    app_state: State<'_, crate::AppState>,
    receipt: AiInsertSummaryResult,
) -> Result<grafium_core::graph::SummaryUndoResult, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    graph.undo_research_summary(&receipt).map_err(|error| error.to_string())
}

/// Restore the same block identities and tree, never a flattened replacement.
#[tauri::command(rename_all = "camelCase")]
pub fn ai_reapply_summary_insert(
    app_state: State<'_, crate::AppState>,
    receipt: AiInsertSummaryResult,
) -> Result<AiInsertSummaryResult, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    graph.reapply_research_summary(&receipt).map_err(|error| error.to_string())?;
    Ok(receipt)
}

#[cfg(test)]
mod summary_safety_tests {
    use super::*;

    fn state() -> KnowledgeState {
        KnowledgeState {
            engine: Arc::new(RwLock::new(None)),
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn summary_operation_registration_cancellation_and_cleanup() {
        let state = state();
        let operation = SummaryOperation::register(&state, Some("synthetic-summary".into())).unwrap();
        assert!(SummaryOperation::register(&state, Some("synthetic-summary".into())).is_err());
        let flag = state.cancels.lock().unwrap()["synthetic-summary"].clone();
        flag.store(true, Ordering::Release);
        let result = operation.run(std::future::pending::<Result<(), grafium_core::CoreError>>()).await;
        assert!(result.unwrap_err().contains("cancelled"));
        drop(operation);
        assert!(state.cancels.lock().unwrap().is_empty());
        let operation = SummaryOperation::register(&state, Some("synthetic-summary".into())).unwrap();
        assert_eq!(operation.run(async { Ok(42) }).await.unwrap(), 42);
        drop(operation);
        assert!(state.cancels.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn summary_operation_model_failure_cleans_up_without_any_model() {
        let state = state();
        {
            let operation = SummaryOperation::register(&state, Some("synthetic-failure".into())).unwrap();
            assert!(operation.run(async {
                Err::<(), _>(grafium_core::CoreError::Other("Synthetic model failure".into()))
            }).await.unwrap_err().contains("Synthetic model failure"));
        }
        assert!(state.cancels.lock().unwrap().is_empty());
    }

    #[test]
    fn summary_command_resolves_existing_targets_without_touching_source_by_default() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = grafium_core::graph::Graph::open(directory.path()).unwrap();
        let canonical = graph.create_page_with_content("solar wind", false, "- Synthetic target\n").unwrap();
        let page = graph.create_page_with_content("Synthetic source", false, "- Solar wind source\n  id:: source\n").unwrap();
        let before = graph.db.list_blocks_for_page(&page.id).unwrap();
        let topics = vec![grafium_core::ai::references::TopicSummary {
            topic: "Solar wind".into(),
            summary: "Solar wind, `Solar wind`, $Solar wind$, https://example.test/Solar_wind and unknown entity.".into(),
            tags: vec![
                TagTerm { term: "Solar wind".into(), qualified: Some("solar_wind".into()) },
                TagTerm { term: "unknown entity".into(), qualified: None },
            ],
        }];
        let receipt = insert_summary_for_graph(&graph, &page.id, None, &topics, Some("source"),
            Some(&graph.root_dir.to_string_lossy()), Some(&before), false).unwrap();
        assert!(receipt.wrap_changes.is_empty());
        assert_eq!(graph.db.get_block_by_id("source").unwrap().content, "Solar wind source");
        let body = &receipt.inserted_blocks[2].content;
        assert!(body.contains("[[solar wind|#solar_wind]]"));
        assert!(body.contains("`Solar wind`"));
        assert!(body.contains("$Solar wind$"));
        assert!(body.contains("https://example.test/Solar_wind"));
        let created = graph.db.get_page_by_title("unknown entity").unwrap();
        assert_eq!(receipt.created_targets.len(), 1);
        assert_eq!(receipt.created_targets[0].id, created.id);
        assert!(graph.db.get_page_by_title("solar_wind").is_err());
        assert_eq!(graph.db.get_page_by_title("solar wind").unwrap().id, canonical.id);
        graph.undo_research_summary(&receipt).unwrap();
        assert!(graph.db.get_page_by_title("unknown entity").is_err());
        let refreshed = graph.db.list_blocks_for_page(&page.id).unwrap();
        let receipt = insert_summary_for_graph(&graph, &page.id, None, &topics, None,
            Some(&graph.root_dir.to_string_lossy()), Some(&refreshed), true).unwrap();
        assert_eq!(receipt.wrap_changes.len(), 1);
        graph.undo_research_summary(&receipt).unwrap();
        assert_eq!(graph.db.get_block_by_id("source").unwrap().content, "Solar wind source");
    }

    #[test]
    fn summary_command_appends_standalone_chips_and_reports_ambiguous_entities() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = grafium_core::graph::Graph::open(directory.path()).unwrap();
        let existing = graph.create_page_with_content("Insulin resistance", false, "- Existing canonical target\n").unwrap();
        let page = graph.create_page_with_content("Synthetic source", false, "- Untouched source prose\n  id:: source\n").unwrap();
        let before = graph.db.list_blocks_for_page(&page.id).unwrap();
        let body = "```\nnovel_space_concept\n```\n\n[Insulin resistance](https://example.test/paper)\n\n$x_y$";
        let topics = vec![
            grafium_core::ai::references::TopicSummary {
                topic: "First topic".into(),
                summary: body.into(),
                tags: vec![
                    TagTerm { term: "insulin_resistance".into(), qualified: None },
                    TagTerm { term: "novel_space_concept".into(), qualified: None },
                    TagTerm { term: "Insuln resistance".into(), qualified: None },
                ],
            },
            grafium_core::ai::references::TopicSummary {
                topic: "Second topic".into(),
                summary: "Different prose, same topic entity.".into(),
                tags: vec![TagTerm { term: "Novel space concept".into(), qualified: None }],
            },
        ];
        assert!(graph.db.get_page_by_title("novel_space_concept").is_err());
        let receipt = insert_summary_for_graph(&graph, &page.id, None, &topics, None,
            Some(&graph.root_dir.to_string_lossy()), Some(&before), false).unwrap();
        assert_eq!(graph.db.get_block_by_id("source").unwrap().content, "Untouched source prose");
        assert_eq!(receipt.inserted_blocks[2].content, body);
        let first_tags = &receipt.inserted_blocks[3];
        assert_eq!(first_tags.parent_id, Some(receipt.inserted_blocks[1].id.clone()));
        assert_eq!(first_tags.order_index, 1);
        assert!(first_tags.content.contains("[[Insulin resistance|#insulin_resistance]]"));
        assert!(first_tags.content.contains("[[novel_space_concept|#novel_space_concept]]"));
        assert!(!first_tags.content.contains("Insuln resistance"));
        assert_eq!(receipt.created_targets.len(), 1);
        assert_eq!(receipt.unlinked_targets.len(), 1);
        assert_eq!(receipt.unlinked_targets[0].source_phrase, "Insuln resistance");
        assert!(!receipt.unlinked_targets[0].reason.is_empty());
        assert!(graph.db.get_page_by_title("Insuln resistance").is_err());
        assert_eq!(graph.db.get_page_by_title("Insulin resistance").unwrap().id, existing.id);
        assert_eq!(receipt.inserted_blocks[6].content, "[[novel_space_concept|#novel_space_concept]]");
        let serialized = serde_json::to_value(&receipt).unwrap();
        assert_eq!(serialized["unlinkedTargets"][0]["sourcePhrase"], "Insuln resistance");
        assert_eq!(serialized["createdTargets"][0]["id"], receipt.created_targets[0].id);
        assert!(graph.undo_research_summary(&receipt).unwrap().retained_targets.is_empty());
        assert!(graph.db.get_page_by_title("novel_space_concept").is_err());
        assert!(graph.db.get_page_by_id(&existing.id).is_ok());
        graph.reapply_research_summary(&receipt).unwrap();
        assert_eq!(graph.db.get_page_by_title("novel_space_concept").unwrap().id, receipt.created_targets[0].id);
    }

    #[test]
    fn summary_command_stale_source_does_not_create_concept_pages() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = grafium_core::graph::Graph::open(directory.path()).unwrap();
        let page = graph.create_page_with_content("Synthetic source", false, "- Original source\n  id:: source\n").unwrap();
        let before = graph.db.list_blocks_for_page(&page.id).unwrap();
        graph.update_block("source", "A pending draft was flushed after generation", None).unwrap();
        let topics = vec![grafium_core::ai::references::TopicSummary {
            topic: "Topic".into(), summary: "Prose without literal tag".into(),
            tags: vec![TagTerm { term: "new_generated_concept".into(), qualified: None }],
        }];
        let result = insert_summary_for_graph(&graph, &page.id, None, &topics, None,
            Some(&graph.root_dir.to_string_lossy()), Some(&before), false);
        assert!(result.unwrap_err().contains("stale"));
        assert!(graph.db.get_page_by_title("new_generated_concept").is_err());
        assert_eq!(graph.db.get_block_by_id("source").unwrap().content, "A pending draft was flushed after generation");
    }
}
