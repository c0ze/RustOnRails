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

fn id_is_numeric(req: &Request) -> bool {
    req.params.value("id").to_ruby_string().chars().all(|c| c.is_ascii_digit())
}

// Rails sets the route's path parameters before its constraint runs.
#[test]
fn test_constraints_see_the_routes_path_params() {
    let router = Router::new()
        .get("/things/:id", echo("numeric"))
        .constraint(id_is_numeric)
        .get("/things/:id", echo("other"));
    let mut numeric = Request::new(support::ctx(), "GET", "/things/42");
    assert_eq!(json!({"route": "numeric", "params": {"id": "42"}}), router.call(&mut numeric).body_json());
    let mut other = Request::new(numeric.ctx, "GET", "/things/abc");
    assert_eq!(json!({"route": "other", "params": {"id": "abc"}}), router.call(&mut other).body_json());
}

#[test]
fn test_health_answers_like_rails_health_controller() {
    let mut req = Request::new(support::ctx(), "GET", "/up");
    let response = rustonrails::health(&mut req);
    assert_eq!((200, Some("text/html; charset=utf-8")), (response.status, response.content_type));
    assert!(String::from_utf8(response.body).unwrap().contains("background-color: green"));
}
