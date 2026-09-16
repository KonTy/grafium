use super::Database;
use crate::error::Result;
use crate::parser::TagTerm;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityDecision {
    Reuse,
    New,
    Ambiguous,
}

impl EntityDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reuse => "reuse",
            Self::New => "new",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntityCandidate {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityResolution {
    pub source_phrase: String,
    pub display_slug: String,
    pub target_title: String,
    pub target_page_id: Option<String>,
    pub decision: EntityDecision,
    pub candidates: Vec<EntityCandidate>,
    pub reason: String,
}

/// Fold common presentation separators, not semantic punctuation or senses.
pub(crate) fn entity_key(value: &str) -> String {
    crate::parser::normalize_page_title(value)
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if matches!(c, '_' | '-') { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn entity_aliases(properties: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(properties).unwrap_or_default();
    let mut aliases = BTreeSet::new();
    for key in ["alias", "aliases"] {
        match &value[key] {
            serde_json::Value::String(text) => {
                for name in text.split(',') {
                    let name = name.trim().trim_start_matches("[[").trim_end_matches("]]");
                    if !name.is_empty() {
                        aliases.insert(name.to_string());
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for name in values.iter().filter_map(|v| v.as_str()) {
                    let name = name.trim().trim_start_matches("[[").trim_end_matches("]]");
                    if !name.is_empty() {
                        aliases.insert(name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    serde_json::to_string(&aliases).unwrap_or_else(|_| "[]".into())
}

pub(crate) fn entity_grams(value: &str) -> String {
    let chars: Vec<char> = value.chars().take(256).collect();
    let grams: BTreeSet<String> = chars.windows(3).map(|w| w.iter().collect()).collect();
    serde_json::to_string(&grams).unwrap_or_else(|_| "[]".into())
}

fn compatible_sense(a: &str, b: &str) -> bool {
    // A namespace or explicit parenthetical qualification is not a typo.
    let namespace = |s: &str| s.rsplit_once('/').map(|(prefix, _)| prefix.to_owned());
    let qualification = |s: &str| s.find('(').map(|offset| s[offset..].to_owned());
    let punctuation = |s: &str| {
        s.chars()
            .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
            .collect::<String>()
    };
    namespace(a) == namespace(b)
        && qualification(a) == qualification(b)
        && punctuation(a) == punctuation(b)
}

fn bounded_spelling_match(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let max_len = a.len().max(b.len());
    if a.len().min(b.len()) < 4 || max_len > 256 {
        return false;
    }
    let budget = if max_len <= 6 { 1 } else { 2 };
    if a.len().abs_diff(b.len()) > budget {
        return false;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, a_char) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, b_char) in b.iter().enumerate() {
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + usize::from(a_char != b_char));
        }
        if *current.iter().min().unwrap_or(&0) > budget {
            return false;
        }
        previous = current;
    }
    previous[b.len()] <= budget && previous[b.len()] * 5 <= max_len
}

fn resolve_one(conn: &Connection, tag: &TagTerm) -> Result<EntityResolution> {
    let target_title = crate::parser::normalize_page_title(tag.label());
    let key = entity_key(&target_title);
    let mut resolution = EntityResolution {
        source_phrase: tag.term.trim().to_string(),
        display_slug: crate::parser::format_concept_tag(&target_title)
            .trim_start_matches('#')
            .to_string(),
        target_title,
        target_page_id: None,
        decision: EntityDecision::New,
        candidates: Vec::new(),
        reason: "No known title or approved alias matches; new page proposal.".into(),
    };
    if key.is_empty()
        || key.chars().count() > 256
        || resolution.target_title.chars().any(char::is_control)
        || resolution.target_title.contains(['[', ']', '|'])
    {
        resolution.decision = EntityDecision::Ambiguous;
        resolution.reason = "Empty or overly long entity name cannot be resolved safely.".into();
        return Ok(resolution);
    }

    let exact = conn
        .prepare(
            "SELECT DISTINCT p.id, p.title FROM entity_names n
             JOIN pages p ON p.id = n.page_id WHERE n.name_key = ?1
             ORDER BY p.title, p.id LIMIT 9",
        )?
        .query_map([&key], |row| {
            Ok(EntityCandidate {
                id: row.get(0)?,
                title: row.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if exact.len() == 1 {
        resolution.target_page_id = Some(exact[0].id.clone());
        resolution.target_title = exact[0].title.clone();
        resolution.display_slug = crate::parser::format_concept_tag(&resolution.target_title)
            .trim_start_matches('#')
            .to_string();
        resolution.decision = EntityDecision::Reuse;
        resolution.reason = "Unique normalized title or approved page alias.".into();
        return Ok(resolution);
    }
    if exact.len() > 1 {
        resolution.decision = EntityDecision::Ambiguous;
        resolution.candidates = exact;
        resolution.reason =
            "This name belongs to multiple pages; choose a target explicitly.".into();
        return Ok(resolution);
    }
    if !key.contains(['(', '/']) {
        let prefix = format!("{key} (");
        resolution.candidates = conn
            .prepare(
                "SELECT DISTINCT p.id, p.title FROM entity_names n
             JOIN pages p ON p.id = n.page_id
             WHERE n.name_key >= ?1 AND n.name_key < ?2
             ORDER BY p.title, p.id LIMIT 8",
            )?
            .query_map(params![prefix, format!("{prefix}\u{10ffff}")], |row| {
                Ok(EntityCandidate {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !resolution.candidates.is_empty() {
            resolution.decision = EntityDecision::Ambiguous;
            resolution.reason =
                "A parenthetical sense is required; a shared surface name is not an alias.".into();
            return Ok(resolution);
        }
    }

    // Only a bounded lexical shortlist gets spelling comparison. No model sees
    // all entities, and a spelling distance is never treated as a probability.
    let names = conn
        .prepare(
            "SELECT n.page_id, p.title, n.name_key, count(*) AS overlap
             FROM entity_name_grams g
             JOIN entity_names n ON n.page_id = g.page_id AND n.name_key = g.name_key
             JOIN pages p ON p.id = n.page_id
             WHERE g.gram IN (SELECT value FROM json_each(?1))
               AND length(n.name_key) BETWEEN ?2 AND ?3
             GROUP BY n.page_id, n.name_key
             ORDER BY overlap DESC, p.title, n.page_id LIMIT 64",
        )?
        .query_map(
            params![
                entity_grams(&key),
                key.chars().count().saturating_sub(2) as i64,
                key.chars().count() as i64 + 2
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut seen = BTreeSet::new();
    for (id, title, candidate_key) in names {
        if compatible_sense(&key, &candidate_key)
            && bounded_spelling_match(&key, &candidate_key)
            && seen.insert(id.clone())
        {
            resolution.candidates.push(EntityCandidate { id, title });
            if resolution.candidates.len() == 8 {
                break;
            }
        }
    }
    if !resolution.candidates.is_empty() {
        resolution.decision = EntityDecision::Ambiguous;
        resolution.reason =
            "Similar spelling is not identity; review these possible targets before linking."
                .into();
    }
    Ok(resolution)
}

impl Database {
    /// Resolve source phrases without creating pages or persisting model aliases.
    pub fn resolve_tag_terms(&self, tags: &[TagTerm]) -> Result<Vec<EntityResolution>> {
        let conn = self.conn()?;
        self.resolve_tag_terms_in_connection(&conn, tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(db: &Database, title: &str) -> Result<EntityResolution> {
        Ok(db.resolve_tag_terms(&[TagTerm::from(title)])?.remove(0))
    }

    #[test]
    fn normalized_names_preserve_existing_identity_and_title() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Café au Lait", false)?;
        for variant in ["CAFÉ_AU_LAIT", "café-au-lait", "  café   au lait  "] {
            let resolved = resolve(&db, variant)?;
            assert_eq!(resolved.decision, EntityDecision::Reuse);
            assert_eq!(resolved.target_page_id.as_deref(), Some(page.id.as_str()));
            assert_eq!(resolved.target_title, "Café au Lait");
        }
        assert_eq!(db.count_pages()?, 1);
        Ok(())
    }

    #[test]
    fn hierarchy_variants_share_lookup_keys_but_preserve_existing_titles() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Projects/Alpha", false)?;
        db.update_page(&page.id, Some(r"Projects \ Alpha"), Some(&serde_json::json!({
            "aliases":["Work / Alpha"]
        })))?;
        for variant in ["Projects / Alpha", r"Projects\Alpha", "Work/Alpha", r"Work \ Alpha"] {
            let result = resolve(&db, variant)?;
            assert_eq!(result.decision, EntityDecision::Reuse);
            assert_eq!(result.target_page_id.as_deref(), Some(page.id.as_str()));
            assert_eq!(result.target_title, r"Projects \ Alpha");
            assert_eq!(db.find_page_by_name(variant)?.unwrap().id, page.id);
        }
        let new = resolve(&db, r"Archive \ Beta")?;
        assert_eq!(new.decision, EntityDecision::New);
        assert_eq!(new.target_title, "Archive/Beta");
        assert_eq!(db.count_pages()?, 1);
        Ok(())
    }

    #[test]
    fn distinct_senses_namespaces_and_technical_punctuation_do_not_collapse() -> Result<()> {
        let db = Database::in_memory()?;
        db.create_page("Mercury (planet)", false)?;
        db.create_page("Projects/Alpha", false)?;
        let cpp = db.create_page("C++", false)?;
        let csharp = db.create_page("C#", false)?;
        assert_eq!(
            resolve(&db, "Mercury (element)")?.decision,
            EntityDecision::New
        );
        assert_eq!(resolve(&db, "Mercury")?.decision, EntityDecision::Ambiguous);
        assert_eq!(resolve(&db, "Archive/Alpha")?.decision, EntityDecision::New);
        assert_eq!(resolve(&db, "c++")?.target_page_id, Some(cpp.id));
        assert_eq!(resolve(&db, "c#")?.target_page_id, Some(csharp.id));
        assert_eq!(resolve(&db, "C")?.decision, EntityDecision::New);
        Ok(())
    }

    #[test]
    fn spelling_shortlist_is_reviewable_not_an_automatic_alias() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Niacin", false)?;
        let resolved = resolve(&db, "Nicin")?;
        assert_eq!(resolved.decision, EntityDecision::Ambiguous);
        assert_eq!(resolved.target_page_id, None);
        assert_eq!(
            resolved.candidates,
            vec![EntityCandidate {
                id: page.id,
                title: page.title
            }]
        );
        assert_eq!(db.count_pages()?, 1);
        Ok(())
    }

    #[test]
    fn approved_alias_updates_and_ambiguity_are_indexed() -> Result<()> {
        let db = Database::in_memory()?;
        let first = db.create_page("Niacin (Vitamin B3)", false)?;
        let second = db.create_page("Another nutrient", false)?;
        db.conn()?.execute(
            "UPDATE pages SET properties = ?2 WHERE id = ?1",
            params![first.id, r#"{"aliases":["Niacin","Vitamin B3"]}"#],
        )?;
        assert_eq!(
            resolve(&db, "niacin")?.target_page_id,
            Some(first.id.clone())
        );
        db.conn()?.execute(
            "UPDATE pages SET properties = ?2 WHERE id = ?1",
            params![second.id, r#"{"alias":"Niacin, Some other name"}"#],
        )?;
        let collision = resolve(&db, "Niacin")?;
        assert_eq!(collision.decision, EntityDecision::Ambiguous);
        assert_eq!(collision.candidates.len(), 2);
        db.conn()?.execute(
            "UPDATE pages SET properties = '{}' WHERE id = ?1",
            [&first.id],
        )?;
        assert_eq!(resolve(&db, "Niacin")?.target_page_id, Some(second.id));
        Ok(())
    }

    #[test]
    fn duplicate_normalized_titles_are_not_arbitrarily_selected() -> Result<()> {
        let db = Database::in_memory()?;
        db.create_page("Memory Palace", false)?;
        db.create_page("Memory_Palace", false)?;
        assert_eq!(
            resolve(&db, "memory palace")?.decision,
            EntityDecision::Ambiguous
        );
        Ok(())
    }

    #[test]
    fn ordinary_link_targets_reuse_unique_normalized_names_and_aliases() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Café au Lait", false)?;
        db.conn()?.execute(
            "UPDATE pages SET properties = ?2 WHERE id = ?1",
            params![page.id, r#"{"aliases":["Milk coffee"]}"#],
        )?;
        assert_eq!(db.get_or_create_page("CAFÉ_AU_LAIT", false)?.id, page.id);
        assert_eq!(db.get_or_create_page("milk_coffee", false)?.id, page.id);
        let collision = db.create_page("Coffee variant", false)?;
        db.conn()?.execute(
            "UPDATE pages SET properties = ?2 WHERE id = ?1",
            params![collision.id, r#"{"aliases":["Milk coffee"]}"#],
        )?;
        assert!(db.get_or_create_page("milk coffee", false).is_err());
        assert_eq!(db.count_pages()?, 2);
        Ok(())
    }

    #[test]
    fn batch_reuses_proposal_identity_and_flags_near_duplicate_spelling() -> Result<()> {
        let db = Database::in_memory()?;
        let result = db.resolve_tag_terms(&[
            TagTerm::from("Memory Palace"),
            TagTerm::from("memory_palace"),
            TagTerm::from("Memory Palce"),
        ])?;
        assert_eq!(result[0].decision, EntityDecision::New);
        assert_eq!(result[1].target_title, result[0].target_title);
        assert_eq!(result[1].source_phrase, "memory_palace");
        assert_eq!(result[2].decision, EntityDecision::Ambiguous);
        assert_eq!(db.count_pages()?, 0);
        Ok(())
    }

    #[test]
    fn unsafe_model_target_text_remains_unresolved() -> Result<()> {
        let db = Database::in_memory()?;
        for title in ["[[injected]]", "target|other", "two\nlines"] {
            assert_eq!(resolve(&db, title)?.decision, EntityDecision::Ambiguous);
        }
        Ok(())
    }
}

impl Database {
    pub(crate) fn resolve_tag_terms_in_connection(
        &self,
        conn: &Connection,
        tags: &[TagTerm],
    ) -> Result<Vec<EntityResolution>> {
        let mut cache: BTreeMap<String, EntityResolution> = BTreeMap::new();
        let mut out = Vec::with_capacity(tags.len());
        for tag in tags {
            let key = entity_key(tag.label());
            let mut resolution = if let Some(known) = cache.get(&key) {
                known.clone()
            } else {
                let mut resolved = resolve_one(conn, tag)?;
                if resolved.decision == EntityDecision::New {
                    if let Some(previous) = cache.iter().take(64).find_map(|(other_key, other)| {
                        (other.decision == EntityDecision::New
                            && compatible_sense(&key, other_key)
                            && bounded_spelling_match(&key, other_key))
                        .then_some(other)
                    }) {
                        resolved.decision = EntityDecision::Ambiguous;
                        resolved.reason = format!(
                            "Similar to another proposal in this batch ({}); review before creating another page.",
                            previous.target_title
                        );
                    }
                }
                cache.insert(key, resolved.clone());
                resolved
            };
            resolution.source_phrase = tag.term.trim().to_string();
            out.push(resolution);
        }
        Ok(out)
    }
}
