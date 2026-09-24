use serde_json::{Value as Json, json};

/// Rails' status symbols used by the PoC.
pub mod status {
    pub const OK: u16 = 200;
    pub const CREATED: u16 = 201;
    pub const NO_CONTENT: u16 = 204;
    pub const BAD_REQUEST: u16 = 400;
    pub const NOT_FOUND: u16 = 404;
    pub const UNPROCESSABLE_CONTENT: u16 = 422;
    pub const INTERNAL_SERVER_ERROR: u16 = 500;
}

pub struct Response {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub body: Vec<u8>,
}

impl Response {
    /// `render json: value, status: status`
    pub fn json(status: u16, value: Json) -> Self {
        Self { status, content_type: Some("application/json; charset=utf-8"), body: value.to_string().into_bytes() }
    }

    /// `head status`
    pub fn head(status: u16) -> Self {
        Self { status, content_type: None, body: Vec::new() }
    }

    /// The body parsed as JSON (null when it isn't).
    pub fn body_json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or(Json::Null)
    }
}

/// What Rails' exceptions app renders for a JSON request.
pub fn error_page(status: u16) -> Response {
    Response::json(status, json!({ "status": status, "error": reason(status) }))
}

/// Rack's reason phrases (Rack 3.1 names 422 "Unprocessable Content").
pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}
