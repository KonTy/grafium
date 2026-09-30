//! Translate Grafium's persisted settings without coupling the shared schema to
//! notes, indexing, reference generation, or the legacy credential file format.

use model_runtime::manager::MemoryCredentials;
use model_runtime::settings::{BackendSettings, RuntimeSettings};

use super::config::{AiConfig, AiMode, CloudConfig, LocalConfig, ProviderType};
use crate::error::{CoreError, Result};

pub(crate) fn configuration(config: &AiConfig) -> Result<(RuntimeSettings, MemoryCredentials)> {
    let mut settings = RuntimeSettings {
        enabled: config.enabled,
        ..Default::default()
    };
    let mut credentials = MemoryCredentials::default();
    match config.mode {
        AiMode::Local => {
            if let Some(local) = &config.local {
                if let Some(key) = &local.api_key {
                    credentials.insert("local", key);
                }
                settings.chat = Some(local_profile(local, false)?);
                settings.embeddings = Some(local_profile(local, true)?);
            }
        }
        AiMode::Cloud => {
            if let Some(cloud) = &config.cloud {
                collect_cloud_credentials(cloud, &mut credentials);
                settings.chat = cloud_profile(cloud, false)?;
                settings.embeddings = cloud_profile(cloud, true)?;
            }
        }
        AiMode::Hybrid => {
            if let Some(local) = &config.local {
                if let Some(key) = &local.api_key {
                    credentials.insert("local", key);
                }
                if matches!(
                    local.provider,
                    ProviderType::Ollama | ProviderType::OpenAiCompatible
                ) {
                    settings.embeddings = Some(local_profile(local, true)?);
                }
            }
            if let Some(cloud) = &config.cloud {
                collect_cloud_credentials(cloud, &mut credentials);
                settings.chat = cloud_profile(cloud, false)?;
            }
        }
    }
    Ok((settings, credentials))
}

fn local_profile(local: &LocalConfig, embedding: bool) -> Result<BackendSettings> {
    let model = if embedding {
        &local.embedding_model
    } else {
        &local.llm_model
    };
    match local.provider {
        ProviderType::Ollama => Ok(BackendSettings::Ollama {
            base_url: local.base_url.clone(),
            model: model.clone(),
            dimension: embedding.then_some(768),
        }),
        ProviderType::OpenAiCompatible => Ok(BackendSettings::OpenAiCompatible {
            base_url: local.base_url.clone(),
            model: model.clone(),
            credential_ref: local.api_key.as_ref().map(|_| "local".into()),
            dimension: embedding.then_some(1024),
        }),
        ProviderType::HuggingFace => Ok(BackendSettings::Embedded {
            model: if embedding {
                local.local_embedding.model_ref.model.clone()
            } else {
                local.local_llm.model_ref.model.clone()
            },
            models_dir: local.models_dir.clone(),
            context_size: if embedding {
                None
            } else {
                local.local_llm.context_size
            },
            // Legacy embedded embeddings requested automatic full offload;
            // chat's absent setting meant CPU. Both still undergo admission.
            gpu_layers: if embedding {
                Some(1_000_000)
            } else {
                local.local_llm.gpu_layers
            },
        }),
        _ => Err(CoreError::Other("Unsupported local provider".into())),
    }
}

fn collect_cloud_credentials(cloud: &CloudConfig, credentials: &mut MemoryCredentials) {
    if let Some(key) = &cloud.llm_api_key {
        credentials.insert("cloud-chat", key);
    }
    if let Some(key) = cloud
        .embedding_api_key
        .as_ref()
        .or(cloud.llm_api_key.as_ref())
    {
        credentials.insert("cloud-embeddings", key);
    }
}

fn cloud_profile(cloud: &CloudConfig, embedding: bool) -> Result<Option<BackendSettings>> {
    let provider = if embedding {
        &cloud.embedding_provider
    } else {
        &cloud.llm_provider
    };
    let model = if embedding {
        &cloud.embedding_model
    } else {
        &cloud.llm_model
    };
    let key = if embedding {
        cloud
            .embedding_api_key
            .as_ref()
            .or(cloud.llm_api_key.as_ref())
    } else {
        cloud.llm_api_key.as_ref()
    };
    let reference = if embedding {
        "cloud-embeddings"
    } else {
        "cloud-chat"
    };
    let profile = match provider {
        ProviderType::OpenAi => BackendSettings::OpenAi {
            model: model.clone(),
            credential_ref: reference.into(),
            dimension: embedding.then_some(1536),
        },
        ProviderType::Anthropic if !embedding => BackendSettings::Anthropic {
            model: model.clone(),
            credential_ref: reference.into(),
        },
        ProviderType::OpenAiCompatible => BackendSettings::OpenAiCompatible {
            model: model.clone(),
            base_url: if embedding {
                cloud
                    .embedding_base_url
                    .clone()
                    .or(cloud.llm_base_url.clone())
            } else {
                cloud.llm_base_url.clone()
            }
            .unwrap_or_else(|| "http://localhost:8000/v1".into()),
            credential_ref: key.map(|_| reference.into()),
            dimension: embedding.then_some(1024),
        },
        _ => return Ok(None),
    };
    Ok(Some(profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_settings_use_references_without_serializing_legacy_secrets() {
        let mut config = AiConfig {
            enabled: true,
            ..Default::default()
        };
        config.local.as_mut().unwrap().api_key = Some("synthetic-private-credential".into());
        let (settings, _) = configuration(&config).unwrap();
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("credential_ref"));
        assert!(!json.contains("synthetic-private-credential"));
    }

    #[test]
    fn existing_native_cpu_chat_and_auto_embedding_choices_are_preserved() {
        let mut config = AiConfig {
            enabled: true,
            ..Default::default()
        };
        config.local.as_mut().unwrap().provider = ProviderType::HuggingFace;
        let (settings, _) = configuration(&config).unwrap();
        assert!(matches!(
            settings.chat,
            Some(BackendSettings::Embedded {
                gpu_layers: None,
                ..
            })
        ));
        assert!(matches!(
            settings.embeddings,
            Some(BackendSettings::Embedded {
                gpu_layers: Some(1_000_000),
                ..
            })
        ));
    }

    #[test]
    fn disabling_ai_does_not_erase_saved_provider_choices() {
        let mut config = AiConfig::default();
        config.local.as_mut().unwrap().llm_model = "saved-model".into();
        let (settings, _) = configuration(&config).unwrap();
        assert!(!settings.enabled);
        assert!(
            matches!(settings.chat, Some(BackendSettings::OpenAiCompatible { model, .. }) if model == "saved-model")
        );
    }
}
