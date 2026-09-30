//! Application-neutral provider configuration and native model lifecycle.
//! Hosts supply storage, secrets, and network policy; no UI or graph is owned here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;

use crate::error::{Result, RuntimeError};
use crate::models;
#[cfg(any(feature = "llm-local", feature = "media"))]
use crate::models::ModelKind;
use crate::providers::{
    anthropic::AnthropicLlm,
    ollama::{OllamaEmbedder, OllamaLlm},
    openai::{OpenAiEmbedder, OpenAiLlm},
    openai_compatible::{OpenAiCompatibleEmbedder, OpenAiCompatibleLlm},
    NetworkConfig,
};
use crate::settings::{
    BackendSettings, ModelRole, RuntimeSettings, SettingsController, SettingsPolicy, SettingsStore,
};
use crate::transcription::Transcriber;
use crate::types::{Embedder, LlmProvider};

/// References are resolved only while preparing providers. Implementations must
/// not return secret material through settings/schema/status serialization.
pub trait CredentialProvider: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<Option<String>>;
}

#[derive(Default)]
pub struct MemoryCredentials(BTreeMap<String, String>);

impl MemoryCredentials {
    pub fn insert(&mut self, reference: impl Into<String>, secret: impl Into<String>) {
        self.0.insert(reference.into(), secret.into());
    }
}

impl CredentialProvider for MemoryCredentials {
    fn resolve(&self, reference: &str) -> Result<Option<String>> {
        Ok(self.0.get(reference).cloned())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelIssue {
    pub role: ModelRole,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagerStatus {
    pub settings: RuntimeSettings,
    /// Prepared is not a claim of healthy inference or resident GPU weights.
    pub prepared: Vec<ModelRole>,
    pub issues: Vec<ModelIssue>,
    pub native: NativeRuntimeStatus,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NativeRuntimeStatus {
    pub compiled: bool,
    pub configured: bool,
    pub shutting_down: bool,
    pub worker_pid: Option<u32>,
    pub queued_requests: usize,
    /// Confirmed by the native worker, never inferred from a configured path.
    pub resident: Option<LoadedModel>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadedModel {
    pub role: ModelRole,
    pub model_path: PathBuf,
    pub context_size: Option<u32>,
    pub on_gpu: bool,
    pub gpu_layers: Option<u32>,
}

#[derive(Default)]
struct Bindings {
    settings: RuntimeSettings,
    chat: Option<Arc<dyn LlmProvider>>,
    embeddings: Option<Arc<dyn Embedder>>,
    transcription: Option<Arc<dyn Transcriber + Send + Sync>>,
    selected_paths: Vec<PathBuf>,
    issues: Vec<ModelIssue>,
}

enum Binding {
    Chat(Arc<dyn LlmProvider>),
    Embeddings(Arc<dyn Embedder>),
    #[cfg(feature = "media")]
    Transcription(Arc<dyn Transcriber + Send + Sync>),
}

pub struct ModelManager {
    models_root: PathBuf,
    policy: SettingsPolicy,
    network: NetworkConfig,
    credentials: Arc<dyn CredentialProvider>,
    operations: Mutex<()>,
    state: RwLock<Bindings>,
}

impl ModelManager {
    pub fn new(
        models_root: PathBuf,
        policy: SettingsPolicy,
        network: NetworkConfig,
        credentials: Arc<dyn CredentialProvider>,
    ) -> Self {
        Self {
            models_root,
            policy,
            network,
            credentials,
            operations: Mutex::new(()),
            state: RwLock::new(Bindings::default()),
        }
    }

    pub fn schema(&self) -> schemars::schema::RootSchema {
        self.policy.schema()
    }

    pub fn models_root(&self) -> &Path {
        &self.models_root
    }

    pub fn list_models(&self) -> Result<Vec<models::CatalogEntry>> {
        models::ModelLibrary::new(&self.models_root)?.list()
    }

    pub fn import_model(&self, source: &Path) -> Result<models::ImportResult> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| RuntimeError::Other("Model operation lock is poisoned".into()))?;
        models::ModelLibrary::new(&self.models_root)?.import(source)
    }

    pub async fn download_model(
        &self,
        request: models::DownloadRequest,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        progress: Option<&(dyn Fn(models::DownloadProgress) + Send + Sync)>,
    ) -> Result<models::ImportResult> {
        models::ModelLibrary::new(&self.models_root)?
            .download(request, &self.network, cancel, progress)
            .await
    }

    /// Remove only library-owned files, preserving their bytes in quarantine.
    /// Configured models must first be deselected; native workers are evicted
    /// before acquiring the catalog's exclusive actual-file lease.
    pub fn remove_model(
        &self,
        id: &str,
        approval: models::RemovalApproval,
    ) -> Result<models::RemovalResult> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| RuntimeError::Other("Model operation lock is poisoned".into()))?;
        let library = models::ModelLibrary::new(&self.models_root)?;
        let entry = library
            .list()?
            .into_iter()
            .find(|entry| entry.owned_id.as_deref() == Some(id))
            .ok_or_else(|| {
                RuntimeError::Other("Only a library-owned model ID can be removed".into())
            })?;
        {
            let state = self
                .state
                .read()
                .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
            let explicit_reference = ModelRole::ALL.into_iter().any(|role| {
                let reference = match state.settings.backend(role) {
                    Some(BackendSettings::Embedded {
                        model: Some(model),
                        models_dir,
                        ..
                    })
                    | Some(BackendSettings::Whisper {
                        model: Some(model),
                        models_dir,
                        ..
                    }) => {
                        let configured = Path::new(model);
                        Some(if configured.is_absolute() {
                            configured.to_path_buf()
                        } else {
                            models_dir
                                .as_deref()
                                .unwrap_or(&self.models_root)
                                .join(configured)
                        })
                    }
                    _ => None,
                };
                reference.is_some_and(|path| {
                    path == entry.info.path
                        || path
                            .canonicalize()
                            .is_ok_and(|path| path == entry.info.path)
                })
            });
            if explicit_reference || state.selected_paths.contains(&entry.info.path) {
                return Err(RuntimeError::Other(
                    "Deselect this model before removing it".into(),
                ));
            }
        }
        self.unload_idle()?;
        library.quarantine_owned(id, approval)
    }

    /// Valid configuration can name an offline server or missing model. Such
    /// roles are reported unavailable, not replaced by a different provider.
    pub fn configure(&self, settings: RuntimeSettings) -> Result<ManagerStatus> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| RuntimeError::Other("Model operation lock is poisoned".into()))?;
        let prepared = self.prepare(settings)?;
        let mut current = self
            .state
            .write()
            .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
        self.evict_for_reconfiguration(&current, &prepared.settings)?;
        *current = prepared;
        Ok(status_of(&current))
    }

    /// Prepare before touching storage and commit the in-memory configuration
    /// only after the host's atomic settings save succeeds.
    pub fn configure_persisted<S: SettingsStore>(
        &self,
        settings: RuntimeSettings,
        controller: &mut SettingsController<S>,
    ) -> Result<ManagerStatus> {
        let _operation = self
            .operations
            .lock()
            .map_err(|_| RuntimeError::Other("Model operation lock is poisoned".into()))?;
        controller.validate(&settings).map_err(RuntimeError::from)?;
        let prepared = self.prepare(settings.clone())?;
        let mut current = self
            .state
            .write()
            .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
        self.evict_for_reconfiguration(&current, &prepared.settings)?;
        controller.persist(settings).map_err(RuntimeError::from)?;
        *current = prepared;
        Ok(status_of(&current))
    }

    pub fn status(&self) -> Result<ManagerStatus> {
        self.state
            .read()
            .map(|state| status_of(&state))
            .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))
    }

    pub fn chat(&self) -> Result<Arc<dyn LlmProvider>> {
        let state = self
            .state
            .read()
            .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
        state
            .chat
            .clone()
            .ok_or_else(|| role_error(&state, ModelRole::Chat))
    }

    pub fn embeddings(&self) -> Result<Arc<dyn Embedder>> {
        let state = self
            .state
            .read()
            .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
        state
            .embeddings
            .clone()
            .ok_or_else(|| role_error(&state, ModelRole::Embeddings))
    }

    pub fn transcription(&self) -> Result<Arc<dyn Transcriber + Send + Sync>> {
        let state = self
            .state
            .read()
            .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
        state
            .transcription
            .clone()
            .ok_or_else(|| role_error(&state, ModelRole::Transcription))
    }

    pub fn unload_idle(&self) -> Result<()> {
        #[cfg(any(feature = "llm-local", feature = "media"))]
        crate::native::worker::evict_idle()?;
        Ok(())
    }

    /// Permanently stop this process's shared native pool at application exit,
    /// cancelling queued/active native work and reporting unconfirmed cleanup.
    /// This affects every manager in the process, not independent model servers.
    /// Use `unload_idle` for reversible eviction or settings changes.
    pub fn shutdown_native(&self) -> Result<()> {
        #[cfg(any(feature = "llm-local", feature = "media"))]
        crate::native::worker::shutdown()?;
        Ok(())
    }

    fn evict_for_reconfiguration(&self, current: &Bindings, next: &RuntimeSettings) -> Result<()> {
        if current.selected_paths.is_empty() || &current.settings == next {
            return Ok(());
        }
        match self.unload_idle() {
            Err(RuntimeError::WorkerBusy) => {
                tracing::info!("Native unload deferred until the active model job finishes");
                Ok(())
            }
            other => other,
        }
    }

    pub fn retry_gpu(&self, role: ModelRole) -> Result<ManagerStatus> {
        #[cfg(any(feature = "llm-local", feature = "media"))]
        {
            let _operation = self
                .operations
                .lock()
                .map_err(|_| RuntimeError::Other("Model operation lock is poisoned".into()))?;
            let mut next = self
                .state
                .read()
                .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?
                .settings
                .clone();
            if !next.enabled {
                return Err(RuntimeError::Other(
                    "Enable the model before requesting GPU".into(),
                ));
            }
            let profile = match role {
                ModelRole::Chat => next.chat.as_mut(),
                ModelRole::Embeddings => next.embeddings.as_mut(),
                ModelRole::Transcription => next.transcription.as_mut(),
            }
            .ok_or_else(|| RuntimeError::Other("This model role is not configured".into()))?;
            let (model, directory) = match profile {
                BackendSettings::Embedded {
                    model,
                    models_dir,
                    gpu_layers,
                    ..
                } => {
                    *gpu_layers = Some(1_000_000);
                    (model.clone(), models_dir.clone())
                }
                BackendSettings::Whisper {
                    model, models_dir, ..
                } => (model.clone(), models_dir.clone()),
                _ => {
                    return Err(RuntimeError::Other(
                        "GPU retry applies only to owned native inference".into(),
                    ))
                }
            };
            let kind = match role {
                ModelRole::Chat => ModelKind::Llm,
                ModelRole::Embeddings => ModelKind::Embedding,
                ModelRole::Transcription => ModelKind::Whisper,
            };
            let path = models::resolve_model(
                model.as_deref(),
                directory.as_deref().unwrap_or(&self.models_root),
                kind,
            )?;
            let prepared = self.prepare(next)?;
            if let Some(issue) = prepared.issues.iter().find(|issue| issue.role == role) {
                return Err(RuntimeError::Other(issue.message.clone()));
            }
            let key = crate::native::worker::gpu_risk_key(role.as_str(), &path)?;
            let mut current = self
                .state
                .write()
                .map_err(|_| RuntimeError::Other("Model settings lock is poisoned".into()))?;
            if crate::native::worker::recovery_status()
                .iter()
                .any(|record| record.key == key)
            {
                crate::native::worker::allow_gpu_retry(&key)?;
            } else {
                self.unload_idle()?;
            }
            *current = prepared;
            Ok(status_of(&current))
        }
        #[cfg(not(any(feature = "llm-local", feature = "media")))]
        {
            let _ = role;
            Err(RuntimeError::Other(
                "This build has no native inference".into(),
            ))
        }
    }

    fn prepare(&self, settings: RuntimeSettings) -> Result<Bindings> {
        settings
            .validate(&self.policy)
            .map_err(RuntimeError::from)?;
        let mut bindings = Bindings {
            settings: settings.clone(),
            ..Default::default()
        };
        if !settings.enabled {
            return Ok(bindings);
        }
        for role in ModelRole::ALL {
            let Some(profile) = settings.backend(role) else {
                continue;
            };
            match self.prepare_role(role, profile) {
                Ok((binding, path)) => {
                    if let Some(path) = path {
                        bindings.selected_paths.push(path);
                    }
                    match binding {
                        Binding::Chat(provider) => bindings.chat = Some(provider),
                        Binding::Embeddings(provider) => bindings.embeddings = Some(provider),
                        #[cfg(feature = "media")]
                        Binding::Transcription(provider) => bindings.transcription = Some(provider),
                    }
                }
                Err(error) => {
                    tracing::warn!(role = role.as_str(), %error, "Configured model is unavailable");
                    bindings.issues.push(ModelIssue {
                        role,
                        message: error.to_string(),
                    });
                }
            }
        }
        Ok(bindings)
    }

    fn credential(&self, reference: &str) -> Result<String> {
        self.credentials
            .resolve(reference)?
            .filter(|secret| !secret.is_empty())
            .ok_or_else(|| {
                RuntimeError::Other(format!("Credential reference '{reference}' is unavailable"))
            })
    }

    fn prepare_role(
        &self,
        role: ModelRole,
        profile: &BackendSettings,
    ) -> Result<(Binding, Option<PathBuf>)> {
        let network = self.network.clone();
        let binding = match profile {
            BackendSettings::Ollama {
                base_url,
                model,
                dimension,
            } => match role {
                ModelRole::Chat => {
                    Binding::Chat(Arc::new(OllamaLlm::with_network(base_url, model, network)?))
                }
                ModelRole::Embeddings => {
                    Binding::Embeddings(Arc::new(OllamaEmbedder::with_network(
                        base_url,
                        model,
                        dimension.unwrap_or(768),
                        network,
                    )?))
                }
                _ => return Err(unsupported_role()),
            },
            BackendSettings::OpenAiCompatible {
                base_url,
                model,
                credential_ref,
                dimension,
            } => {
                let key = credential_ref
                    .as_deref()
                    .map(|reference| self.credential(reference))
                    .transpose()?;
                match role {
                    ModelRole::Chat => Binding::Chat(Arc::new(OpenAiCompatibleLlm::with_network(
                        base_url, model, key, network,
                    )?)),
                    ModelRole::Embeddings => {
                        Binding::Embeddings(Arc::new(OpenAiCompatibleEmbedder::with_network(
                            base_url,
                            model,
                            dimension.unwrap_or(1024),
                            key,
                            network,
                        )?))
                    }
                    _ => return Err(unsupported_role()),
                }
            }
            BackendSettings::OpenAi {
                model,
                credential_ref,
                dimension,
            } => {
                let key = self.credential(credential_ref)?;
                match role {
                    ModelRole::Chat => {
                        Binding::Chat(Arc::new(OpenAiLlm::with_network(&key, model, network)?))
                    }
                    ModelRole::Embeddings => {
                        Binding::Embeddings(Arc::new(OpenAiEmbedder::with_network(
                            &key,
                            model,
                            dimension.unwrap_or(1536),
                            network,
                        )?))
                    }
                    _ => return Err(unsupported_role()),
                }
            }
            BackendSettings::Anthropic {
                model,
                credential_ref,
            } => {
                if role != ModelRole::Chat {
                    return Err(unsupported_role());
                }
                let key = self.credential(credential_ref)?;
                Binding::Chat(Arc::new(AnthropicLlm::with_network(
                    "https://api.anthropic.com/v1",
                    &key,
                    model,
                    network,
                )?))
            }
            BackendSettings::Embedded {
                model,
                models_dir,
                context_size,
                gpu_layers,
            } => {
                #[cfg(feature = "llm-local")]
                {
                    let kind = if role == ModelRole::Chat {
                        ModelKind::Llm
                    } else {
                        ModelKind::Embedding
                    };
                    let path = models::resolve_model(
                        model.as_deref(),
                        models_dir.as_deref().unwrap_or(&self.models_root),
                        kind,
                    )?;
                    let binding = match role {
                        ModelRole::Chat => Binding::Chat(Arc::new(
                            crate::native::llm::LocalLlm::load(&path, *context_size, *gpu_layers)?,
                        )),
                        ModelRole::Embeddings => Binding::Embeddings(Arc::new(
                            crate::native::embedder::LocalEmbedder::load_with_options(
                                &path,
                                *context_size,
                                *gpu_layers,
                            )?,
                        )),
                        _ => return Err(unsupported_role()),
                    };
                    return Ok((binding, Some(path)));
                }
                #[cfg(not(feature = "llm-local"))]
                {
                    let _ = (model, models_dir, context_size, gpu_layers);
                    return Err(RuntimeError::Other(
                        "Embedded model support is not compiled in".into(),
                    ));
                }
            }
            BackendSettings::Whisper {
                model,
                models_dir,
                language,
            } => {
                #[cfg(feature = "media")]
                {
                    if role != ModelRole::Transcription {
                        return Err(unsupported_role());
                    }
                    let path = models::resolve_model(
                        model.as_deref(),
                        models_dir.as_deref().unwrap_or(&self.models_root),
                        ModelKind::Whisper,
                    )?;
                    return Ok((
                        Binding::Transcription(Arc::new(
                            crate::native::transcribe::WorkerTranscriber::new(
                                &path,
                                language.as_deref(),
                            ),
                        )),
                        Some(path),
                    ));
                }
                #[cfg(not(feature = "media"))]
                {
                    let _ = (model, models_dir, language);
                    return Err(RuntimeError::Other(
                        "Whisper support is not compiled in".into(),
                    ));
                }
            }
        };
        Ok((binding, None))
    }
}

fn unsupported_role() -> RuntimeError {
    RuntimeError::Other("The configured backend does not support this model role".into())
}

fn role_error(state: &Bindings, role: ModelRole) -> RuntimeError {
    let message = state
        .issues
        .iter()
        .find(|issue| issue.role == role)
        .map(|issue| issue.message.clone())
        .unwrap_or_else(|| format!("{} model is not configured or enabled", role.as_str()));
    RuntimeError::Other(message)
}

fn status_of(state: &Bindings) -> ManagerStatus {
    let mut prepared = Vec::new();
    if state.chat.is_some() {
        prepared.push(ModelRole::Chat);
    }
    if state.embeddings.is_some() {
        prepared.push(ModelRole::Embeddings);
    }
    if state.transcription.is_some() {
        prepared.push(ModelRole::Transcription);
    }
    ManagerStatus {
        settings: state.settings.clone(),
        prepared,
        issues: state.issues.clone(),
        native: native_status(),
    }
}

fn native_status() -> NativeRuntimeStatus {
    #[cfg(any(feature = "llm-local", feature = "media"))]
    {
        use crate::native::worker::NativeModelKind;
        let status = crate::native::worker::status();
        NativeRuntimeStatus {
            compiled: true,
            configured: status.configured,
            shutting_down: status.shutting_down,
            worker_pid: status.worker_pid,
            queued_requests: status.queued_requests,
            resident: status.resident.map(|resident| LoadedModel {
                role: match resident.model.kind {
                    NativeModelKind::Chat => ModelRole::Chat,
                    NativeModelKind::Embeddings => ModelRole::Embeddings,
                    NativeModelKind::Transcription => ModelRole::Transcription,
                },
                model_path: resident.model.model_path,
                context_size: resident.model.context_size,
                on_gpu: resident.model.on_gpu,
                gpu_layers: resident.model.gpu_layers,
            }),
        }
    }
    #[cfg(not(any(feature = "llm-local", feature = "media")))]
    {
        NativeRuntimeStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn settings(model: &str) -> RuntimeSettings {
        RuntimeSettings {
            enabled: true,
            chat: Some(BackendSettings::OpenAiCompatible {
                base_url: "http://127.0.0.1:1/v1".into(),
                model: model.into(),
                credential_ref: None,
                dimension: None,
            }),
            ..Default::default()
        }
    }

    fn manager(root: &Path, credentials: MemoryCredentials) -> ModelManager {
        ModelManager::new(
            root.to_path_buf(),
            SettingsPolicy::default(),
            NetworkConfig::new(std::time::Duration::from_secs(1)).unwrap(),
            Arc::new(credentials),
        )
    }

    #[test]
    fn configuration_prepares_without_connecting_and_rejects_invalid_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path(), MemoryCredentials::default());
        let original = settings("configured-model");
        let status = manager.configure(original.clone()).unwrap();
        assert_eq!(status.prepared, vec![ModelRole::Chat]);
        assert!(status.issues.is_empty());
        assert!(manager.chat().is_ok());
        let mut invalid = settings("");
        invalid.schema_version = 999;
        assert!(manager.configure(invalid).is_err());
        assert_eq!(manager.status().unwrap().settings, original);
    }

    #[test]
    fn missing_credentials_are_explicit_role_failures_not_another_provider() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path(), MemoryCredentials::default());
        let settings = RuntimeSettings {
            enabled: true,
            chat: Some(BackendSettings::OpenAi {
                model: "test".into(),
                credential_ref: "missing".into(),
                dimension: None,
            }),
            ..Default::default()
        };
        let status = manager.configure(settings).unwrap();
        assert!(status.prepared.is_empty());
        assert_eq!(status.issues.len(), 1);
        assert!(manager
            .chat()
            .err()
            .unwrap()
            .to_string()
            .contains("Credential reference"));
    }

    #[derive(Default)]
    struct Store {
        value: Mutex<Option<RuntimeSettings>>,
        fail: AtomicBool,
    }
    impl SettingsStore for Store {
        fn load(&self) -> Result<Option<RuntimeSettings>> {
            Ok(self.value.lock().unwrap().clone())
        }
        fn save(&self, settings: &RuntimeSettings) -> Result<()> {
            if self.fail.load(Ordering::Relaxed) {
                return Err(RuntimeError::Other("synthetic storage failure".into()));
            }
            *self.value.lock().unwrap() = Some(settings.clone());
            Ok(())
        }
    }

    #[test]
    fn failed_persistence_keeps_both_old_settings_and_providers() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path(), MemoryCredentials::default());
        let store = Arc::new(Store::default());
        let mut controller = SettingsController::new(store.clone(), SettingsPolicy::default());
        manager
            .configure_persisted(settings("first"), &mut controller)
            .unwrap();
        store.fail.store(true, Ordering::Relaxed);
        assert!(manager
            .configure_persisted(settings("second"), &mut controller)
            .is_err());
        assert_eq!(manager.status().unwrap().settings, settings("first"));
        assert_eq!(controller.read(), &settings("first"));
        assert_eq!(store.load().unwrap(), Some(settings("first")));
    }

    #[test]
    fn status_and_schema_never_include_resolved_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let mut credentials = MemoryCredentials::default();
        credentials.insert("test-reference", "synthetic-secret-value");
        let manager = manager(dir.path(), credentials);
        let mut config = settings("model");
        if let Some(BackendSettings::OpenAiCompatible { credential_ref, .. }) = &mut config.chat {
            *credential_ref = Some("test-reference".into());
        }
        let status = serde_json::to_string(&manager.configure(config).unwrap()).unwrap();
        let schema = serde_json::to_string(&manager.schema()).unwrap();
        assert!(!status.contains("synthetic-secret-value"));
        assert!(!schema.contains("synthetic-secret-value"));
    }

    #[cfg(not(any(feature = "llm-local", feature = "media")))]
    #[test]
    fn native_shutdown_does_not_disable_network_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path(), MemoryCredentials::default());
        let settings = settings("server-owned-model");
        manager.configure(settings.clone()).unwrap();
        manager.shutdown_native().unwrap();
        assert!(manager.chat().is_ok());
        assert_eq!(manager.status().unwrap().settings, settings);
        assert!(!manager.status().unwrap().native.compiled);
    }

    #[test]
    fn owned_removal_is_reversible_and_preserves_source_and_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("downloaded.gguf");
        std::fs::write(&original, b"synthetic model payload").unwrap();
        let root = dir.path().join("managed");
        let manager = manager(&root, MemoryCredentials::default());
        let imported = manager.import_model(&original).unwrap();
        let id = imported.entry.owned_id.unwrap();
        let unrelated = root.join("other.txt");
        std::fs::write(&unrelated, b"leave alone").unwrap();
        let removed = manager
            .remove_model(&id, models::RemovalApproval::ExplicitUserRequest)
            .unwrap();
        assert_eq!(
            std::fs::read(removed.quarantine_path).unwrap(),
            b"synthetic model payload"
        );
        assert!(original.exists());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"leave alone");
        assert!(!removed.original_path.exists());
    }

    #[test]
    fn configured_model_removal_is_rejected_before_mutating_files() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("model.gguf");
        std::fs::write(&original, b"synthetic model payload").unwrap();
        let manager = manager(&dir.path().join("managed"), MemoryCredentials::default());
        let imported = manager.import_model(&original).unwrap();
        let path = imported.entry.info.path;
        let id = imported.entry.owned_id.unwrap();
        // This policy check is independent of whether C++ support is compiled.
        manager
            .state
            .write()
            .unwrap()
            .selected_paths
            .push(path.clone());
        assert!(manager
            .remove_model(&id, models::RemovalApproval::ExplicitUserRequest)
            .unwrap_err()
            .to_string()
            .contains("Deselect"));
        assert!(path.exists());
        assert!(manager
            .remove_model("unmanaged", models::RemovalApproval::ExplicitUserRequest)
            .is_err());
    }
}
