use serde::{Deserialize, Serialize};

/// A single point captured from stylus input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StrokePoint {
    /// X coordinate in canvas units (pixels at reference DPI)
    pub x: f32,
    /// Y coordinate in canvas units
    pub y: f32,
    /// Pressure from 0.0 (no pressure) to 1.0 (max pressure)
    pub pressure: f32,
    /// Tilt angle in radians (0 = perpendicular to screen, π/2 = flat)
    pub tilt: f32,
    /// Timestamp in milliseconds since stroke start
    pub timestamp_ms: u32,
}

/// Pen tool type for the stroke.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum PenTool {
    Pen,
    Highlighter,
    Eraser,
}

impl PenTool {
    pub fn as_str(&self) -> &str {
        match self {
            PenTool::Pen => "pen",
            PenTool::Highlighter => "highlighter",
            PenTool::Eraser => "eraser",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "highlighter" => PenTool::Highlighter,
            "eraser" => PenTool::Eraser,
            _ => PenTool::Pen,
        }
    }
}

/// A single stroke (pen-down to pen-up).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stroke {
    /// Unique ID for this stroke
    pub id: String,
    /// Ordered points captured during this stroke
    pub points: Vec<StrokePoint>,
    /// Pen tool used
    pub tool: PenTool,
    /// Stroke color as CSS hex (e.g. "#1a1a1a")
    pub color: String,
    /// Base stroke width in pixels (modulated by pressure at render time)
    pub width: f32,
}

/// Recognition status for an ink page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum RecognitionStatus {
    /// Not yet recognized
    Pending,
    /// Background recognition complete (indexed for search)
    Indexed,
    /// User has confirmed/corrected the recognized text
    Confirmed,
}

impl RecognitionStatus {
    pub fn as_str(&self) -> &str {
        match self {
            RecognitionStatus::Pending => "pending",
            RecognitionStatus::Indexed => "indexed",
            RecognitionStatus::Confirmed => "confirmed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "indexed" => RecognitionStatus::Indexed,
            "confirmed" => RecognitionStatus::Confirmed,
            _ => RecognitionStatus::Pending,
        }
    }
}

/// An ink page — a full canvas of handwriting stored as an SVG file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InkPage {
    /// All strokes on this page
    pub strokes: Vec<Stroke>,
    /// Canvas width in pixels
    pub canvas_width: f32,
    /// Canvas height in pixels
    pub canvas_height: f32,
    /// Unix timestamp (ms) when this page was created
    pub created_at: i64,
    /// Unix timestamp (ms) of last modification
    pub updated_at: i64,
}

impl InkPage {
    pub fn new(canvas_width: f32, canvas_height: f32) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            strokes: Vec::new(),
            canvas_width,
            canvas_height,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn add_stroke(&mut self, stroke: Stroke) {
        self.strokes.push(stroke);
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }

    pub fn remove_stroke(&mut self, stroke_id: &str) -> bool {
        let len_before = self.strokes.len();
        self.strokes.retain(|s| s.id != stroke_id);
        if self.strokes.len() != len_before {
            self.updated_at = chrono::Utc::now().timestamp_millis();
            true
        } else {
            false
        }
    }
}

/// Index record stored in SQLite for an ink file (NOT the strokes themselves).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InkIndex {
    /// Unique ID
    pub id: String,
    /// Block ID this ink is associated with
    pub block_id: String,
    /// Relative file path to the SVG (e.g. "assets/ink/page-2026-05-22.svg")
    pub file_path: String,
    /// Recognized text (for FTS search) — empty until recognition runs
    pub recognized_text: String,
    /// Recognition status
    pub status: RecognitionStatus,
    /// HTR model version that produced the recognized text
    pub model_version: Option<String>,
    /// Confidence score 0.0-1.0
    pub confidence: Option<f32>,
    /// Unix timestamp when created
    pub created_at: i64,
    /// Unix timestamp when recognition was last run
    pub recognized_at: Option<i64>,
}

/// A correction pair for on-device training.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InkCorrection {
    /// Unique ID
    pub id: String,
    /// Reference to the ink index entry
    pub ink_id: String,
    /// The stroke IDs involved in this correction (region of the page)
    pub stroke_ids: Vec<String>,
    /// What the model recognized (before correction)
    pub original_text: String,
    /// What the user corrected it to
    pub corrected_text: String,
    /// Unix timestamp of correction
    pub created_at: i64,
    /// Whether this correction has been used in fine-tuning
    pub used_in_training: bool,
}
