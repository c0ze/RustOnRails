use serde_json::{Value as Json, json};

use crate::{Value, value_json};

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
    /// `Set-Cookie` headers, in order.
    pub cookies: Vec<String>,
    /// Other headers: Rails' default ones, `Vary`.
    pub headers: Vec<(&'static str, String)>,
    /// An exception reached the top: Rails' cookie and session middleware
    /// never see the response, so it carries no cookies.
    pub raised: bool,
}

impl Response {
    /// `render json: value, status: status`
    pub fn json(status: u16, value: Json) -> Self {
        Self { status, content_type: Some("application/json; charset=utf-8"), body: value.to_string().into_bytes(), cookies: Vec::new(), headers: Vec::new(), raised: false }
    }

    /// `render json: value` with a param's or the fallback's Value: a
    /// String is the body as it is (Rails calls to_json on anything but a
    /// String), anything else its JSON.
    pub fn json_value(status: u16, value: Value) -> Self {
        match value {
            Value::Str(text) => Self { body: text.into_bytes(), ..Self::head(status) }.typed("application/json; charset=utf-8"),
            other => Self::json(status, value_json(other)),
        }
    }

    /// A rendered template: `text/html`, as Rails sends a page.
    pub fn html(status: u16, body: String) -> Self {
        Self { body: body.into_bytes(), ..Self::head(status) }.typed("text/html; charset=utf-8")
    }

    /// `head status`
    pub fn head(status: u16) -> Self {
        Self { status, content_type: None, body: Vec::new(), cookies: Vec::new(), headers: Vec::new(), raised: false }
    }

    fn typed(self, content_type: &'static str) -> Self {
        Self { content_type: Some(content_type), ..self }
    }

    /// The body parsed as JSON (null when it isn't).
    pub fn body_json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or(Json::Null)
    }
}

/// What Rails' exceptions app renders for a JSON request. The router
/// renders it again in the request's own format (`Router::shown`).
pub fn error_page(status: u16) -> Response {
    let error = match reason(status) {
        "Unknown" => reason(500),
        known => known,
    };
    Response { raised: true, ..Response::json(status, json!({ "status": status, "error": error })) }
}

/// Rack's reason phrases (Rack 3.1 names 422 "Unprocessable Content").
pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        511 => "Network Authentication Required",
        _ => "Unknown",
    }
}
