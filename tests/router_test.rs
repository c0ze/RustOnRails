mod support;

use rustonrails::{Handler, Json, Request, Response, Router, json};

/// A handler that echoes which route ran and the params it saw.
fn echo(name: &'static str) -> Handler {
    Box::new(move |req: &mut Request| {
        let params: Json = ["id", "post_id", "format", "email"]
            .iter()
            .filter_map(|k| req.params.get(k).map(|v| (k.to_string(), v.clone())))
            .collect::<serde_json::Map<String, Json>>()
            .into();
        Response::json(200, json!({ "route": name, "params": params }))
    })
}

fn has_email(req: &Request) -> bool {
    !req.params.value("email").is_blank()
}

fn router() -> Router {
    Router::new()
        .get("/users/lookup(.:format)", echo("users#lookup"))
        .constraint(has_email)
        .get("/users/:id(.:format)", echo("users#show"))
        .get("/posts/:post_id/comments(.:format)", echo("comments#index"))
        .patch("/posts/:id(.:format)", echo("posts#update"))
        .get("/", echo("root"))
}

fn call(method: &str, path: &str, query: &str) -> Json {
    let mut req = Request::new(support::ctx(), method, path).with_query(query);
    router().call(&mut req).body_json()
}

#[test]
fn test_path_params_and_format() {
    assert_eq!(json!({"route": "comments#index", "params": {"post_id": "5"}}), call("GET", "/posts/5/comments", ""));
    assert_eq!(json!({"route": "users#show", "params": {"id": "7", "format": "json"}}), call("GET", "/users/7.json", ""));
    assert_eq!(json!("root"), call("GET", "/", "")["route"]);
}

#[test]
fn test_method_must_match() {
    assert_eq!(json!("posts#update"), call("PATCH", "/posts/1", "")["route"]);
    assert_eq!(json!({"status": 404, "error": "Not Found"}), call("POST", "/posts/1", ""));
}

#[test]
fn test_constraint_passes() {
    assert_eq!(json!("users#lookup"), call("GET", "/users/lookup", "email=a%40b.c")["route"]);
}

#[test]
fn test_failed_constraint_falls_through() {
    assert_eq!(json!({"route": "users#show", "params": {"id": "lookup"}}), call("GET", "/users/lookup", ""));
}

#[test]
fn test_no_route_is_a_404_page() {
    assert_eq!(json!({"status": 404, "error": "Not Found"}), call("GET", "/nowhere", ""));
}

#[test]
fn test_json_bodies_become_params() {
    let mut req = Request::new(support::ctx(), "PATCH", "/posts/3").with_json(json!({"email": "x"}));
    assert!(req.is_json());
    let body = router().call(&mut req).body_json();
    assert_eq!(json!({"id": "3", "email": "x"}), body["params"]);
}
