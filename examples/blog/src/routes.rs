use rustonrails::{Request, Response, Router};

/// config/routes.rb
pub fn routes() -> Router {
    Router::new()
        // routes.rb:10  get "up" => "rails/health#show", as: :rails_health_check
        .get("/up(.:format)", Box::new(health))
}

/// Rails::HealthController#show, as it answers an HTML request.
fn health(_req: &mut Request) -> Response {
    let body = r#"<!DOCTYPE html><html><body style="background-color: green"></body></html>"#;
    Response { status: 200, content_type: Some("text/html; charset=utf-8"), body: body.as_bytes().to_vec() }
}
