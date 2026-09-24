use rustonrails::{Error, Json, Params, Response, Value, error_page, json, parse_query, status};

fn map(value: Json) -> serde_json::Map<String, Json> {
    value.as_object().unwrap().clone()
}

#[test]
fn test_query_strings_parse_like_rails() {
    let parsed = parse_query("email=a%40b.c&user[name]=Ann&tag[]=x&tag[]=y");
    assert_eq!(json!({"email": "a@b.c", "user": {"name": "Ann"}, "tag": ["x", "y"]}), Json::Object(parsed));
}

#[test]
fn test_query_overrides_body_and_path_overrides_both() {
    let mut params = Params::new(map(json!({"id": "body", "a": 1})), map(json!({"id": "query"})));
    assert_eq!(Value::from("query"), params.value("id"));
    params.merge_path(map(json!({"id": "7"})));
    assert_eq!(Value::from("7"), params.value("id"));
    assert_eq!(Value::Int(1), params.value("a"));
}

#[test]
fn test_require_and_permit() {
    let params = Params::new(map(json!({"user": {"name": "Ann", "admin": true, "tags": ["x"], "email": null}})), Default::default());
    let permitted = params.require("user").unwrap().permit(&["email", "name", "tags"]);
    assert_eq!(vec![("email".to_string(), Value::Nil), ("name".to_string(), Value::from("Ann"))], permitted);
}

#[test]
fn test_missing_or_empty_is_parameter_missing() {
    let empty = Params::new(map(json!({"user": {}})), Default::default());
    let error = empty.require("user").unwrap_err();
    assert!(matches!(error, Error::ParameterMissing { key: "user" }));
    assert_eq!("param is missing or the value is empty or invalid: user", error.to_string());
    assert!(matches!(empty.expect("post", &["title"]), Err(Error::ParameterMissing { .. })));
}

#[test]
fn test_wrap_copies_body_keys_only() {
    let body = map(json!({"title": "Hi", "junk": 1}));
    let mut params = Params::new(body.clone(), map(json!({"page": "2"})));
    params.wrap(&body, "post", &["title", "page"]);
    assert_eq!(json!({"title": "Hi"}), *params.get("post").unwrap());
}

#[test]
fn test_wrap_leaves_existing_key_alone() {
    let body = map(json!({"post": {"title": "Nested"}, "title": "Top"}));
    let mut params = Params::new(body.clone(), Default::default());
    params.wrap(&body, "post", &["title"]);
    assert_eq!(json!({"title": "Nested"}), *params.get("post").unwrap());
}

#[test]
fn test_responses() {
    let ok = Response::json(status::CREATED, json!({"id": 1}));
    assert_eq!(201, ok.status);
    assert_eq!(Some("application/json; charset=utf-8"), ok.content_type);
    assert_eq!(json!({"id": 1}), ok.body_json());
    assert!(Response::head(status::NO_CONTENT).body.is_empty());
    assert_eq!(json!({"status": 404, "error": "Not Found"}), error_page(404).body_json());
    assert_eq!(json!({"status": 422, "error": "Unprocessable Content"}), error_page(422).body_json());
}
