//! Ink module — stylus stroke capture, SVG serialization, and HTR recognition indexing.
//!
//! Design philosophy: strokes are stored as SVG files on disk (portable, viewable anywhere).
//! SQLite only indexes the recognized text and file path for search/graph integration.

pub mod models;
pub mod svg;

pub use models::*;
pub use svg::{InkSvgParser, InkSvgSerializer};
