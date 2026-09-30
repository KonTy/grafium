//! Bounded metadata inspection without loading native libraries or model tensors.

use std::collections::HashMap;
use std::io::{Read, Take};
use std::path::Path;

use crate::error::{Result, RuntimeError};

const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ITEMS: u64 = 1_000_000;
const MAX_KEYS: u64 = 65_536;

pub struct EmbeddingMetadata {
    pub context_length: u32,
    pub embedding_length: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct TransformerShape {
    pub layers: u32,
    pub embedding: u32,
    pub heads: u32,
    pub kv_heads: u32,
    pub key_length: u32,
    pub value_length: u32,
}

impl TransformerShape {
    /// Current native contexts use f16 keys and values, including GQA models.
    pub fn context_bytes(self, tokens: u32) -> Option<u64> {
        if [
            self.layers,
            self.embedding,
            self.heads,
            self.kv_heads,
            self.key_length,
            self.value_length,
        ]
        .contains(&0)
            || self.kv_heads > self.heads
        {
            return None;
        }
        let kv = u64::from(self.key_length)
            .checked_add(u64::from(self.value_length))?
            .checked_mul(u64::from(self.kv_heads))?
            .checked_mul(u64::from(self.layers))?
            .checked_mul(u64::from(tokens))?
            .checked_mul(2)?;
        // A conservative host-side scratch allowance; native fitting computes
        // the actual backend graph/tensor allocations before GPU admission.
        let scratch = u64::from(tokens.min(2048))
            .checked_mul(u64::from(self.embedding))?
            .checked_mul(u64::from(self.layers))?
            .checked_mul(8)?;
        kv.checked_add(scratch)?.checked_add(512 * 1024 * 1024)
    }
}

struct Metadata {
    architecture: String,
    numbers: HashMap<String, u32>,
    descriptions: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ModelMetadata {
    pub architecture: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CatalogMetadata {
    pub architecture: Option<String>,
    pub description: Option<String>,
}

pub fn read_catalog_metadata(path: &Path) -> Result<CatalogMetadata> {
    let metadata = inspect_metadata(path)?;
    Ok(CatalogMetadata {
        architecture: Some(metadata.architecture),
        description: metadata.description,
    })
}

const DESCRIPTION_KEYS: &[&str] = &[
    "general.name",
    "general.basename",
    "general.size_label",
    "general.finetune",
    "general.base_model.0.name",
];

/// Inspect catalog metadata without loading native code or tensor weights.
pub fn inspect_metadata(path: &Path) -> Result<ModelMetadata> {
    let metadata = parse_metadata(open_metadata(path)?)?;
    let mut values = Vec::new();
    for key in DESCRIPTION_KEYS {
        if let Some(value) = metadata
            .descriptions
            .get(*key)
            .filter(|v| !v.trim().is_empty())
        {
            if !values.contains(&value.as_str()) {
                values.push(value.as_str());
            }
        }
    }
    Ok(ModelMetadata {
        architecture: metadata.architecture,
        description: (!values.is_empty()).then(|| values.join(" · ")),
    })
}

pub fn read_transformer_shape(path: &Path) -> Result<Option<TransformerShape>> {
    let metadata = parse_metadata(open_metadata(path)?)?;
    let get = |suffix| {
        metadata
            .numbers
            .get(&format!("{}.{}", metadata.architecture, suffix))
            .copied()
    };
    let (Some(layers), Some(embedding), Some(heads)) = (
        get("block_count"),
        get("embedding_length"),
        get("attention.head_count"),
    ) else {
        return Ok(None);
    };
    if heads == 0 || embedding == 0 || embedding % heads != 0 {
        return Ok(None);
    }
    Ok(Some(TransformerShape {
        layers,
        embedding,
        heads,
        kv_heads: get("attention.head_count_kv").unwrap_or(heads),
        key_length: get("attention.key_length").unwrap_or(embedding / heads),
        value_length: get("attention.value_length").unwrap_or(embedding / heads),
    }))
}

pub fn read_embedding_metadata(path: &Path) -> Result<EmbeddingMetadata> {
    parse(open_metadata(path)?).map_err(|error| {
        RuntimeError::Other(format!(
            "Invalid embedding model metadata in {}: {error}",
            path.display()
        ))
    })
}

/// Tokenizer arrays hold hundreds of thousands of tiny values; buffering turns
/// what would be a syscall per value into a handful of large reads.
fn open_metadata(path: &Path) -> Result<Take<std::io::BufReader<std::fs::File>>> {
    Ok(
        std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?)
            .take(MAX_METADATA_BYTES),
    )
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError::Other(message.into())
}

fn u32_value(reader: &mut impl Read) -> Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn u64_value(reader: &mut impl Read) -> Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn string(reader: &mut impl Read) -> Result<String> {
    let size = u64_value(reader)?;
    if size > 4096 {
        return Err(invalid("metadata key or architecture is too long"));
    }
    let mut bytes = vec![0; size as usize];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| invalid("metadata string is not UTF-8"))
}

fn skip(reader: &mut impl Read, bytes: u64) -> Result<()> {
    if bytes > MAX_METADATA_BYTES {
        return Err(invalid("metadata value exceeds the inspection limit"));
    }
    if std::io::copy(&mut reader.take(bytes), &mut std::io::sink())? != bytes {
        return Err(invalid("truncated metadata value"));
    }
    Ok(())
}

fn skip_value(reader: &mut impl Read, kind: u32, allow_array: bool) -> Result<()> {
    let size = match kind {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        8 => u64_value(reader)?,
        9 if allow_array => {
            let element = u32_value(reader)?;
            let count = u64_value(reader)?;
            if count > MAX_ITEMS || element == 9 {
                return Err(invalid("oversized or nested metadata array"));
            }
            for _ in 0..count {
                skip_value(reader, element, false)?;
            }
            return Ok(());
        }
        _ => return Err(invalid("unsupported metadata value type")),
    };
    skip(reader, size)
}

fn parse(mut reader: Take<impl Read>) -> Result<EmbeddingMetadata> {
    let metadata = parse_metadata(&mut reader)?;
    let architecture = metadata.architecture;
    let dimensions = metadata.numbers;
    let context_length = *dimensions
        .get(&format!("{architecture}.context_length"))
        .ok_or_else(|| invalid("missing context_length"))?;
    let embedding_length = *dimensions
        .get(&format!("{architecture}.embedding_length"))
        .ok_or_else(|| invalid("missing embedding_length"))?;
    if embedding_length == 0 || embedding_length > i32::MAX as u32 {
        return Err(invalid("embedding_length must be a positive int32"));
    }
    Ok(EmbeddingMetadata {
        context_length,
        embedding_length,
    })
}

fn parse_metadata(mut reader: impl Read) -> Result<Metadata> {
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" || !matches!(u32_value(&mut reader)?, 2 | 3) {
        return Err(invalid("expected little-endian GGUF version 2 or 3"));
    }
    let _tensor_count = u64_value(&mut reader)?;
    let count = u64_value(&mut reader)?;
    if count > MAX_KEYS {
        return Err(invalid("too many metadata entries"));
    }
    let mut architecture = None;
    let mut dimensions = HashMap::new();
    let mut descriptions = HashMap::new();
    for _ in 0..count {
        let key = string(&mut reader)?;
        let kind = u32_value(&mut reader)?;
        if key == "general.architecture" {
            if kind != 8 || architecture.is_some() {
                return Err(invalid("general.architecture must be a unique string"));
            }
            architecture = Some(string(&mut reader)?);
        } else if DESCRIPTION_KEYS.contains(&key.as_str()) && kind == 8 {
            if descriptions.insert(key, string(&mut reader)?).is_some() {
                return Err(invalid("duplicate descriptive model metadata"));
            }
        } else if key.ends_with(".embedding_length") || key.ends_with(".context_length") {
            if kind != 4 || dimensions.insert(key, u32_value(&mut reader)?).is_some() {
                return Err(invalid("embedding dimensions must be unique uint32 values"));
            }
        } else if [
            ".block_count",
            ".attention.head_count",
            ".attention.head_count_kv",
            ".attention.key_length",
            ".attention.value_length",
        ]
        .iter()
        .any(|s| key.ends_with(s))
            && kind == 4
        {
            if dimensions.insert(key, u32_value(&mut reader)?).is_some() {
                return Err(invalid("duplicate model dimension"));
            }
        } else {
            skip_value(&mut reader, kind, true)?;
        }
    }
    let architecture = architecture
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("missing general.architecture"))?;
    Ok(Metadata {
        architecture,
        numbers: dimensions,
        descriptions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_metadata_is_bounded_rust_inspection_without_tensor_loading() {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(0u64.to_le_bytes());
        bytes.extend(3u64.to_le_bytes());
        for (key, value) in [
            ("general.architecture", "llama"),
            ("general.name", "Example model"),
            ("general.size_label", "7B"),
        ] {
            bytes.extend((key.len() as u64).to_le_bytes());
            bytes.extend(key.as_bytes());
            bytes.extend(8u32.to_le_bytes());
            bytes.extend((value.len() as u64).to_le_bytes());
            bytes.extend(value.as_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("example.gguf");
        std::fs::write(&file, bytes).unwrap();
        let metadata = read_catalog_metadata(&file).unwrap();
        assert_eq!(metadata.architecture.as_deref(), Some("llama"));
        let description = metadata.description.unwrap();
        assert!(description.contains("Example model"));
        assert!(description.contains("7B"));
    }

    #[test]
    fn reads_dimensions_without_tensors_and_skips_tokenizer_arrays() {
        fn text(bytes: &mut Vec<u8>, value: &str) {
            bytes.extend((value.len() as u64).to_le_bytes());
            bytes.extend(value.as_bytes());
        }
        let mut bytes = b"GGUF".to_vec();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(0u64.to_le_bytes());
        bytes.extend(4u64.to_le_bytes());
        for (key, value) in [
            ("bert.context_length", 2048u32),
            ("bert.embedding_length", 768),
        ] {
            text(&mut bytes, key);
            bytes.extend(4u32.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
        text(&mut bytes, "tokenizer.ggml.tokens");
        bytes.extend(9u32.to_le_bytes());
        bytes.extend(8u32.to_le_bytes());
        bytes.extend(2u64.to_le_bytes());
        text(&mut bytes, "token one");
        text(&mut bytes, "token two");
        text(&mut bytes, "general.architecture");
        bytes.extend(8u32.to_le_bytes());
        text(&mut bytes, "bert");
        let metadata = parse(bytes.as_slice().take(MAX_METADATA_BYTES)).unwrap();
        assert_eq!(metadata.context_length, 2048);
        assert_eq!(metadata.embedding_length, 768);
        assert!(parse(bytes.as_slice().take(32)).is_err());
    }

    #[test]
    fn hostile_lengths_and_nested_arrays_are_errors_without_allocating_them() {
        let mut giant_string = &u64::MAX.to_le_bytes()[..];
        assert!(string(&mut giant_string).is_err());
        let mut array = 9u32.to_le_bytes().to_vec();
        array.extend(1u64.to_le_bytes());
        assert!(skip_value(&mut array.as_slice(), 9, true).is_err());
        assert!(skip(&mut &[0u8; 8][..], u64::MAX).is_err());
    }

    #[test]
    fn truncated_and_excessive_headers_are_rejected() {
        assert!(parse((&b"GGUF"[..]).take(MAX_METADATA_BYTES)).is_err());
        let mut header = b"GGUF".to_vec();
        header.extend(3u32.to_le_bytes());
        header.extend(0u64.to_le_bytes());
        header.extend(u64::MAX.to_le_bytes());
        assert!(parse(header.as_slice().take(MAX_METADATA_BYTES)).is_err());
    }
}
#[test]
fn grouped_query_cache_scales_with_kv_heads_not_quantized_file_size() {
    let shape = TransformerShape {
        layers: 32,
        embedding: 4096,
        heads: 32,
        kv_heads: 8,
        key_length: 128,
        value_length: 128,
    };
    let smaller = shape.context_bytes(4096).unwrap();
    let ordinary = TransformerShape {
        kv_heads: 32,
        ..shape
    }
    .context_bytes(4096)
    .unwrap();
    assert_eq!(ordinary - smaller, 32 * 4096 * 24 * 256 * 2);
    assert!(shape.context_bytes(8192).unwrap() > smaller);
    assert!(TransformerShape { heads: 0, ..shape }
        .context_bytes(4096)
        .is_none());
}
