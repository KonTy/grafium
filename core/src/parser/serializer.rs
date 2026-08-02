use crate::models::Block;
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SerializationStats {
    parent_map_inserts: usize,
    child_bucket_lookups: usize,
    serialized_blocks: usize,
}

/// Serialize a list of blocks (belonging to one page) back into a outline-style markdown file.
/// Blocks are expected to be sorted by order_index already.
/// Page-level properties can be prepended separately.
pub fn serialize_page(page_properties: &serde_json::Value, blocks: &[Block]) -> String {
    serialize_page_internal(page_properties, blocks).0
}

/// Serialize one block and all of its descendants using the same canonical
/// formatting as `serialize_page`.
pub fn serialize_block_subtree(blocks: &[Block], root_id: &str, depth: usize) -> Option<String> {
    let mut stats = SerializationStats::default();
    let children_by_parent = build_children_by_parent(blocks, &mut stats);
    let root = blocks.iter().find(|block| block.id == root_id)?;
    let mut out = String::new();
    serialize_block(&mut out, root, &children_by_parent, depth, &mut stats);
    Some(out)
}

fn serialize_page_internal(
    page_properties: &serde_json::Value,
    blocks: &[Block],
) -> (String, SerializationStats) {
    let mut out = String::new();
    let mut stats = SerializationStats::default();

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

    let children_by_parent = build_children_by_parent(blocks, &mut stats);
    serialize_blocks(&mut out, &children_by_parent, None, 0, &mut stats);

    (out, stats)
}

fn build_children_by_parent<'a>(
    blocks: &'a [Block],
    stats: &mut SerializationStats,
) -> HashMap<Option<&'a str>, Vec<&'a Block>> {
    let mut children_by_parent: HashMap<Option<&'a str>, Vec<&'a Block>> = HashMap::new();
    for block in blocks {
        children_by_parent
            .entry(block.parent_id.as_deref())
            .or_default()
            .push(block);
        stats.parent_map_inserts += 1;
    }
    children_by_parent
}

fn serialize_blocks<'a>(
    out: &mut String,
    children_by_parent: &HashMap<Option<&'a str>, Vec<&'a Block>>,
    parent_id: Option<&'a str>,
    depth: usize,
    stats: &mut SerializationStats,
) {
    stats.child_bucket_lookups += 1;
    let Some(children) = children_by_parent.get(&parent_id) else {
        return;
    };

    for block in children {
        serialize_block(out, block, children_by_parent, depth, stats);
    }
}

fn serialize_block<'a>(
    out: &mut String,
    block: &'a Block,
    children_by_parent: &HashMap<Option<&'a str>, Vec<&'a Block>>,
    depth: usize,
    stats: &mut SerializationStats,
) {
    stats.serialized_blocks += 1;
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

    serialize_blocks(
        out,
        children_by_parent,
        Some(block.id.as_str()),
        depth + 1,
        stats,
    );
}

#[cfg(test)]
fn serialize_page_with_stats(
    page_properties: &serde_json::Value,
    blocks: &[Block],
) -> (String, SerializationStats) {
    serialize_page_internal(page_properties, blocks)
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

    #[test]
    fn test_serialize_hierarchy_snapshot_is_stable() {
        let blocks = vec![
            Block {
                id: "parent".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 0,
                content: "Parent".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({"status": "active"}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "child-1".to_string(),
                page_id: "page-1".to_string(),
                parent_id: Some("parent".to_string()),
                order_index: 0,
                content: "Child one".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "grandchild".to_string(),
                page_id: "page-1".to_string(),
                parent_id: Some("child-1".to_string()),
                order_index: 0,
                content: "Grandchild\ncontinuation".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({"note": "deep"}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "child-2".to_string(),
                page_id: "page-1".to_string(),
                parent_id: Some("parent".to_string()),
                order_index: 1,
                content: "Child two".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "sibling".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 1,
                content: "Sibling root".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({"priority": "high"}),
                created_at: 0,
                updated_at: 0,
            },
        ];

        let result = serialize_page(&serde_json::json!({"owner": "alice"}), &blocks);

        assert_eq!(
            result,
            concat!(
                "owner:: alice\n",
                "\n",
                "- Parent\n",
                "  id:: parent\n",
                "  status:: active\n",
                "  - Child one\n",
                "    id:: child-1\n",
                "    - Grandchild\n",
                "      continuation\n",
                "      id:: grandchild\n",
                "      note:: deep\n",
                "  - Child two\n",
                "    id:: child-2\n",
                "- Sibling root\n",
                "  id:: sibling\n",
                "  priority:: high\n",
            )
        );
    }

    #[test]
    fn test_serialize_uses_linear_parent_lookup_work() {
        let mut blocks = Vec::new();
        for i in 0..1_500 {
            let (parent_id, order_index) = if i == 0 {
                (None, 0)
            } else {
                let parent = if i % 3 == 0 {
                    None
                } else {
                    Some(format!("block-{}", i - 1))
                };
                (parent, (i % 7) as i32)
            };
            blocks.push(Block {
                id: format!("block-{i}"),
                page_id: "page-1".to_string(),
                parent_id,
                order_index,
                content: format!("Block {i}"),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            });
        }

        let (result, stats) = serialize_page_with_stats(&serde_json::json!({}), &blocks);

        assert!(result.contains("- Block 0\n"));
        assert_eq!(stats.parent_map_inserts, blocks.len());
        assert_eq!(stats.serialized_blocks, blocks.len());
        assert!(stats.child_bucket_lookups <= blocks.len() + 1);
    }

    #[test]
    fn test_serialize_block_subtree_matches_page_fragment() {
        let blocks = vec![
            Block {
                id: "root".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 0,
                content: "Root".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "child".to_string(),
                page_id: "page-1".to_string(),
                parent_id: Some("root".to_string()),
                order_index: 0,
                content: "Child".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            Block {
                id: "sibling".to_string(),
                page_id: "page-1".to_string(),
                parent_id: None,
                order_index: 1,
                content: "Sibling".to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
        ];

        let fragment = serialize_block_subtree(&blocks, "root", 0).unwrap();
        assert_eq!(
            fragment,
            concat!(
                "- Root\n",
                "  id:: root\n",
                "  - Child\n",
                "    id:: child\n",
            )
        );
    }
}
