//! Host-independent settings, generated JSON Schema, validation, and persistence.
//!
//! This module never resolves credentials, contacts endpoints, discovers models,
//! or loads native libraries. Credential references are identifiers in a
//! host-owned secret store, not secret values. A host's model manager must prepare
//! replacement bindings before persisting and switching a configuration.
//!
//! Provider restrictions are not a network sandbox: Ollama and compatible URLs
//! can point off-machine. Hosts must also apply their transport `NetworkConfig`,
//! including DNS, redirect, and proxy policy. Endpoint authorizers must be pure.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use schemars::schema::{RootSchema, Schema, SchemaObject, SubschemaValidation};
use schemars::{gen::SchemaGenerator, JsonSchema};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::resources::{safe_context_size, safe_gpu_layers};
use crate::RuntimeError;

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
const MAX_MODEL_LENGTH: u32 = 1_024;
const MAX_PATH_LENGTH: u32 = 4_096;
const MAX_ENDPOINT_LENGTH: u32 = 4_096;
const MAX_CREDENTIAL_REF_LENGTH: u32 = 256;
const MAX_LANGUAGE_LENGTH: u32 = 32;
// Runtime admission remains authoritative; the boundary test keeps this schema
// annotation aligned with resources::safe_context_size.
const MAX_CONTEXT_SIZE: u32 = 16_384;
const TEXT_PATTERN: &str = r"^[^\u0000-\u001f\u007f-\u009f]*\S[^\u0000-\u001f\u007f-\u009f]*$";
const CREDENTIAL_REF_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._:/-]*$";

/// A task for which a backend is configured, not a claim that it is connected.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ModelRole {
    Chat,
    Embeddings,
    Transcription,
}

impl ModelRole {
    pub const ALL: [Self; 3] = [Self::Chat, Self::Embeddings, Self::Transcription];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Embeddings => "embeddings",
            Self::Transcription => "transcription",
        }
    }

    pub const fn supports(self, backend: BackendKind) -> bool {
        match self {
            Self::Chat => !matches!(backend, BackendKind::Whisper),
            Self::Embeddings => !matches!(backend, BackendKind::Anthropic | BackendKind::Whisper),
            Self::Transcription => matches!(backend, BackendKind::Whisper),
        }
    }
}

/// The provider discriminator used in settings and host capability declarations.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    Embedded,
    Whisper,
    Ollama,
    OpenAiCompatible,
    OpenAi,
    Anthropic,
}

impl BackendKind {
    pub const ALL: [Self; 6] = [
        Self::Embedded,
        Self::Whisper,
        Self::Ollama,
        Self::OpenAiCompatible,
        Self::OpenAi,
        Self::Anthropic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Whisper => "whisper",
            Self::Ollama => "ollama",
            Self::OpenAiCompatible => "open-ai-compatible",
            Self::OpenAi => "open-ai",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Persisted AI configuration. Enabled with no roles remains unconfigured.
///
/// Missing fields use disabled, unconfigured version-one defaults. Unknown fields
/// (including plaintext `api_key` fields) are rejected rather than ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(title = "AI runtime settings")]
pub struct RuntimeSettings {
    /// Settings format version. Unsupported versions require explicit migration.
    #[schemars(title = "Schema version", schema_with = "version_schema")]
    pub schema_version: u32,
    /// Permit AI use. This does not indicate model availability or connectivity.
    #[schemars(title = "Enable AI")]
    pub enabled: bool,
    /// Optional generation and conversation backend.
    #[schemars(title = "Chat")]
    pub chat: Option<BackendSettings>,
    /// Optional vector embedding backend.
    #[schemars(title = "Embeddings")]
    pub embeddings: Option<BackendSettings>,
    /// Optional local audio transcription backend.
    #[schemars(title = "Transcription")]
    pub transcription: Option<BackendSettings>,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            enabled: false,
            chat: None,
            embeddings: None,
            transcription: None,
        }
    }
}

impl RuntimeSettings {
    pub fn backend(&self, role: ModelRole) -> Option<&BackendSettings> {
        match role {
            ModelRole::Chat => self.chat.as_ref(),
            ModelRole::Embeddings => self.embeddings.as_ref(),
            ModelRole::Transcription => self.transcription.as_ref(),
        }
    }

    /// Validate without rewriting persisted JSON or performing model/network I/O.
    pub fn validate(&self, policy: &SettingsPolicy) -> Result<(), ValidationErrors> {
        validate_settings(self, policy)
    }
}

/// Backend-specific settings. Secret material belongs in a separate host store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "backend", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BackendSettings {
    /// Native llama.cpp inference. Model discovery and resource admission happen
    /// separately when the model manager prepares a binding.
    #[schemars(title = "Embedded llama.cpp")]
    Embedded {
        /// Model identifier or path; null preserves host model auto-selection.
        #[serde(default)]
        #[schemars(title = "Model", schema_with = "optional_model_schema")]
        model: Option<String>,
        /// Host-selected model directory; null uses the host's default directory.
        #[serde(default)]
        #[schemars(title = "Models directory", schema_with = "path_schema")]
        models_dir: Option<PathBuf>,
        /// Context tokens; null uses the backend/model default.
        #[serde(default)]
        #[schemars(title = "Context size", schema_with = "context_schema")]
        context_size: Option<u32>,
        /// GPU layers; null or zero uses CPU. Admission may still require CPU.
        #[serde(default)]
        #[schemars(title = "GPU layers", schema_with = "gpu_schema")]
        gpu_layers: Option<u32>,
    },
    /// Native Whisper audio transcription.
    #[schemars(title = "Whisper")]
    Whisper {
        /// Model identifier or path; null preserves host model auto-selection.
        #[serde(default)]
        #[schemars(title = "Model", schema_with = "optional_model_schema")]
        model: Option<String>,
        /// Host-selected model directory; null uses the host's default directory.
        #[serde(default)]
        #[schemars(title = "Models directory", schema_with = "path_schema")]
        models_dir: Option<PathBuf>,
        /// Language code, such as en; null allows automatic language detection.
        #[serde(default)]
        #[schemars(title = "Language", schema_with = "language_schema")]
        language: Option<String>,
    },
    /// An Ollama HTTP endpoint, which is not necessarily local.
    #[schemars(title = "Ollama")]
    Ollama {
        /// Absolute HTTP(S) base URL without credentials, query, or fragment.
        #[schemars(title = "Base URL", schema_with = "endpoint_schema")]
        base_url: String,
        /// Model identifier served by this endpoint.
        #[schemars(title = "Model", schema_with = "model_schema")]
        model: String,
        /// Embedding vector dimensions; null uses the provider's default.
        #[serde(default)]
        #[schemars(title = "Embedding dimensions", schema_with = "dimension_schema")]
        dimension: Option<usize>,
    },
    /// A self-hosted or remote OpenAI-compatible endpoint.
    #[schemars(title = "OpenAI-compatible endpoint")]
    OpenAiCompatible {
        /// Absolute HTTP(S) API base URL without credentials, query, or fragment.
        #[schemars(title = "Base URL", schema_with = "endpoint_schema")]
        base_url: String,
        /// Model identifier served by this endpoint.
        #[schemars(title = "Model", schema_with = "model_schema")]
        model: String,
        /// Identifier in the host's secret store, never an API key or password.
        #[serde(default)]
        #[schemars(
            title = "Credential reference",
            schema_with = "optional_credential_schema"
        )]
        credential_ref: Option<String>,
        /// Embedding vector dimensions; null uses the provider's default.
        #[serde(default)]
        #[schemars(title = "Embedding dimensions", schema_with = "dimension_schema")]
        dimension: Option<usize>,
    },
    /// The hosted OpenAI service.
    #[schemars(title = "OpenAI")]
    OpenAi {
        /// Model identifier served by OpenAI.
        #[schemars(title = "Model", schema_with = "model_schema")]
        model: String,
        /// Identifier in the host's secret store, never an API key or password.
        #[schemars(title = "Credential reference", schema_with = "credential_schema")]
        credential_ref: String,
        /// Embedding vector dimensions; null uses the provider's default.
        #[serde(default)]
        #[schemars(title = "Embedding dimensions", schema_with = "dimension_schema")]
        dimension: Option<usize>,
    },
    /// The hosted Anthropic conversation service.
    #[schemars(title = "Anthropic")]
    Anthropic {
        /// Model identifier served by Anthropic.
        #[schemars(title = "Model", schema_with = "model_schema")]
        model: String,
        /// Identifier in the host's secret store, never an API key or password.
        #[schemars(title = "Credential reference", schema_with = "credential_schema")]
        credential_ref: String,
    },
}

impl BackendSettings {
    pub const fn kind(&self) -> BackendKind {
        match self {
            Self::Embedded { .. } => BackendKind::Embedded,
            Self::Whisper { .. } => BackendKind::Whisper,
            Self::Ollama { .. } => BackendKind::Ollama,
            Self::OpenAiCompatible { .. } => BackendKind::OpenAiCompatible,
            Self::OpenAi { .. } => BackendKind::OpenAi,
            Self::Anthropic { .. } => BackendKind::Anthropic,
        }
    }
}

/// Host-offered backends. These declarations may restrict, but never enable,
/// native features absent from the compiled runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsCapabilities {
    pub allowed_backends: BTreeSet<BackendKind>,
    pub embedded_available: bool,
    pub whisper_available: bool,
}

impl Default for SettingsCapabilities {
    fn default() -> Self {
        Self {
            allowed_backends: BackendKind::ALL.into_iter().collect(),
            embedded_available: cfg!(feature = "llm-local"),
            whisper_available: cfg!(feature = "media"),
        }
    }
}

impl SettingsCapabilities {
    pub fn allows(&self, backend: BackendKind) -> bool {
        self.allowed_backends.contains(&backend)
            && match backend {
                BackendKind::Embedded => self.embedded_available && cfg!(feature = "llm-local"),
                BackendKind::Whisper => self.whisper_available && cfg!(feature = "media"),
                _ => true,
            }
    }
}

/// Pure host authorization for syntactically valid endpoint URLs. Returning an
/// error denies the endpoint. This is not a substitute for transport enforcement.
pub type EndpointAuthorizer =
    dyn Fn(ModelRole, BackendKind, &Url) -> Result<(), String> + Send + Sync;

#[derive(Clone, Default)]
pub struct SettingsPolicy {
    pub capabilities: SettingsCapabilities,
    endpoint_authorizer: Option<Arc<EndpointAuthorizer>>,
}

impl fmt::Debug for SettingsPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsPolicy")
            .field("capabilities", &self.capabilities)
            .field(
                "has_endpoint_authorizer",
                &self.endpoint_authorizer.is_some(),
            )
            .finish()
    }
}

impl SettingsPolicy {
    pub fn new(capabilities: SettingsCapabilities) -> Self {
        Self {
            capabilities,
            endpoint_authorizer: None,
        }
    }

    pub fn with_endpoint_authorizer(
        mut self,
        authorize: impl Fn(ModelRole, BackendKind, &Url) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.endpoint_authorizer = Some(Arc::new(authorize));
        self
    }

    /// Generated UI constraints cannot encode arbitrary URL authorization.
    /// The extension indicates when runtime host authorization is also required.
    pub fn schema(&self) -> RootSchema {
        let mut schema = settings_schema(&self.capabilities);
        schema.schema.extensions.insert(
            "x-endpoint-authorization-required".into(),
            self.endpoint_authorizer.is_some().into(),
        );
        schema
    }
}

/// A field-specific validation failure. Paths use dotted JSON property names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{path}: {message}")]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

/// All independent failures found during a validation pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationErrors {
    pub errors: Vec<ValidationError>,
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid AI settings")?;
        for error in &self.errors {
            write!(f, "; {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

impl From<ValidationErrors> for RuntimeError {
    fn from(error: ValidationErrors) -> Self {
        Self::Other(error.to_string())
    }
}

/// Validate even disabled settings so forged configurations cannot bypass policy.
/// This performs no credential lookup, persistence, filesystem, or network I/O.
pub fn validate_settings(
    settings: &RuntimeSettings,
    policy: &SettingsPolicy,
) -> Result<(), ValidationErrors> {
    let mut errors = Vec::new();
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        push_error(
            &mut errors,
            "schema_version",
            format!("Supported schema version is {SETTINGS_SCHEMA_VERSION}"),
        );
    }
    for role in ModelRole::ALL {
        let Some(backend) = settings.backend(role) else {
            continue;
        };
        let kind = backend.kind();
        let field = |name: &str| format!("{}.{}", role.as_str(), name);
        if !role.supports(kind) {
            push_error(
                &mut errors,
                field("backend"),
                "Backend does not support this model role",
            );
        }
        if !policy.capabilities.allows(kind) {
            push_error(
                &mut errors,
                field("backend"),
                "Backend is not offered by this host or compiled runtime",
            );
        }
        match backend {
            BackendSettings::Embedded {
                model,
                models_dir,
                context_size,
                gpu_layers,
            } => {
                validate_native_fields(model, models_dir, role, &mut errors);
                if let Err(error) = safe_context_size(*context_size) {
                    push_error(&mut errors, field("context_size"), error.to_string());
                }
                if let Err(error) = safe_gpu_layers(*gpu_layers) {
                    push_error(&mut errors, field("gpu_layers"), error.to_string());
                }
            }
            BackendSettings::Whisper {
                model,
                models_dir,
                language,
            } => {
                validate_native_fields(model, models_dir, role, &mut errors);
                if let Some(language) = language {
                    validate_text(
                        language,
                        MAX_LANGUAGE_LENGTH,
                        field("language"),
                        &mut errors,
                    );
                }
            }
            BackendSettings::Ollama {
                base_url,
                model,
                dimension,
            }
            | BackendSettings::OpenAiCompatible {
                base_url,
                model,
                dimension,
                ..
            } => {
                validate_endpoint(base_url, role, kind, policy, &mut errors);
                validate_text(model, MAX_MODEL_LENGTH, field("model"), &mut errors);
                validate_dimension(*dimension, field("dimension"), &mut errors);
            }
            BackendSettings::OpenAi {
                model, dimension, ..
            } => {
                validate_text(model, MAX_MODEL_LENGTH, field("model"), &mut errors);
                validate_dimension(*dimension, field("dimension"), &mut errors);
            }
            BackendSettings::Anthropic { model, .. } => {
                validate_text(model, MAX_MODEL_LENGTH, field("model"), &mut errors);
            }
        }
        let credential_ref = match backend {
            BackendSettings::OpenAiCompatible { credential_ref, .. } => credential_ref.as_deref(),
            BackendSettings::OpenAi { credential_ref, .. }
            | BackendSettings::Anthropic { credential_ref, .. } => Some(credential_ref.as_str()),
            _ => None,
        };
        if let Some(reference) = credential_ref {
            if !validate_text(
                reference,
                MAX_CREDENTIAL_REF_LENGTH,
                field("credential_ref"),
                &mut errors,
            ) {
                continue;
            }
            if !reference.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !reference
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c))
            {
                push_error(
                    &mut errors,
                    field("credential_ref"),
                    "Use a secret-store identifier: start with an ASCII letter or digit, then use letters, digits, '.', '_', ':', '/', or '-'; never enter a secret value",
                );
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationErrors { errors })
    }
}

fn push_error(
    errors: &mut Vec<ValidationError>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(ValidationError {
        path: path.into(),
        message: message.into(),
    });
}

fn validate_text(
    value: &str,
    max_length: u32,
    path: String,
    errors: &mut Vec<ValidationError>,
) -> bool {
    if value.trim().is_empty()
        || value.chars().count() > max_length as usize
        || value.chars().any(char::is_control)
    {
        push_error(
            errors,
            path,
            format!("Must contain non-whitespace text, no control characters, and at most {max_length} characters"),
        );
        false
    } else {
        true
    }
}

fn validate_native_fields(
    model: &Option<String>,
    models_dir: &Option<PathBuf>,
    role: ModelRole,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(model) = model {
        validate_text(
            model,
            MAX_MODEL_LENGTH,
            format!("{}.model", role.as_str()),
            errors,
        );
    }
    if let Some(path) = models_dir {
        let field = format!("{}.models_dir", role.as_str());
        match path.to_str() {
            Some(value) => {
                validate_text(value, MAX_PATH_LENGTH, field, errors);
            }
            None => push_error(
                errors,
                field,
                "Must be a UTF-8 path for serializable settings",
            ),
        }
    }
}

fn validate_dimension(dimension: Option<usize>, path: String, errors: &mut Vec<ValidationError>) {
    if dimension == Some(0) {
        push_error(errors, path, "Embedding dimensions must be positive");
    }
}

fn validate_endpoint(
    value: &str,
    role: ModelRole,
    kind: BackendKind,
    policy: &SettingsPolicy,
    errors: &mut Vec<ValidationError>,
) {
    let field = format!("{}.base_url", role.as_str());
    if !validate_text(value, MAX_ENDPOINT_LENGTH, field.clone(), errors) {
        return;
    }
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    let parsed = Url::parse(value).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && !url.cannot_be_a_base()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && authority.is_some_and(|authority| !authority.is_empty() && !authority.contains('@'))
            && !value.contains('\\')
            && !value.chars().any(char::is_whitespace)
    });
    let Some(url) = parsed else {
        push_error(
            errors,
            field,
            "Use an absolute HTTP(S) base URL with a host and no userinfo, query, fragment, whitespace, or backslashes",
        );
        return;
    };
    if let Some(authorize) = &policy.endpoint_authorizer {
        if let Err(message) = authorize(role, kind, &url) {
            push_error(errors, field, message);
        }
    }
}

/// Generate fields from the serializable DTO and narrow its derived backend
/// alternatives for each role and host capability. No separate form is maintained.
/// Runtime validation is still mandatory, especially for URL authorization.
pub fn settings_schema(capabilities: &SettingsCapabilities) -> RootSchema {
    let mut schema = schemars::schema_for!(RuntimeSettings);
    let backend_schema = schema
        .definitions
        .remove("BackendSettings")
        .expect("RuntimeSettings derives a BackendSettings definition");
    let alternatives = backend_schema
        .into_object()
        .subschemas
        .and_then(|subschemas| subschemas.one_of)
        .expect("the tagged BackendSettings enum derives oneOf");
    let properties = &mut schema
        .schema
        .object
        .as_mut()
        .expect("RuntimeSettings is an object")
        .properties;
    for role in ModelRole::ALL {
        let field = properties
            .get_mut(role.as_str())
            .expect("every model role has a settings field");
        let metadata = field.clone().into_object().metadata;
        let mut offered = alternatives
            .iter()
            .filter(|schema| {
                let Some(tag) = backend_tag(schema) else {
                    return false;
                };
                BackendKind::ALL.into_iter().any(|kind| {
                    tag == kind.as_str() && role.supports(kind) && capabilities.allows(kind)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        offered.push(<()>::json_schema(&mut SchemaGenerator::default()));
        *field = SchemaObject {
            metadata,
            subschemas: Some(Box::new(SubschemaValidation {
                one_of: Some(offered),
                ..Default::default()
            })),
            ..Default::default()
        }
        .into();
    }
    schema
}

fn backend_tag(schema: &Schema) -> Option<&str> {
    match schema {
        Schema::Object(schema) => {
            schema
                .object
                .as_ref()?
                .properties
                .get("backend")
                .and_then(|tag| match tag {
                    Schema::Object(tag) => tag.enum_values.as_ref()?.first()?.as_str(),
                    Schema::Bool(_) => None,
                })
        }
        Schema::Bool(_) => None,
    }
}

fn text_schema<T: JsonSchema>(generator: &mut SchemaGenerator, max_length: u32) -> SchemaObject {
    let mut schema = T::json_schema(generator).into_object();
    let validation = schema.string();
    validation.min_length = Some(1);
    validation.max_length = Some(max_length);
    validation.pattern = Some(TEXT_PATTERN.into());
    schema
}

fn version_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = u32::json_schema(generator).into_object();
    schema.const_value = Some(SETTINGS_SCHEMA_VERSION.into());
    schema.into()
}

fn model_schema(generator: &mut SchemaGenerator) -> Schema {
    text_schema::<String>(generator, MAX_MODEL_LENGTH).into()
}

fn optional_model_schema(generator: &mut SchemaGenerator) -> Schema {
    text_schema::<Option<String>>(generator, MAX_MODEL_LENGTH).into()
}

fn path_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = text_schema::<Option<PathBuf>>(generator, MAX_PATH_LENGTH);
    schema.format = Some("path".into());
    schema
        .extensions
        .insert("x-path-kind".into(), "directory".into());
    schema.into()
}

fn language_schema(generator: &mut SchemaGenerator) -> Schema {
    text_schema::<Option<String>>(generator, MAX_LANGUAGE_LENGTH).into()
}

fn endpoint_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = text_schema::<String>(generator, MAX_ENDPOINT_LENGTH);
    schema.format = Some("uri".into());
    // JSON Schema's URI format alone also permits non-HTTP and opaque URIs.
    schema.string().pattern =
        Some(r"^[Hh][Tt][Tt][Pp][Ss]?://[^/?#@\s\\]+(?:/[^?#\s\\]*)?$".into());
    schema.into()
}

fn credential_reference_schema<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = text_schema::<T>(generator, MAX_CREDENTIAL_REF_LENGTH);
    schema.string().pattern = Some(CREDENTIAL_REF_PATTERN.into());
    schema.format = Some("password-reference".into());
    schema
        .extensions
        .insert("x-credential-reference".into(), true.into());
    schema.into()
}

fn credential_schema(generator: &mut SchemaGenerator) -> Schema {
    credential_reference_schema::<String>(generator)
}

fn optional_credential_schema(generator: &mut SchemaGenerator) -> Schema {
    credential_reference_schema::<Option<String>>(generator)
}

fn context_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<u32>::json_schema(generator).into_object();
    schema.number().minimum = Some(1.0);
    schema.number().maximum = Some(f64::from(MAX_CONTEXT_SIZE));
    schema.extensions.insert(
        "x-effective-default".into(),
        safe_context_size(None)
            .expect("the runtime default context is safe")
            .into(),
    );
    schema.into()
}

fn gpu_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<u32>::json_schema(generator).into_object();
    schema.number().maximum = Some(f64::from(i32::MAX));
    schema.extensions.insert(
        "x-effective-default".into(),
        safe_gpu_layers(None)
            .expect("the runtime default GPU count is safe")
            .into(),
    );
    schema.into()
}

fn dimension_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<usize>::json_schema(generator).into_object();
    schema.number().minimum = Some(1.0);
    schema.into()
}

/// Host-injected storage. `None` means no persisted settings.
///
/// `save` MUST be atomic: failure must leave the previous persisted value intact.
/// Implementations must not resolve credential references or persist secret values.
pub trait SettingsStore: Send + Sync {
    fn load(&self) -> crate::Result<Option<RuntimeSettings>>;
    fn save(&self, settings: &RuntimeSettings) -> crate::Result<()>;
}

impl<T: SettingsStore + ?Sized> SettingsStore for Arc<T> {
    fn load(&self) -> crate::Result<Option<RuntimeSettings>> {
        (**self).load()
    }

    fn save(&self, settings: &RuntimeSettings) -> crate::Result<()> {
        (**self).save(settings)
    }
}

impl<T: SettingsStore + ?Sized> SettingsStore for Box<T> {
    fn load(&self) -> crate::Result<Option<RuntimeSettings>> {
        (**self).load()
    }

    fn save(&self, settings: &RuntimeSettings) -> crate::Result<()> {
        (**self).save(settings)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
    #[error("Settings storage failed: {0}")]
    Store(#[source] RuntimeError),
}

impl From<SettingsError> for RuntimeError {
    fn from(error: SettingsError) -> Self {
        match error {
            SettingsError::Validation(error) => error.into(),
            SettingsError::Store(error) => error,
        }
    }
}

/// Validated settings state, not provider state.
///
/// Construction, reads, schema generation, and validation do not touch storage.
/// `load` explicitly reads; `persist` validates then atomically saves before
/// replacing the in-memory value. Failed loads/saves preserve the previous value.
/// A model manager must prepare bindings before `persist`, then switch only after
/// success. This controller intentionally does not construct or probe providers.
pub struct SettingsController<S: SettingsStore> {
    store: S,
    policy: SettingsPolicy,
    settings: RuntimeSettings,
}

impl<S: SettingsStore> SettingsController<S> {
    pub fn new(store: S, policy: SettingsPolicy) -> Self {
        Self {
            store,
            policy,
            settings: RuntimeSettings::default(),
        }
    }

    pub fn load(&mut self) -> Result<&RuntimeSettings, SettingsError> {
        let loaded = self
            .store
            .load()
            .map_err(SettingsError::Store)?
            .unwrap_or_default();
        self.validate(&loaded)?;
        self.settings = loaded;
        Ok(&self.settings)
    }

    pub fn validate(&self, settings: &RuntimeSettings) -> Result<(), ValidationErrors> {
        validate_settings(settings, &self.policy)
    }

    pub fn persist(
        &mut self,
        settings: RuntimeSettings,
    ) -> Result<&RuntimeSettings, SettingsError> {
        self.validate(&settings)?;
        self.store.save(&settings).map_err(SettingsError::Store)?;
        self.settings = settings;
        Ok(&self.settings)
    }

    pub fn read(&self) -> &RuntimeSettings {
        &self.settings
    }

    pub fn policy(&self) -> &SettingsPolicy {
        &self.policy
    }

    pub fn schema(&self) -> RootSchema {
        self.policy.schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn ollama() -> BackendSettings {
        BackendSettings::Ollama {
            base_url: "http://localhost:11434".into(),
            model: "host-model".into(),
            dimension: None,
        }
    }

    fn configured() -> RuntimeSettings {
        RuntimeSettings {
            enabled: true,
            chat: Some(ollama()),
            ..Default::default()
        }
    }

    fn errors(settings: &RuntimeSettings) -> Vec<ValidationError> {
        settings
            .validate(&SettingsPolicy::default())
            .unwrap_err()
            .errors
    }

    fn has_error(settings: &RuntimeSettings, path: &str) -> bool {
        errors(settings).iter().any(|error| error.path == path)
    }

    #[test]
    fn defaults_and_round_trip_preserve_absent_models_and_credential_references() {
        let defaults: RuntimeSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(defaults, RuntimeSettings::default());
        assert!(!defaults.enabled);
        assert_eq!(defaults.schema_version, 1);
        defaults.validate(&SettingsPolicy::default()).unwrap();
        RuntimeSettings {
            enabled: true,
            ..Default::default()
        }
        .validate(&SettingsPolicy::default())
        .unwrap();

        for backend in [
            json!({"backend": "embedded"}),
            json!({"backend": "whisper"}),
            json!({"backend": "ollama", "base_url": "http://localhost:11434", "model": "test"}),
            json!({"backend": "open-ai-compatible", "base_url": "https://server.invalid/v1", "model": "test", "credential_ref": "vault:ai/test"}),
            json!({"backend": "open-ai", "model": "test", "credential_ref": "vault:ai/openai"}),
            json!({"backend": "anthropic", "model": "test", "credential_ref": "vault:ai/anthropic"}),
        ] {
            let settings: RuntimeSettings =
                serde_json::from_value(json!({"chat": backend})).unwrap();
            let serialized = serde_json::to_value(&settings).unwrap();
            assert_eq!(
                settings,
                serde_json::from_value::<RuntimeSettings>(serialized.clone()).unwrap()
            );
            assert_eq!(
                serialized["chat"]["backend"],
                settings.chat.as_ref().unwrap().kind().as_str()
            );
        }
    }

    #[test]
    fn rejects_unknown_fields_and_plaintext_credential_fields() {
        for value in [
            json!({"api_key": "not-allowed"}),
            json!({"chat": {"backend": "ollama", "base_url": "http://localhost", "model": "test", "api_key": "not-allowed"}}),
            json!({"chat": {"backend": "open-ai", "model": "test", "credential_ref": "vault:key", "api_key": "not-allowed"}}),
            json!({"chat": {"backend": "embedded", "context_sze": 1}}),
            json!({"chat": {"backend": "open-ai", "model": "test"}}),
        ] {
            assert!(serde_json::from_value::<RuntimeSettings>(value).is_err());
        }
    }

    #[test]
    fn rejects_unsupported_schema_versions() {
        for schema_version in [0, 2, u32::MAX] {
            assert!(has_error(
                &RuntimeSettings {
                    schema_version,
                    ..Default::default()
                },
                "schema_version"
            ));
        }
    }

    #[test]
    fn rejects_role_mismatches_even_when_disabled() {
        let settings = RuntimeSettings {
            chat: Some(BackendSettings::Whisper {
                model: None,
                models_dir: None,
                language: None,
            }),
            embeddings: Some(BackendSettings::Anthropic {
                model: "test".into(),
                credential_ref: "vault:anthropic".into(),
            }),
            transcription: Some(ollama()),
            ..Default::default()
        };
        for path in [
            "chat.backend",
            "embeddings.backend",
            "transcription.backend",
        ] {
            assert!(has_error(&settings, path));
        }
        for role in ModelRole::ALL {
            for kind in BackendKind::ALL {
                let expected = match role {
                    ModelRole::Chat => kind != BackendKind::Whisper,
                    ModelRole::Embeddings => {
                        kind != BackendKind::Whisper && kind != BackendKind::Anthropic
                    }
                    ModelRole::Transcription => kind == BackendKind::Whisper,
                };
                assert_eq!(role.supports(kind), expected);
            }
        }
    }

    #[test]
    fn accepts_supported_remote_roles_and_optional_credential_references() {
        let policy = SettingsPolicy::default();
        for role in [ModelRole::Chat, ModelRole::Embeddings] {
            for backend in [
                ollama(),
                BackendSettings::OpenAiCompatible {
                    base_url: "https://example.invalid/v1".into(),
                    model: "test".into(),
                    credential_ref: None,
                    dimension: Some(1),
                },
                BackendSettings::OpenAiCompatible {
                    base_url: "https://example.invalid/v1".into(),
                    model: "test".into(),
                    credential_ref: Some("vault:ai/model".into()),
                    dimension: None,
                },
                BackendSettings::OpenAi {
                    model: "test".into(),
                    credential_ref: "vault:ai/model".into(),
                    dimension: Some(1),
                },
            ] {
                let mut settings = RuntimeSettings::default();
                match role {
                    ModelRole::Chat => settings.chat = Some(backend),
                    ModelRole::Embeddings => settings.embeddings = Some(backend),
                    ModelRole::Transcription => unreachable!(),
                }
                settings.validate(&policy).unwrap();
            }
        }
        RuntimeSettings {
            chat: Some(BackendSettings::Anthropic {
                model: "test".into(),
                credential_ref: "vault:ai/model".into(),
            }),
            ..Default::default()
        }
        .validate(&policy)
        .unwrap();
    }

    #[test]
    fn native_limits_match_resource_admission_and_schema_boundaries() {
        assert!(safe_context_size(Some(MAX_CONTEXT_SIZE)).is_ok());
        assert!(safe_context_size(Some(MAX_CONTEXT_SIZE + 1)).is_err());
        for (context_size, gpu_layers) in [(0, 0), (MAX_CONTEXT_SIZE + 1, u32::MAX)] {
            let settings = RuntimeSettings {
                chat: Some(BackendSettings::Embedded {
                    model: None,
                    models_dir: None,
                    context_size: Some(context_size),
                    gpu_layers: Some(gpu_layers),
                }),
                ..Default::default()
            };
            assert!(has_error(&settings, "chat.context_size"));
            if gpu_layers != 0 {
                assert!(has_error(&settings, "chat.gpu_layers"));
            }
        }
        let settings = RuntimeSettings {
            chat: Some(BackendSettings::Embedded {
                model: None,
                models_dir: Some(PathBuf::from("nonexistent-model-directory")),
                context_size: Some(MAX_CONTEXT_SIZE),
                gpu_layers: Some(i32::MAX as u32),
            }),
            ..Default::default()
        };
        let result = settings.validate(&SettingsPolicy::default());
        assert!(
            result.is_ok()
                || result
                    .unwrap_err()
                    .errors
                    .iter()
                    .all(|e| e.path == "chat.backend")
        );
    }

    #[test]
    fn validates_model_ref_dimension_language_and_path_fields() {
        let settings = RuntimeSettings {
            chat: Some(BackendSettings::OpenAi {
                model: " ".into(),
                credential_ref: "Bearer a-secret".into(),
                dimension: Some(0),
            }),
            embeddings: Some(BackendSettings::Embedded {
                model: Some("x".repeat(MAX_MODEL_LENGTH as usize + 1)),
                models_dir: Some(PathBuf::from("")),
                context_size: None,
                gpu_layers: None,
            }),
            transcription: Some(BackendSettings::Whisper {
                model: Some("bad\0model".into()),
                models_dir: Some(PathBuf::from("x".repeat(MAX_PATH_LENGTH as usize + 1))),
                language: Some("x".repeat(MAX_LANGUAGE_LENGTH as usize + 1)),
            }),
            ..Default::default()
        };
        for path in [
            "chat.model",
            "chat.credential_ref",
            "chat.dimension",
            "embeddings.model",
            "embeddings.models_dir",
            "transcription.model",
            "transcription.models_dir",
            "transcription.language",
        ] {
            assert!(has_error(&settings, path), "{path}");
        }
        for reference in [
            "",
            "   ",
            "\n",
            &"x".repeat(MAX_CREDENTIAL_REF_LENGTH as usize + 1),
        ] {
            let settings = RuntimeSettings {
                chat: Some(BackendSettings::OpenAi {
                    model: "test".into(),
                    credential_ref: reference.into(),
                    dimension: Some(1),
                }),
                ..Default::default()
            };
            assert!(has_error(&settings, "chat.credential_ref"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_paths_that_cannot_round_trip_through_json() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let settings = RuntimeSettings {
            chat: Some(BackendSettings::Embedded {
                model: None,
                models_dir: Some(PathBuf::from(OsString::from_vec(vec![0xff]))),
                context_size: None,
                gpu_layers: None,
            }),
            ..Default::default()
        };
        assert!(has_error(&settings, "chat.models_dir"));
    }

    #[test]
    fn validates_absolute_http_endpoints_without_embedded_auth() {
        for base_url in [
            "",
            "/relative",
            "localhost:11434",
            "ftp://host",
            "file:///model",
            "http:localhost",
            "https:///host",
            "https://",
            "https://host:bad",
            "https://user:password@host",
            "https://user@host",
            "https://@host",
            "https://host?key=secret",
            "https://host?",
            "https://host#fragment",
            "https://host#",
            " https://host",
            "https://host/a b",
            "https://host\\@other",
            "https://host/\npath",
        ] {
            for compatible in [false, true] {
                let mut settings = configured();
                settings.chat = Some(if compatible {
                    BackendSettings::OpenAiCompatible {
                        base_url: base_url.into(),
                        model: "test".into(),
                        credential_ref: None,
                        dimension: None,
                    }
                } else {
                    BackendSettings::Ollama {
                        base_url: base_url.into(),
                        model: "test".into(),
                        dimension: None,
                    }
                });
                assert!(has_error(&settings, "chat.base_url"), "{base_url:?}");
            }
        }
        for base_url in [
            "http://localhost:11434",
            "https://example.invalid/v1/",
            "HTTP://[::1]:8080/v1",
            "https://example.invalid/base%20path",
        ] {
            let mut settings = configured();
            if let Some(BackendSettings::Ollama { base_url: url, .. }) = &mut settings.chat {
                *url = base_url.into();
            }
            settings.validate(&SettingsPolicy::default()).unwrap();
        }
        let mut settings = configured();
        if let Some(BackendSettings::Ollama { base_url, .. }) = &mut settings.chat {
            *base_url = format!(
                "https://host.invalid/{}",
                "x".repeat(MAX_ENDPOINT_LENGTH as usize)
            );
        }
        assert!(has_error(&settings, "chat.base_url"));
    }

    fn tags(schema: &Value, role: &str) -> Vec<String> {
        schema["properties"][role]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|variant| {
                variant["properties"]["backend"]["enum"][0]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    #[test]
    fn restrictive_policy_rejects_forged_providers_and_hides_them_from_schema() {
        let policy = SettingsPolicy::new(SettingsCapabilities {
            allowed_backends: [BackendKind::Ollama].into_iter().collect(),
            embedded_available: false,
            whisper_available: false,
        });
        let forged = RuntimeSettings {
            chat: Some(BackendSettings::OpenAi {
                model: "test".into(),
                credential_ref: "vault:test".into(),
                dimension: None,
            }),
            ..Default::default()
        };
        let errors = forged.validate(&policy).unwrap_err();
        assert_eq!(errors.errors[0].path, "chat.backend");
        let schema = serde_json::to_value(policy.schema()).unwrap();
        assert_eq!(tags(&schema, "chat"), ["ollama"]);
        assert_eq!(tags(&schema, "embeddings"), ["ollama"]);
        assert!(tags(&schema, "transcription").is_empty());
        assert!(schema["definitions"]["BackendSettings"].is_null());
        assert!(!schema.to_string().contains("\"open-ai\""));
        configured().validate(&policy).unwrap();
    }

    #[test]
    fn feature_and_role_constraints_are_reflected_in_schema() {
        let capabilities = SettingsCapabilities {
            embedded_available: true,
            whisper_available: true,
            ..Default::default()
        };
        let schema = serde_json::to_value(settings_schema(&capabilities)).unwrap();
        for role in ModelRole::ALL {
            let offered = tags(&schema, role.as_str());
            for kind in BackendKind::ALL {
                assert_eq!(
                    offered.iter().any(|tag| tag == kind.as_str()),
                    capabilities.allows(kind) && role.supports(kind)
                );
            }
        }
        assert_eq!(
            capabilities.allows(BackendKind::Embedded),
            cfg!(feature = "llm-local")
        );
        assert_eq!(
            capabilities.allows(BackendKind::Whisper),
            cfg!(feature = "media")
        );
    }

    #[test]
    fn endpoint_authorization_remains_runtime_enforced_without_hosted_providers() {
        let policy = SettingsPolicy::new(SettingsCapabilities {
            allowed_backends: [BackendKind::Ollama].into_iter().collect(),
            ..Default::default()
        })
        .with_endpoint_authorizer(|role, kind, url| {
            assert_eq!(role, ModelRole::Chat);
            assert_eq!(kind, BackendKind::Ollama);
            if url.host_str() == Some("localhost") {
                Ok(())
            } else {
                Err("Endpoint not authorized by host".into())
            }
        });
        configured().validate(&policy).unwrap();
        let mut remote = configured();
        if let Some(BackendSettings::Ollama { base_url, .. }) = &mut remote.chat {
            *base_url = "https://remote.invalid".into();
        }
        let errors = remote.validate(&policy).unwrap_err();
        assert_eq!(errors.errors[0].path, "chat.base_url");
        assert_eq!(errors.errors[0].message, "Endpoint not authorized by host");
        assert_eq!(
            serde_json::to_value(policy.schema()).unwrap()["x-endpoint-authorization-required"],
            true
        );
    }

    #[test]
    fn schema_has_defaults_constraints_and_reference_not_plaintext_fields() {
        let schema =
            serde_json::to_value(settings_schema(&SettingsCapabilities::default())).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["schema_version"]["default"], 1);
        assert_eq!(schema["properties"]["schema_version"]["const"], 1);
        assert_eq!(schema["properties"]["enabled"]["default"], false);
        for role in ModelRole::ALL {
            assert_eq!(
                schema["properties"][role.as_str()].get("default"),
                Some(&Value::Null)
            );
            assert!(schema["properties"][role.as_str()]["title"].is_string());
        }
        let backends = serde_json::to_value(schemars::schema_for!(BackendSettings)).unwrap();
        for variant in backends["oneOf"].as_array().unwrap() {
            assert_eq!(variant["additionalProperties"], false);
            let fields = &variant["properties"];
            assert!(fields.get("api_key").is_none());
            assert!(fields.get("password").is_none());
            assert!(variant["title"].is_string());
            if let Some(model) = fields.get("model") {
                assert_eq!(model["maxLength"], MAX_MODEL_LENGTH);
                assert_eq!(model["minLength"], 1);
                assert!(model["description"].is_string());
            }
            if let Some(reference) = fields.get("credential_ref") {
                assert_eq!(reference["format"], "password-reference");
                assert_eq!(reference["x-credential-reference"], true);
                assert_eq!(reference["maxLength"], MAX_CREDENTIAL_REF_LENGTH);
            }
            if let Some(endpoint) = fields.get("base_url") {
                assert_eq!(endpoint["format"], "uri");
                assert_eq!(endpoint["maxLength"], MAX_ENDPOINT_LENGTH);
                assert!(endpoint["pattern"].is_string());
            }
            if let Some(path) = fields.get("models_dir") {
                assert_eq!(path["format"], "path");
                assert_eq!(path["x-path-kind"], "directory");
            }
            if let Some(context) = fields.get("context_size") {
                assert_eq!(context["minimum"], 1.0);
                assert_eq!(context["maximum"], f64::from(MAX_CONTEXT_SIZE));
                assert_eq!(context["default"], Value::Null);
                assert_eq!(
                    context["x-effective-default"],
                    safe_context_size(None).unwrap()
                );
            }
            if let Some(gpu) = fields.get("gpu_layers") {
                assert_eq!(gpu["maximum"], f64::from(i32::MAX));
                assert_eq!(gpu["x-effective-default"], safe_gpu_layers(None).unwrap());
            }
            if let Some(dimension) = fields.get("dimension") {
                assert_eq!(dimension["minimum"], 1.0);
            }
        }
    }

    #[derive(Default)]
    struct FakeStore {
        value: Mutex<Option<RuntimeSettings>>,
        loads: AtomicUsize,
        saves: AtomicUsize,
        fail_load: AtomicBool,
        fail_save: AtomicBool,
    }

    impl SettingsStore for FakeStore {
        fn load(&self) -> crate::Result<Option<RuntimeSettings>> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if self.fail_load.load(Ordering::SeqCst) {
                Err(RuntimeError::Other("load failed".into()))
            } else {
                Ok(self.value.lock().unwrap().clone())
            }
        }

        fn save(&self, settings: &RuntimeSettings) -> crate::Result<()> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            if self.fail_save.load(Ordering::SeqCst) {
                Err(RuntimeError::Other("save failed".into()))
            } else {
                *self.value.lock().unwrap() = Some(settings.clone());
                Ok(())
            }
        }
    }

    #[test]
    fn failed_validation_or_atomic_save_preserves_previous_settings() {
        let store = Arc::new(FakeStore::default());
        let mut controller = SettingsController::new(store.clone(), SettingsPolicy::default());
        let prior = configured();
        controller.persist(prior.clone()).unwrap();
        let invalid = RuntimeSettings {
            schema_version: 99,
            ..Default::default()
        };
        assert!(matches!(
            controller.persist(invalid),
            Err(SettingsError::Validation(_))
        ));
        assert_eq!(store.saves.load(Ordering::SeqCst), 1);
        assert_eq!(controller.read(), &prior);
        store.fail_save.store(true, Ordering::SeqCst);
        assert!(matches!(
            controller.persist(RuntimeSettings::default()),
            Err(SettingsError::Store(RuntimeError::Other(message))) if message == "save failed"
        ));
        assert_eq!(controller.read(), &prior);
        assert_eq!(*store.value.lock().unwrap(), Some(prior));
    }

    #[test]
    fn explicit_load_validates_and_preserves_previous_value_on_failure() {
        let store = Arc::new(FakeStore::default());
        let mut controller = SettingsController::new(store.clone(), SettingsPolicy::default());
        assert_eq!(controller.load().unwrap(), &RuntimeSettings::default());
        assert_eq!(store.saves.load(Ordering::SeqCst), 0);
        *store.value.lock().unwrap() = Some(configured());
        assert_eq!(controller.load().unwrap(), &configured());
        store.fail_load.store(true, Ordering::SeqCst);
        assert!(matches!(controller.load(), Err(SettingsError::Store(_))));
        assert_eq!(controller.read(), &configured());
        store.fail_load.store(false, Ordering::SeqCst);
        *store.value.lock().unwrap() = Some(RuntimeSettings {
            schema_version: 0,
            ..Default::default()
        });
        assert!(matches!(
            controller.load(),
            Err(SettingsError::Validation(_))
        ));
        assert_eq!(controller.read(), &configured());
    }

    #[test]
    fn construction_validation_schema_and_reads_have_no_storage_or_network_effects() {
        let store = Arc::new(FakeStore::default());
        let controller = SettingsController::new(store.clone(), SettingsPolicy::default());
        let mut settings = configured();
        if let Some(BackendSettings::Ollama { base_url, .. }) = &mut settings.chat {
            *base_url = "https://does-not-resolve.invalid:1/v1".into();
        }
        controller.validate(&settings).unwrap();
        let _ = controller.schema();
        assert_eq!(controller.read(), &RuntimeSettings::default());
        assert_eq!(store.loads.load(Ordering::SeqCst), 0);
        assert_eq!(store.saves.load(Ordering::SeqCst), 0);
        assert!(store.value.lock().unwrap().is_none());
    }
}
