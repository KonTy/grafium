use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub file_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_journal: bool,
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BlockType {
    Text,
    Handwriting,
    Audio,
    Mixed,
    Flashcard,
    Query,
}

impl BlockType {
    pub fn as_str(&self) -> &str {
        match self {
            BlockType::Text => "text",
            BlockType::Handwriting => "handwriting",
            BlockType::Audio => "audio",
            BlockType::Mixed => "mixed",
            BlockType::Flashcard => "flashcard",
            BlockType::Query => "query",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "handwriting" => BlockType::Handwriting,
            "audio" => BlockType::Audio,
            "mixed" => BlockType::Mixed,
            "flashcard" => BlockType::Flashcard,
            "query" => BlockType::Query,
            _ => BlockType::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub page_id: String,
    pub parent_id: Option<String>,
    pub order_index: i32,
    pub content: String,
    pub block_type: BlockType,
    pub properties: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LinkType {
    Page,
    Tag,
    Topic,
    BlockRef,
}

impl LinkType {
    pub fn as_str(&self) -> &str {
        match self {
            LinkType::Page => "page",
            LinkType::Tag => "tag",
            LinkType::Topic => "topic",
            LinkType::BlockRef => "block_ref",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "tag" => LinkType::Tag,
            "topic" => LinkType::Topic,
            "block_ref" => LinkType::BlockRef,
            _ => LinkType::Page,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub from_block_id: String,
    pub to_page_id: String,
    pub link_type: LinkType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandwritingStroke {
    pub id: String,
    pub block_id: String,
    pub strokes: Vec<u8>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioNote {
    pub id: String,
    pub block_id: String,
    pub audio_path: String,
    pub duration_ms: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTranscript {
    pub id: String,
    pub audio_id: String,
    pub transcript: String,
    pub is_relevant: bool,
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flashcard {
    pub id: String,
    pub block_id: String,
    pub front: String,
    pub back: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_reviewed_at: Option<i64>,
    pub next_review_at: Option<i64>,
    pub ease_factor: f64,
    pub interval_days: i32,
    pub review_count: i32,
}

/// A study "topic" (deck) derived from a flashcard tag, with review counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashcardTopic {
    /// The tag that names this topic (empty string = untagged cards).
    pub topic: String,
    /// Total cards in this topic.
    pub total: i64,
    /// Cards currently due for review in this topic.
    pub due: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskState {
    Todo,
    Doing,
    Done,
    Canceled,
    Later,
    Now,
}

impl TaskState {
    pub fn as_str(&self) -> &str {
        match self {
            TaskState::Todo => "TODO",
            TaskState::Doing => "DOING",
            TaskState::Done => "DONE",
            TaskState::Canceled => "CANCELED",
            TaskState::Later => "LATER",
            TaskState::Now => "NOW",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "TODO" => Some(TaskState::Todo),
            "DOING" => Some(TaskState::Doing),
            "DONE" => Some(TaskState::Done),
            "CANCELED" | "CANCELLED" => Some(TaskState::Canceled),
            "LATER" => Some(TaskState::Later),
            "NOW" => Some(TaskState::Now),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub block_id: String,
    pub state: TaskState,
    pub scheduled_date: Option<String>,
    pub deadline_date: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Favorite {
    pub id: String,
    pub page_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentPage {
    pub id: String,
    pub page_id: String,
    pub last_opened_at: i64,
}
