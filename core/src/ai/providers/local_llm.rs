//! Grafium configuration and CoreError adapter for the shared native provider.

use std::path::Path;

use model_runtime::types::LlmProvider as RuntimeLlm;

use crate::ai::config::{AiConfig, LocalLlmSettings};
use crate::ai::traits::{
    AcceleratorStatus, BoxFuture, ChatMessage, CompletionOptions, Concurrency, LlmProvider,
};
use crate::error::{CoreError, Result};
use crate::model_library::{self, ModelKind};

pub struct LocalLlm(model_runtime::native::llm::LocalLlm);

impl LocalLlm {
    pub fn load(path: &Path, context_size: Option<u32>, gpu_layers: Option<u32>) -> Result<Self> {
        let gpu_layers = Some(crate::ai::resources::safe_gpu_layers(gpu_layers)?);
        Ok(Self(model_runtime::native::llm::LocalLlm::load(
            path,
            context_size,
            gpu_layers,
        )?))
    }

    pub fn from_settings(models_dir: &Path, settings: &LocalLlmSettings) -> Result<Self> {
        let path = settings.model_ref.resolve(models_dir, ModelKind::Llm)?;
        Self::load(&path, settings.context_size, settings.gpu_layers)
    }

    pub fn from_config(config: &AiConfig, data_dir: &Path) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".into()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        Self::from_settings(&models_dir, &local.local_llm)
    }

    pub fn from_config_forcing_gpu(config: &AiConfig, data_dir: &Path) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".into()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        let path = local
            .local_llm
            .model_ref
            .resolve(&models_dir, ModelKind::Llm)?;
        Self::load(
            &path,
            local.local_llm.context_size,
            Some(model_runtime::native::llm::ALL_GPU_LAYERS),
        )
    }

    pub fn chat_template_for_debug(&self) -> Option<String> {
        self.0.chat_template_for_debug()
    }
}

impl LlmProvider for LocalLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.0
                .complete(messages, options)
                .await
                .map_err(CoreError::from)
        })
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.0
                .complete_stream(messages, options, on_token)
                .await
                .map_err(CoreError::from)
        })
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move { self.0.health_check().await.map_err(CoreError::from) })
    }

    fn count_prompt_tokens<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<Option<usize>>> {
        Box::pin(async move {
            self.0
                .count_prompt_tokens(messages, options)
                .await
                .map_err(CoreError::from)
        })
    }

    fn name(&self) -> &str {
        self.0.name()
    }
    fn native_model_path(&self) -> Option<&Path> {
        self.0.native_model_path()
    }
    fn context_window(&self) -> Option<usize> {
        self.0.context_window()
    }
    fn supports_thinking(&self) -> bool {
        self.0.supports_thinking()
    }
    fn concurrency(&self) -> Concurrency {
        self.0.concurrency()
    }
    fn accelerator_status(&self) -> Option<AcceleratorStatus> {
        self.0.accelerator_status()
    }
    fn backend_summary(&self) -> Option<String> {
        self.0.backend_summary()
    }
    fn abort_in_flight(&self) {
        self.0.abort_in_flight()
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use crate::ai::config::{AiConfig, LocalConfig, ProviderType};

    /// `from_config` must resolve the model against `LocalConfig::models_dir`
    /// when it's set, rather than always falling back to
    /// `<data_dir>/models` — this is what lets a user point Grafium at a
    /// models folder shared with other apps (e.g. `~/Documents/models`)
    /// instead of duplicating multi-gigabyte GGUF files into Grafium's own
    /// data directory. We don't need a real model file to prove this: the
    /// "no model found" error message names the directory that was
    /// actually searched, so asserting on that message is enough.
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

        let err = LocalLlm::from_config(&ai_config, data_dir.path())
            .map(|_| ())
            .unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains(&custom_models_dir.path().display().to_string()),
            "expected error to name the configured models_dir ({}), got: {message}",
            custom_models_dir.path().display()
        );
        assert!(
            !message.contains(&data_dir.path().join("models").display().to_string()),
            "must not fall back to the default data_dir/models path when an override is set, got: {message}"
        );
    }

    #[test]
    fn from_config_falls_back_to_default_models_dir_when_unset() {
        let data_dir = tempfile::tempdir().unwrap();

        let mut ai_config = AiConfig::default();
        ai_config.local = Some(LocalConfig {
            provider: ProviderType::HuggingFace,
            models_dir: None,
            ..LocalConfig::default()
        });

        let err = LocalLlm::from_config(&ai_config, data_dir.path())
            .map(|_| ())
            .unwrap_err();

        let default_dir = data_dir.path().join("models");
        assert!(
            err.to_string().contains(&default_dir.display().to_string()),
            "expected default models_dir ({}) in error, got: {err}",
            default_dir.display()
        );
    }
}
