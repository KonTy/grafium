//! Marking an ordinary page as a *collection* — a book, a project, a reading
//! list — without a schema for it.
//!
//! The obvious way to model "a book with ordered chapters" is a membership
//! table (collection_id, page_id, position). We deliberately don't. A page in
//! this app already *is* an ordered list of blocks, each of which can hold a
//! `[[page link]]`, and the block editor already gives ordering, drag-to-
//! reorder, and free-form notes between entries for free. A separate membership
//! table would duplicate all of that and then have to be kept in sync with the
//! blocks the user actually edits. So a collection is just a normal page with a
//! marker in its `properties`, and its members are its linked blocks. Nothing
//! new to migrate, and the reorder UI is the one users already know.
//!
//! The marker lives under a single `collection` key so it rides the existing
//! `properties` JSON blob (and its normalized `page_properties` mirror) with no
//! column change:
//!
//! ```json
//! { "collection": { "kind": "book", "status": "draft" } }
//! ```
//!
//! The helpers here read and write *only* that key. Everything else a page
//! carries in `properties` is unrelated user data, so clobbering it while
//! toggling a marker would be a real data-loss bug — hence the surgical
//! insert/remove rather than replacing the whole blob.

use crate::models::Page;
use serde::{Deserialize, Serialize};

/// What a page's `collection` marker says about it.
///
/// `status` is optional because marking a page a collection and giving it a
/// workflow state (`draft`, `published`, …) are separate acts: the page menu
/// can flip a page into a book long before anyone decides the book is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub kind: String,
    pub status: Option<String>,
}

/// Read a page's collection marker, or `None` if it isn't one.
///
/// A marker only counts when `kind` is actually a string — a page whose
/// `properties.collection` is malformed (missing `kind`, or not an object) is
/// treated as "not a collection" rather than surfaced as a half-broken one, so
/// a stray hand-edit to the JSON can't wedge the UI.
pub fn collection_of(page: &Page) -> Option<CollectionInfo> {
    let collection = page.properties.get("collection")?;
    let kind = collection.get("kind")?.as_str()?.to_string();
    let status = collection
        .get("status")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    Some(CollectionInfo { kind, status })
}

/// Mark `props` as a collection of `kind`, leaving every other property intact.
///
/// If the page already carried a `status`, it is preserved: changing what kind
/// of collection a page is shouldn't silently reset where it is in its
/// workflow. When `props` isn't a JSON object yet (a fresh page serializes its
/// properties as `null`), it is promoted to an empty object first so the marker
/// has somewhere to live.
pub fn mark_collection(props: &mut serde_json::Value, kind: &str) {
    if !props.is_object() {
        *props = serde_json::Value::Object(serde_json::Map::new());
    }
    // Guaranteed by the promotion above; the `else` only exists so a future
    // change can't turn this into a panic.
    let Some(object) = props.as_object_mut() else {
        return;
    };

    let existing_status = object
        .get("collection")
        .and_then(|collection| collection.get("status"))
        .cloned();

    let mut collection = serde_json::Map::new();
    collection.insert(
        "kind".to_string(),
        serde_json::Value::String(kind.to_string()),
    );
    if let Some(status) = existing_status {
        collection.insert("status".to_string(), status);
    }
    object.insert(
        "collection".to_string(),
        serde_json::Value::Object(collection),
    );
}

/// Remove a page's collection marker, leaving every other property intact.
///
/// A no-op when the page has no properties object or no marker, so "un-mark"
/// is safe to call unconditionally from the page menu.
pub fn clear_collection(props: &mut serde_json::Value) {
    if let Some(object) = props.as_object_mut() {
        object.remove("collection");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with_props(props: serde_json::Value) -> Page {
        Page {
            id: "p1".to_string(),
            title: "My Book".to_string(),
            file_path: None,
            created_at: 0,
            updated_at: 0,
            is_journal: false,
            properties: props,
        }
    }

    #[test]
    fn collection_of_is_none_for_a_plain_page() {
        assert!(collection_of(&page_with_props(serde_json::json!({}))).is_none());
        assert!(collection_of(&page_with_props(serde_json::Value::Null)).is_none());
        assert!(collection_of(&page_with_props(serde_json::json!({ "icon": "book" }))).is_none());
    }

    #[test]
    fn collection_of_reads_kind_and_optional_status() {
        let page = page_with_props(serde_json::json!({
            "collection": { "kind": "book", "status": "draft" }
        }));
        let info = collection_of(&page).expect("marker present");
        assert_eq!(info.kind, "book");
        assert_eq!(info.status.as_deref(), Some("draft"));

        let no_status = page_with_props(serde_json::json!({ "collection": { "kind": "project" } }));
        let info = collection_of(&no_status).unwrap();
        assert_eq!(info.kind, "project");
        assert_eq!(info.status, None);
    }

    #[test]
    fn malformed_marker_reads_as_not_a_collection() {
        // `collection` present but without a string `kind` must not surface a
        // half-populated CollectionInfo.
        assert!(collection_of(&page_with_props(
            serde_json::json!({ "collection": { "status": "draft" } })
        ))
        .is_none());
        assert!(collection_of(&page_with_props(
            serde_json::json!({ "collection": "book" })
        ))
        .is_none());
    }

    #[test]
    fn mark_then_read_round_trips() {
        let mut props = serde_json::json!({});
        mark_collection(&mut props, "book");
        let page = page_with_props(props);
        assert_eq!(collection_of(&page).unwrap().kind, "book");
    }

    #[test]
    fn mark_preserves_unrelated_properties() {
        let mut props = serde_json::json!({
            "icon": "📕",
            "color": "red",
            "tags": ["fiction", "1900s"]
        });
        mark_collection(&mut props, "book");

        assert_eq!(props["icon"], serde_json::json!("📕"));
        assert_eq!(props["color"], serde_json::json!("red"));
        assert_eq!(props["tags"], serde_json::json!(["fiction", "1900s"]));
        assert_eq!(props["collection"]["kind"], serde_json::json!("book"));
    }

    #[test]
    fn re_marking_a_new_kind_preserves_existing_status() {
        let mut props = serde_json::json!({
            "collection": { "kind": "book", "status": "published" }
        });
        mark_collection(&mut props, "project");
        assert_eq!(props["collection"]["kind"], serde_json::json!("project"));
        assert_eq!(
            props["collection"]["status"],
            serde_json::json!("published")
        );
    }

    #[test]
    fn mark_promotes_a_null_properties_blob() {
        let mut props = serde_json::Value::Null;
        mark_collection(&mut props, "book");
        assert!(props.is_object());
        assert_eq!(props["collection"]["kind"], serde_json::json!("book"));
    }

    #[test]
    fn clear_removes_only_the_marker() {
        let mut props = serde_json::json!({
            "icon": "📕",
            "collection": { "kind": "book", "status": "draft" }
        });
        clear_collection(&mut props);

        assert!(props.get("collection").is_none());
        assert_eq!(props["icon"], serde_json::json!("📕"));
    }

    #[test]
    fn clear_is_a_no_op_without_a_marker() {
        let mut props = serde_json::json!({ "icon": "📗" });
        clear_collection(&mut props);
        assert_eq!(props, serde_json::json!({ "icon": "📗" }));

        let mut null_props = serde_json::Value::Null;
        clear_collection(&mut null_props);
        assert_eq!(null_props, serde_json::Value::Null);
    }

    #[test]
    fn full_lifecycle_round_trips_cleanly() {
        // mark → read → clear → read, with a sibling property watching the
        // whole time to prove nothing else is disturbed.
        let mut props = serde_json::json!({ "author": "Ada" });

        mark_collection(&mut props, "book");
        let marked = page_with_props(props.clone());
        assert_eq!(collection_of(&marked).unwrap().kind, "book");

        clear_collection(&mut props);
        let cleared = page_with_props(props.clone());
        assert!(collection_of(&cleared).is_none());
        assert_eq!(props["author"], serde_json::json!("Ada"));
    }
}
