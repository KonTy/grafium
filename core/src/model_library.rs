//! Grafium compatibility adapter for the shared model catalog.

use std::path::{Path, PathBuf};

use crate::error::Result;

pub use model_runtime::models::{
    classify, default_models_dir, is_known_unstable_filename, LocalModelRef, ModelInfo, ModelKind,
    KNOWN_UNSTABLE_ARCHITECTURES, KNOWN_UNSTABLE_MODEL_FILENAMES,
};

pub fn scan_models_dir(models_dir: &Path) -> Result<Vec<ModelInfo>> {
    model_runtime::models::scan_models_dir(models_dir).map_err(Into::into)
}

/// Copies the source without replacing an existing, potentially user-owned file.
pub fn import_model(source: &Path, models_dir: &Path) -> Result<ModelInfo> {
    model_runtime::models::import_model(source, models_dir).map_err(Into::into)
}

pub fn resolve_model(
    configured: Option<&str>,
    models_dir: &Path,
    kind: ModelKind,
) -> Result<PathBuf> {
    model_runtime::models::resolve_model(configured, models_dir, kind).map_err(Into::into)
}
