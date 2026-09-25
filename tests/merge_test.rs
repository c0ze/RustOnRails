use rustonrails::{json, merge};

/// `task.as_json.merge("overdue" => true)`: new keys go last, a key
/// already there keeps its place and takes the new value.
#[test]
fn test_merge_is_ruby_hash_merge() {
    let merged = merge(json!({ "id": 1, "title": "a" }), json!({ "overdue": true, "id": 2 }));
    assert_eq!(r#"{"id":2,"title":"a","overdue":true}"#, merged.to_string());
    assert_eq!(json!({ "a": 1 }), merge(json!({ "a": 1 }), json!({})));
}

#[test]
#[should_panic(expected = "merge takes two JSON objects")]
fn test_merge_on_anything_but_objects_is_a_bug() {
    merge(json!([1]), json!({ "a": 1 }));
}
