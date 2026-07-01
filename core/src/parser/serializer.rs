use crate::models::Block;

/// Serialize a list of blocks (belonging to one page) back into a outline-style markdown file.
/// Blocks are expected to be sorted by order_index already.
/// Page-level properties can be prepended separately.
pub fn serialize_page(
    page_properties: &serde_json::Value,
    blocks: &[Block],
) -> String {
    let mut out = String::new();

    // Write page-level properties at the top
    if let Some(obj) = page_properties.as_object() {
        for (key, value) in obj {
            if let Some(v) = value.as_str() {
                if !v.is_empty() {
                    out.push_str(&format!("{}:: {}\n", key, v));
                }
            }
        }
        if !obj.is_empty() {
            out.push('\n');
        }
    }

    // Build a tree structure from flat blocks
    serialize_blocks(&mut out, blocks, None, 0);

    out
}

fn serialize_blocks(out: &mut String, blocks: &[Block], parent_id: Option<&str>, depth: usize) {
    let children: Vec<&Block> = blocks
        .iter()
        .filter(|b| b.parent_id.as_deref() == parent_id)
        .collect();

    for block in children {
        let indent = "  ".repeat(depth);
        // Write block content with proper continuation indentation for multiline content.
        let lines: Vec<&str> = block.content.split('\n').collect();
        if let Some((first, rest)) = lines.split_first() {
            out.push_str(&format!("{}- {}\n", indent, first));
            let continuation_indent = format!("{}  ", indent);
            for line in rest {
                out.push_str(&format!("{}{}\n", continuation_indent, line));
            }
        } else {
            out.push_str(&format!("{}- \n", indent));
        }

        // Write block-level properties (id, custom properties)
        let prop_indent = "  ".repeat(depth + 1);
        out.push_str(&format!("{}id:: {}\n", prop_indent, block.id));

        if let Some(obj) = block.properties.as_object() {
            for (key, value) in obj {
                if key == "id" {
                    continue; // Already written
                }
                if let Some(v) = value.as_str() {
                    if !v.is_empty() {
                        out.push_str(&format!("{}{}:: {}\n", prop_indent, key, v));
                    }
                }
            }
        }

        // Recurse into children
        serialize_blocks(out, blocks, Some(&block.id), depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BlockType;

    #[test]
    fn test_serialize_simple() {
        let blocks = vec![
            Block {
                id: "block-1".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 0,
                content: "Hello world".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "block-2".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 1,
                content: "Second block".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
        ];

        let result = serialize_page(&serde_json::json!({}), &blocks);
        assert!(result.contains("- Hello world\n"));
        assert!(result.contains("- Second block\n"));
        assert!(result.contains("  id:: block-1\n"));
        assert!(result.contains("  id:: block-2\n"));
    }

    #[test]
    fn test_serialize_nested() {
        let blocks = vec![
            Block {
                id: "parent".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 0,
                content: "Parent".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "child".to_string(),
                page_id: "page-1".to_string(),
                parent_id: Some("parent".to_string()),
                order_index: 0,
                content: "Child".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
        ];

        let result = serialize_page(&serde_json::json!({}), &blocks);
        assert!(result.contains("- Parent\n"));
        assert!(result.contains("  id:: parent\n"));
        assert!(result.contains("  - Child\n"));
        assert!(result.contains("    id:: child\n"));
    }

    #[test]
    fn test_serialize_multiline_code_fence_continuations() {
        let blocks = vec![Block {
            id: "code-1".to_string(),
            page_id: "page-1".to_string(),
            parent_id: None,
            order_index: 0,
            content: "```rust\nlet x = 1;\nlet y = x + 1;\n```".to_string(),
            block_type: BlockType::Text,
            properties: serde_json::json!({}),
            created_at: 0,
            updated_at: 0,
        }];

        let result = serialize_page(&serde_json::json!({}), &blocks);

        assert!(result.contains("- ```rust\n"));
        assert!(result.contains("  let x = 1;\n"));
        assert!(result.contains("  let y = x + 1;\n"));
        assert!(result.contains("  ```\n"));
    }
}
