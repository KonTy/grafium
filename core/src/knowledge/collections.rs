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
//! { "collection": "book", "collection-status": "draft" }
//! ```
//!
//! The helpers here read and write *only* those keys. Everything else a page
//! carries in `properties` is unrelated user data, so clobbering it while
//! toggling a marker would be a real data-loss bug — hence the surgical
//! insert/remove rather than replacing the whole blob.

use crate::models::Page;
use serde::{Deserialize, Serialize};

/// Property keys holding the collection marking.
///
/// Deliberately **flat string** properties rather than a nested object. The
/// markdown serializer only emits `key:: value` for string values, and
/// indexing a file *replaces* a page's properties with whatever the parser
/// read back — so a nested `{"collection": {...}}` was written to the database
/// and then silently erased by the next reindex, file-watcher event or sync
/// pull, with `pages_list_collections` simply returning nothing and no error
/// to explain it. A flat string survives the markdown round trip, which also
/// means a collection travels between devices in the file itself.
pub const COLLECTION_KIND_KEY: &str = "collection";
pub const COLLECTION_STATUS_KEY: &str = "collection-status";

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
    let kind = page
        .properties
        .get(COLLECTION_KIND_KEY)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let status = page
        .properties
        .get(COLLECTION_STATUS_KEY)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(CollectionInfo { kind, status })
}

/// Marks `props` as a collection of `kind`, leaving every other property alone.
pub fn mark_collection(props: &mut serde_json::Value, kind: &str) {
    if !props.is_object() {
        // A page whose properties are null/array/string still has to be
        // markable; replacing a non-object is the only option, and there was
        // nothing structured there to lose.
        *props = serde_json::json!({});
    }
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            COLLECTION_KIND_KEY.to_string(),
            serde_json::Value::String(kind.to_string()),
        );
    }
}

/// Removes the collection marking, leaving unrelated properties untouched.
pub fn clear_collection(props: &mut serde_json::Value) {
    if let Some(obj) = props.as_object_mut() {
        obj.remove(COLLECTION_KIND_KEY);
        obj.remove(COLLECTION_STATUS_KEY);
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
            "collection": "book", "collection-status": "draft"
        }));
        let info = collection_of(&page).expect("marker present");
        assert_eq!(info.kind, "book");
        assert_eq!(info.status.as_deref(), Some("draft"));

        let no_status = page_with_props(serde_json::json!({ "collection": "project" }));
        let info = collection_of(&no_status).unwrap();
        assert_eq!(info.kind, "project");
        assert_eq!(info.status, None);
    }

    #[test]
    fn malformed_marker_reads_as_not_a_collection() {
        // A status with no kind is not a collection: the kind is what makes
        // the page one, and half-populated info would show an empty header.
        assert!(collection_of(&page_with_props(
            serde_json::json!({ "collection-status": "draft" })
        ))
        .is_none());
        // Non-string or blank values are equally not a marking. Properties are
        // free-form and round-trip through markdown, so anything can land here.
        for bad in [
            serde_json::json!({ "collection": "" }),
            serde_json::json!({ "collection": "   " }),
            serde_json::json!({ "collection": 7 }),
            serde_json::json!({ "collection": { "kind": "book" } }),
        ] {
            assert!(collection_of(&page_with_props(bad)).is_none());
        }
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
        assert_eq!(props["collection"], serde_json::json!("book"));
    }

    #[test]
    fn re_marking_a_new_kind_preserves_existing_status() {
        let mut props = serde_json::json!({
            "collection": "book", "collection-status": "published"
        });
        mark_collection(&mut props, "project");
        assert_eq!(props["collection"], serde_json::json!("project"));
        assert_eq!(props["collection-status"], serde_json::json!("published"));
    }

    #[test]
    fn mark_promotes_a_null_properties_blob() {
        let mut props = serde_json::Value::Null;
        mark_collection(&mut props, "book");
        assert!(props.is_object());
        assert_eq!(props["collection"], serde_json::json!("book"));
    }

    #[test]
    fn clear_removes_only_the_marker() {
        let mut props = serde_json::json!({
            "icon": "📕",
            "collection": "book", "collection-status": "draft"
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
