//! Schema system — SuperTag equivalent for structured knowledge.
//!
//! Schemas define typed fields for tagged pages. When a page has a tag like #person,
//! the schema system knows what fields to expect (name, company, role, etc.).
//!
//! Schemas are stored as YAML files in `.grafium/schemas/` within each graph,
//! making them human-editable and git-friendly.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

/// A schema definition (equivalent to Tana SuperTag).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    /// The tag this schema applies to (e.g., "person", "meeting", "concept").
    pub tag: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Icon (emoji or icon name).
    pub icon: Option<String>,
    /// Description of what this schema represents.
    pub description: Option<String>,
    /// Fields defined for this schema.
    pub fields: Vec<SchemaField>,
    /// Template content for new pages with this tag.
    pub template: Option<String>,
    /// Whether AI should auto-classify pages into this schema.
    pub ai_auto_classify: bool,
}

/// A field in a schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaField {
    /// Field identifier (used in properties JSON).
    pub key: String,
    /// Display label.
    pub label: String,
    /// Field type.
    pub field_type: FieldType,
    /// Whether this field is required.
    pub required: bool,
    /// Default value (as JSON).
    pub default: Option<serde_json::Value>,
    /// For Select type: allowed values.
    pub options: Option<Vec<String>>,
    /// Whether AI should attempt to auto-fill this field.
    pub ai_autofill: bool,
    /// Description/help text.
    pub description: Option<String>,
}

/// Field type enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    /// Plain text.
    Text,
    /// Rich text (markdown).
    RichText,
    /// Number (integer or float).
    Number,
    /// Date (ISO 8601).
    Date,
    /// DateTime.
    DateTime,
    /// Boolean (checkbox).
    Boolean,
    /// Single select from options.
    Select,
    /// Multiple select from options.
    MultiSelect,
    /// Reference to another page (wikilink).
    Reference,
    /// Multiple references.
    References,
    /// URL.
    Url,
    /// Email address.
    Email,
    /// Tags (list of strings).
    Tags,
}

/// Schema manager — loads and provides schemas for a graph.
pub struct SchemaManager {
    schemas: HashMap<String, Schema>,
    schemas_dir: PathBuf,
}

impl SchemaManager {
    /// Load all schemas from the `.grafium/schemas/` directory.
    pub fn load(graph_root: &Path) -> Result<Self> {
        let schemas_dir = graph_root.join(".grafium").join("schemas");
        let mut schemas = HashMap::new();

        if schemas_dir.exists() {
            for entry in std::fs::read_dir(&schemas_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path
                    .extension()
                    .map_or(false, |ext| ext == "yaml" || ext == "yml")
                {
                    match Self::load_schema(&path) {
                        Ok(schema) => {
                            schemas.insert(schema.tag.clone(), schema);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load schema {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        Ok(Self {
            schemas,
            schemas_dir,
        })
    }

    /// Load a single schema from a YAML file.
    fn load_schema(path: &Path) -> Result<Schema> {
        let content = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&content)
            .map_err(|e| CoreError::Parse(format!("Schema YAML error: {}", e)))
    }

    /// Save a schema to disk.
    pub fn save_schema(&mut self, schema: Schema) -> Result<()> {
        std::fs::create_dir_all(&self.schemas_dir)?;

        let filename = format!("{}.yaml", schema.tag);
        let path = self.schemas_dir.join(&filename);
        let content = serde_yaml::to_string(&schema)
            .map_err(|e| CoreError::Other(format!("Schema serialize error: {}", e)))?;

        std::fs::write(&path, content)?;
        self.schemas.insert(schema.tag.clone(), schema);
        Ok(())
    }

    /// Delete a schema.
    pub fn delete_schema(&mut self, tag: &str) -> Result<()> {
        let filename = format!("{}.yaml", tag);
        let path = self.schemas_dir.join(&filename);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        self.schemas.remove(tag);
        Ok(())
    }

    /// Get schema for a tag.
    pub fn get(&self, tag: &str) -> Option<&Schema> {
        self.schemas.get(tag)
    }

    /// List all schemas.
    pub fn list(&self) -> Vec<&Schema> {
        self.schemas.values().collect()
    }

    /// Get schemas with AI auto-classify enabled.
    pub fn auto_classify_schemas(&self) -> Vec<&Schema> {
        self.schemas
            .values()
            .filter(|s| s.ai_auto_classify)
            .collect()
    }

    /// Create default schemas for common use cases.
    pub fn create_defaults(&mut self) -> Result<()> {
        let defaults = vec![
            Schema {
                tag: "person".to_string(),
                display_name: "Person".to_string(),
                icon: Some("👤".to_string()),
                description: Some("A person or contact".to_string()),
                fields: vec![
                    SchemaField {
                        key: "company".to_string(),
                        label: "Company".to_string(),
                        field_type: FieldType::Reference,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: true,
                        description: None,
                    },
                    SchemaField {
                        key: "role".to_string(),
                        label: "Role".to_string(),
                        field_type: FieldType::Text,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: true,
                        description: None,
                    },
                    SchemaField {
                        key: "email".to_string(),
                        label: "Email".to_string(),
                        field_type: FieldType::Email,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: false,
                        description: None,
                    },
                ],
                template: None,
                ai_auto_classify: true,
            },
            Schema {
                tag: "concept".to_string(),
                display_name: "Concept".to_string(),
                icon: Some("💡".to_string()),
                description: Some("An idea, theory, or concept".to_string()),
                fields: vec![
                    SchemaField {
                        key: "domain".to_string(),
                        label: "Domain".to_string(),
                        field_type: FieldType::Tags,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: true,
                        description: Some("Knowledge domains this concept belongs to".to_string()),
                    },
                    SchemaField {
                        key: "related_to".to_string(),
                        label: "Related To".to_string(),
                        field_type: FieldType::References,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: true,
                        description: None,
                    },
                ],
                template: None,
                ai_auto_classify: true,
            },
            Schema {
                tag: "source".to_string(),
                display_name: "Source".to_string(),
                icon: Some("📄".to_string()),
                description: Some("A book, paper, article, or other source material".to_string()),
                fields: vec![
                    SchemaField {
                        key: "author".to_string(),
                        label: "Author".to_string(),
                        field_type: FieldType::Reference,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: true,
                        description: None,
                    },
                    SchemaField {
                        key: "year".to_string(),
                        label: "Year".to_string(),
                        field_type: FieldType::Number,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: true,
                        description: None,
                    },
                    SchemaField {
                        key: "url".to_string(),
                        label: "URL".to_string(),
                        field_type: FieldType::Url,
                        required: false,
                        default: None,
                        options: None,
                        ai_autofill: false,
                        description: None,
                    },
                    SchemaField {
                        key: "source_type".to_string(),
                        label: "Type".to_string(),
                        field_type: FieldType::Select,
                        required: false,
                        default: None,
                        options: Some(vec![
                            "book".to_string(),
                            "paper".to_string(),
                            "article".to_string(),
                            "video".to_string(),
                            "podcast".to_string(),
                            "website".to_string(),
                        ]),
                        ai_autofill: true,
                        description: None,
                    },
                ],
                template: None,
                ai_auto_classify: true,
            },
        ];

        for schema in defaults {
            if !self.schemas.contains_key(&schema.tag) {
                self.save_schema(schema)?;
            }
        }

        Ok(())
    }
}
