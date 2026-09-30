//! Grafium configuration and CoreError adapter for the shared native embedder.

use crate::ai::config::LocalEmbeddingSettings;
use crate::ai::traits::{BoxFuture, Embedder};
use crate::error::{CoreError, Result};
use crate::model_library::{self, ModelKind};
use model_runtime::types::Embedder as RuntimeEmbedder;
use std::path::Path;

pub struct LocalEmbedder(model_runtime::native::embedder::LocalEmbedder);

impl LocalEmbedder {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self(model_runtime::native::embedder::LocalEmbedder::load(
            path,
        )?))
    }

    pub fn from_settings(models_dir: &Path, settings: &LocalEmbeddingSettings) -> Result<Self> {
        Self::load(
            &settings
                .model_ref
                .resolve(models_dir, ModelKind::Embedding)?,
        )
    }

    pub fn from_config(config: &crate::ai::config::AiConfig, data_dir: &Path) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".into()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        Self::from_settings(&models_dir, &local.local_embedding)
    }
}

impl Embedder for LocalEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move { self.0.embed(texts).await.map_err(CoreError::from) })
    }
    fn embed_documents<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move { self.0.embed_documents(texts).await.map_err(CoreError::from) })
    }
    fn embed_queries<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move { self.0.embed_queries(texts).await.map_err(CoreError::from) })
    }
    fn embed_query<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        Box::pin(async move { self.0.embed_query(text).await.map_err(CoreError::from) })
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
    fn model_name(&self) -> &str {
        self.0.model_name()
    }
    fn embedding_scheme_id(&self) -> String {
        self.0.embedding_scheme_id()
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use crate::ai::config::{AiConfig, LocalConfig, ProviderType};

    fn gguf_string(value: &str) -> Vec<u8> {
        let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(value.as_bytes());
        bytes
    }

    fn metadata_fixture(entries: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        // No tensors: this fixture cannot be loaded for inference. Successful
        // preparation therefore proves that it did not request a native load.
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        for (key, kind, value) in entries {
            bytes.extend(gguf_string(key));
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(value);
        }
        bytes.resize(bytes.len().div_ceil(32) * 32, 0);
        bytes
    }

    fn model_fixture(architecture: &str, context: u32, dimension: u32) -> Vec<u8> {
        metadata_fixture(&[
            ("general.architecture", 8, gguf_string(architecture)),
            (
                &format!("{architecture}.context_length"),
                4,
                context.to_le_bytes().to_vec(),
            ),
            (
                &format!("{architecture}.embedding_length"),
                4,
                dimension.to_le_bytes().to_vec(),
            ),
        ])
    }

    #[test]
    fn engine_startup_and_reconfigure_use_current_model_metadata_and_models_root() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("knowledge");
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(
            models_dir.join("embed.gguf"),
            model_fixture("bert", 512, 384),
        )
        .unwrap();
        let mut config = AiConfig {
            enabled: true,
            local: Some(LocalConfig {
                provider: ProviderType::HuggingFace,
                local_llm: crate::ai::config::LocalLlmSettings {
                    model_ref: model_library::LocalModelRef::named("missing-chat.gguf"),
                    ..Default::default()
                },
                local_embedding: LocalEmbeddingSettings {
                    model_ref: model_library::LocalModelRef::named("embed.gguf"),
                },
                ..Default::default()
            }),
            ..AiConfig::default()
        };
        let mut engine =
            crate::KnowledgeEngine::new_with_models_root(&data_dir, config.clone(), dir.path())
                .unwrap();
        assert!(
            engine.can_index(),
            "initialization must use the host models root"
        );
        assert!(!engine.is_llm_ready());
        assert!(engine.llm_load_error().is_some());

        config.local.as_mut().unwrap().local_embedding.model_ref =
            model_library::LocalModelRef::named("missing-embed.gguf");
        engine.reconfigure(config.clone()).unwrap();
        assert!(
            !engine.can_index(),
            "a prior provider must not survive new config"
        );

        config.local.as_mut().unwrap().local_embedding.model_ref =
            model_library::LocalModelRef::named("embed.gguf");
        engine.reconfigure(config.clone()).unwrap();
        assert!(
            engine.can_index(),
            "reconfiguration must retain the host models root"
        );

        config.enabled = false;
        engine.reconfigure(config).unwrap();
        assert!(!engine.can_index());
        assert!(
            engine.llm_load_error().is_none(),
            "disabled AI must not retain stale errors"
        );
    }

    /// Same reasoning as `local_llm`'s equivalent test: proves
    /// `from_config` resolves against `LocalConfig::models_dir` when set,
    /// rather than always falling back to `<data_dir>/models`, without
    /// needing a real model file on disk (the "no model found" error names
    /// the directory that was actually searched).
    #[test]
    fn from_config_honors_models_dir_override_over_default_data_dir() {
        let data_dir = tempfile::tempdir().unwrap();
        let custom_models_dir = tempfile::tempdir().unwrap();

        let mut ai_config = AiConfig::default();
        ai_config.local = Some(LocalConfig {
            provider: ProviderType::HuggingFace,
            models_dir: Some(custom_models_dir.path().to_path_buf()),
            ..LocalConfig::default()
        });

        let err = LocalEmbedder::from_config(&ai_config, data_dir.path())
            .map(|_| ())
            .unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains(&custom_models_dir.path().display().to_string()),
            "expected error to name the configured models_dir ({}), got: {message}",
            custom_models_dir.path().display()
        );
    }
}
