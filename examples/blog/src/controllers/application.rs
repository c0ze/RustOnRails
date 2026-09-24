use rustonrails::{Error, Request, Response, Result, json, status};

// application_controller.rb  rescue_from ActiveRecord::RecordNotFound, with: :not_found
pub fn rescue(req: &mut Request, error: Error) -> Result<Response> {
    match error {
        Error::RecordNotFound { .. } => not_found(req),
        other => Err(other),
    }
}

// application_controller.rb  def not_found
fn not_found(_req: &mut Request) -> Result<Response> {
    Ok(Response::json(status::NOT_FOUND, json!({ "error": "not found" })))
}
