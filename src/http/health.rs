use super::{Request, Response};

/// `Rails::HealthController#show` answering an HTML request: the `/up`
/// route a new Rails app has.
pub fn health(_req: &mut Request) -> Response {
    let body = r#"<!DOCTYPE html><html><body style="background-color: green"></body></html>"#;
    Response { content_type: Some("text/html; charset=utf-8"), body: body.as_bytes().to_vec(), ..Response::head(200) }
}
