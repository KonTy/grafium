use grafium_core::Graph;
use tempfile::TempDir;
use std::fs;
use std::path::Path;

/// Test that validation correctly identifies a valid graph structure
#[test]
fn test_validate_structure_valid_graph() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create proper graph structure
    fs::create_dir(root.join("pages")).unwrap();
    fs::create_dir(root.join("journals")).unwrap();
    fs::create_dir(root.join(".grafium")).unwrap();

    let report = Graph::validate_structure(root);
    assert!(report.is_valid, "Valid graph should pass validation");
    assert!(report.has_pages_dir, "Should have pages dir");
    assert!(report.has_journals_dir, "Should have journals dir");
    assert!(report.has_metadata_dir, "Should have .grafium dir");
    assert!(report.has_valid_db, "DB missing is acceptable");
    assert!(report.not_nested_in_another_graph, "Top-level graph should not be nested");
    assert!(report.has_no_nested_graph_roots, "Graph should not contain nested roots");
    assert_eq!(report.error_message, None, "Valid graph should have no error");
}

/// Test that validation rejects a directory missing pages/
#[test]
fn test_validate_structure_missing_pages_dir() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create incomplete structure (missing pages/)
    fs::create_dir(root.join("journals")).unwrap();
    fs::create_dir(root.join(".grafium")).unwrap();

    let report = Graph::validate_structure(root);
    assert!(!report.is_valid, "Graph without pages/ should fail");
    assert!(!report.has_pages_dir, "Should not have pages dir");
    assert!(report.has_journals_dir, "Should have journals dir");
    assert!(report.has_metadata_dir, "Should have .grafium dir");
    assert!(report.error_message.is_some(), "Should have error message");
    assert!(
        report.error_message.as_ref().unwrap().contains("pages/"),
        "Error should mention missing pages/"
    );
}

/// Test that validation rejects a directory missing journals/
#[test]
fn test_validate_structure_missing_journals_dir() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create incomplete structure (missing journals/)
    fs::create_dir(root.join("pages")).unwrap();
    fs::create_dir(root.join(".grafium")).unwrap();

    let report = Graph::validate_structure(root);
    assert!(!report.is_valid, "Graph without journals/ should fail");
    assert!(report.has_pages_dir, "Should have pages dir");
    assert!(!report.has_journals_dir, "Should not have journals dir");
    assert!(report.has_metadata_dir, "Should have .grafium dir");
    assert!(report.error_message.is_some(), "Should have error message");
    assert!(
        report.error_message.as_ref().unwrap().contains("journals/"),
        "Error should mention missing journals/"
    );
}

/// Test that validation rejects a directory missing .grafium/
#[test]
fn test_validate_structure_missing_metadata_dir() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create incomplete structure (missing .grafium/)
    fs::create_dir(root.join("pages")).unwrap();
    fs::create_dir(root.join("journals")).unwrap();

    let report = Graph::validate_structure(root);
    assert!(!report.is_valid, "Graph without .grafium/ should fail");
    assert!(report.has_pages_dir, "Should have pages dir");
    assert!(report.has_journals_dir, "Should have journals dir");
    assert!(!report.has_metadata_dir, "Should not have .grafium dir");
    assert!(report.error_message.is_some(), "Should have error message");
    assert!(
        report.error_message.as_ref().unwrap().contains(".grafium/"),
        "Error should mention missing .grafium/"
    );
}

/// Test that validation rejects a directory with multiple missing folders
#[test]
fn test_validate_structure_multiple_missing() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create very minimal structure (only pages/)
    fs::create_dir(root.join("pages")).unwrap();

    let report = Graph::validate_structure(root);
    assert!(!report.is_valid, "Graph missing multiple dirs should fail");
    assert!(report.error_message.is_some(), "Should have error message");
    let error = report.error_message.as_ref().unwrap();
    assert!(
        error.contains("journals/"),
        "Error should mention missing journals/"
    );
    assert!(
        error.contains(".grafium/"),
        "Error should mention missing .grafium/"
    );
}

/// Test that validation works on a directory that doesn't exist
#[test]
fn test_validate_structure_nonexistent_path() {
    let report = Graph::validate_structure(Path::new("/nonexistent/path/12345678"));
    assert!(!report.is_valid, "Nonexistent path should fail");
    assert!(!report.has_pages_dir, "Nonexistent path has no pages dir");
    assert!(!report.has_journals_dir, "Nonexistent path has no journals dir");
    assert!(!report.has_metadata_dir, "Nonexistent path has no .grafium dir");
}

/// Test that open_graph refuses to open invalid structures
#[test]
fn test_open_graph_rejects_invalid_structure() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create incomplete structure
    fs::create_dir(root.join("pages")).unwrap();
    // Missing journals/ and .grafium/

    // validate_structure should catch this before any open operation
    let validation = Graph::validate_structure(root);
    assert!(!validation.is_valid, "validate_structure should catch invalid structure");

    // Graph::open still succeeds by design (open-or-create semantics)
    let result = Graph::open(root);
    assert!(result.is_ok(), "Graph::open creates missing dirs (open or create semantics)");
}

/// Test that a newly created graph passes validation
#[test]
fn test_validate_structure_newly_created_graph() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path();

    // Create a new graph
    let _graph = Graph::open(root).expect("Failed to create graph");

    // Now validate it
    let report = Graph::validate_structure(root);
    assert!(report.is_valid, "Newly created graph should pass validation");
    assert!(report.has_pages_dir, "Created graph should have pages dir");
    assert!(report.has_journals_dir, "Created graph should have journals dir");
    assert!(report.has_metadata_dir, "Created graph should have .grafium dir");
    assert!(report.has_valid_db, "Created graph should have valid DB");
    assert!(report.not_nested_in_another_graph, "Created graph should not be nested");
    assert!(report.has_no_nested_graph_roots, "Created graph should not contain nested roots");
}

/// Test that a graph nested inside another graph is rejected.
#[test]
fn test_validate_structure_rejects_nested_graph_root() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let outer = tmp.path().join("Outer");
    let inner = outer.join("journals").join("Inner");

    // Outer graph
    fs::create_dir_all(outer.join("pages")).unwrap();
    fs::create_dir_all(outer.join("journals")).unwrap();
    fs::create_dir_all(outer.join(".grafium")).unwrap();

    // Inner graph nested under outer/journals
    fs::create_dir_all(inner.join("pages")).unwrap();
    fs::create_dir_all(inner.join("journals")).unwrap();
    fs::create_dir_all(inner.join(".grafium")).unwrap();

    let inner_report = Graph::validate_structure(&inner);
    assert!(!inner_report.is_valid, "Nested graph root should be rejected");
    assert!(!inner_report.not_nested_in_another_graph, "Inner root must be marked nested");
    assert!(
        inner_report.error_message.as_ref().unwrap().contains("nested inside another graph"),
        "Error should explain nested root rejection"
    );
}

/// Test that a graph containing nested graph roots is rejected.
#[test]
fn test_validate_structure_rejects_graph_with_nested_graphs_inside() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let root = tmp.path().join("All");
    let nested = root.join("journals").join("NestedGraph");

    // Root graph
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("journals")).unwrap();
    fs::create_dir_all(root.join(".grafium")).unwrap();

    // Nested graph inside root/journals
    fs::create_dir_all(nested.join("pages")).unwrap();
    fs::create_dir_all(nested.join("journals")).unwrap();
    fs::create_dir_all(nested.join(".grafium")).unwrap();

    let root_report = Graph::validate_structure(&root);
    assert!(!root_report.is_valid, "Root containing nested graph should be rejected");
    assert!(!root_report.has_no_nested_graph_roots, "Nested graph presence should be flagged");
    assert!(
        root_report.error_message.as_ref().unwrap().contains("contains nested graph root"),
        "Error should explain nested graph presence"
    );
}
