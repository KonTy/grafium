use super::ast::{QueryNode, DateFilter};
use crate::error::{CoreError, Result};

/// Parse a query string like:
/// - [[Page]]
/// - "text search"
/// - (and [[Project]] (task TODO))
/// - (and (scheduled today) (task TODO))
/// - (property key value)
/// - (deadline before 2024-01-01)
pub fn parse_query(input: &str) -> Result<QueryNode> {
    let trimmed = input.trim();

    if trimmed.starts_with("[[") && trimmed.ends_with("]]") {
        let page = &trimmed[2..trimmed.len() - 2];
        return Ok(QueryNode::Page(page.to_string()));
    }

    if trimmed.starts_with('"') && trimmed.ends_with('"') {
        let text = &trimmed[1..trimmed.len() - 1];
        return Ok(QueryNode::Text(text.to_string()));
    }

    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        return parse_sexp(&trimmed[1..trimmed.len() - 1]);
    }

    // Fallback: treat as text search
    Ok(QueryNode::Text(trimmed.to_string()))
}

fn parse_sexp(input: &str) -> Result<QueryNode> {
    let trimmed = input.trim();

    if trimmed.starts_with("and ") {
        let rest = &trimmed[4..];
        let children = parse_children(rest)?;
        return Ok(QueryNode::And(children));
    }

    if trimmed.starts_with("or ") {
        let rest = &trimmed[3..];
        let children = parse_children(rest)?;
        return Ok(QueryNode::Or(children));
    }

    if trimmed.starts_with("task ") {
        let state = trimmed[5..].trim().to_string();
        return Ok(QueryNode::TaskState(state));
    }

    if trimmed.starts_with("property ") {
        let parts: Vec<&str> = trimmed[9..].splitn(2, ' ').collect();
        if parts.len() == 2 {
            return Ok(QueryNode::Property(parts[0].to_string(), parts[1].to_string()));
        }
        return Err(CoreError::Parse("Invalid property query".to_string()));
    }

    if trimmed.starts_with("scheduled ") {
        let date_filter = parse_date_filter(&trimmed[10..])?;
        return Ok(QueryNode::Scheduled(date_filter));
    }

    if trimmed.starts_with("deadline ") {
        let date_filter = parse_date_filter(&trimmed[9..])?;
        return Ok(QueryNode::Deadline(date_filter));
    }

    if trimmed.starts_with("created-since ") {
        let days: u32 = trimmed[14..].trim().parse().unwrap_or(7);
        return Ok(QueryNode::CreatedSince(days));
    }

    if trimmed.starts_with("updated-since ") {
        let days: u32 = trimmed[14..].trim().parse().unwrap_or(7);
        return Ok(QueryNode::UpdatedSince(days));
    }

    Err(CoreError::Parse(format!("Unknown query expression: {}", trimmed)))
}

fn parse_date_filter(input: &str) -> Result<DateFilter> {
    let trimmed = input.trim();

    if trimmed == "today" {
        return Ok(DateFilter::Today);
    }

    if trimmed.starts_with("before ") {
        return Ok(DateFilter::Before(trimmed[7..].to_string()));
    }

    if trimmed.starts_with("after ") {
        return Ok(DateFilter::After(trimmed[6..].to_string()));
    }

    Ok(DateFilter::On(trimmed.to_string()))
}

fn parse_children(input: &str) -> Result<Vec<QueryNode>> {
    let mut children = Vec::new();
    let mut depth = 0;
    let mut current_start = 0;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '(' => {
                if depth == 0 {
                    current_start = i;
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let segment: String = chars[current_start..=i].iter().collect();
                    children.push(parse_query(segment.trim())?);
                }
            }
            '[' if depth == 0 => {
                // [[Page]] link
                if i + 1 < chars.len() && chars[i + 1] == '[' {
                    let start = i;
                    while i < chars.len() - 1 {
                        if chars[i] == ']' && chars[i + 1] == ']' {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    let segment: String = chars[start..=i].iter().collect();
                    children.push(parse_query(segment.trim())?);
                }
            }
            '"' if depth == 0 => {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                let segment: String = chars[start..=i].iter().collect();
                children.push(parse_query(segment.trim())?);
            }
            _ => {}
        }
        i += 1;
    }

    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_page_query() {
        let result = parse_query("[[Project]]").unwrap();
        match result {
            QueryNode::Page(p) => assert_eq!(p, "Project"),
            _ => panic!("Expected Page node"),
        }
    }

    #[test]
    fn test_parse_text_query() {
        let result = parse_query("\"hello world\"").unwrap();
        match result {
            QueryNode::Text(t) => assert_eq!(t, "hello world"),
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn test_parse_task_query() {
        let result = parse_query("(task TODO)").unwrap();
        match result {
            QueryNode::TaskState(s) => assert_eq!(s, "TODO"),
            _ => panic!("Expected TaskState node"),
        }
    }
}
