use serde_json::{Map, Value as Json};

use super::{Cookies, Params, Session, parse_query};
use crate::Ctx;

/// One request: what the client sent, the merged `params`, and the `Ctx`
/// the controller works in.
pub struct Request {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub query: Map<String, Json>,
    pub body: Map<String, Json>,
    pub params: Params,
    /// The body claimed to be JSON and didn't parse. Rails answers 400 once
    /// a route matches, before the action runs.
    pub malformed_body: bool,
    /// `cookies`, from the `Cookie` header.
    pub cookies: Cookies,
    /// `session`, which the router sets up with the app's store.
    pub session: Session,
    pub ctx: Ctx,
}

/// The media types Rails parses as JSON (`Mime[:json]` and its synonyms).
const JSON_TYPES: [&str; 3] = ["application/json", "text/x-json", "application/jsonrequest"];

impl Request {
    pub fn new(ctx: Ctx, method: &str, path: &str) -> Self {
        Self {
            // Methods are case-sensitive: `delete` isn't DELETE.
            method: method.to_string(),
            path: path.to_string(),
            content_type: None,
            headers: Vec::new(),
            query: Map::new(),
            body: Map::new(),
            params: Params::default(),
            malformed_body: false,
            cookies: Cookies::default(),
            session: Session::default(),
            ctx,
        }
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self.cookies = Cookies::parse(self.header("cookie").as_deref());
        self
    }

    /// The `format` param (`/shop.json`, `?format=json`), which decides
    /// the format when there is one.
    fn format_param(&self) -> Option<&str> {
        self.params.get("format").and_then(Json::as_str)
    }

    /// The path's extension, which Rails reads after Accept, even with no
    /// route to set a `format` param.
    fn path_extension(&self) -> Option<&str> {
        extension(&self.path)
    }

    /// Rails reads Accept only when it isn't a browser's (`text/html, */*`
    /// lists `*/*` among others), or for an XHR.
    fn accept_used(&self) -> Option<String> {
        accept_used(|name| self.header(name))
    }

    /// The media types Accept lists, most wanted first.
    fn accepted(accept: &str) -> Vec<String> {
        let mut types: Vec<(usize, f64, String)> = accept
            .split(',')
            .enumerate()
            .map(|(i, part)| {
                let mut pieces = part.split(';');
                let media = pieces.next().unwrap_or("").trim().to_ascii_lowercase();
                let q = pieces.find_map(|p| p.trim().strip_prefix("q=").and_then(|q| q.parse::<f64>().ok())).unwrap_or(1.0);
                (i, q, media)
            })
            .collect();
        types.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        types.into_iter().map(|(_, _, media)| media).collect()
    }

    /// Whether an HTML template answers the request: its format is HTML,
    /// or the Accept header Rails reads takes HTML (or anything).
    pub fn accepts_html(&self) -> bool {
        if let Some(format) = self.format_param() {
            return format == "html";
        }
        match self.accept_used() {
            Some(accept) => Self::accepted(&accept)
                .iter()
                .any(|media| matches!(media.as_str(), "text/html" | "application/xhtml+xml" | "*/*" | "text/*")),
            None => self.path_extension().is_none_or(|extension| extension == "html"),
        }
    }

    /// Whether Rails' exceptions app answers in JSON: the request's first
    /// format is JSON. Anything else gets an HTML page.
    pub fn wants_json_errors(&self) -> bool {
        json_errors(self.format_param(), self.accept_used(), self.path_extension())
    }

    /// Whether a render varies by Accept (`Vary: Accept`): Rails chose the
    /// format from it.
    pub fn negotiated(&self) -> bool {
        self.format_param().is_none() && self.accept_used().is_some()
    }

    /// `request.headers["X-Api-Token"]`: the first value, any case.
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }

    pub fn with_query(mut self, query: &str) -> Self {
        self.query = parse_query(query);
        self.params = Params::new(self.body.clone(), self.query.clone());
        self
    }

    /// A JSON body, as a client sending `Content-Type: application/json`
    /// (Rails' `as: :json`). A body that isn't an object is `params[:_json]`.
    pub fn with_json(mut self, body: Json) -> Self {
        self.content_type = Some("application/json".to_string());
        self.body = match body {
            Json::Object(map) => map,
            other => Map::from_iter([("_json".to_string(), other)]),
        };
        self.params = Params::new(self.body.clone(), self.query.clone());
        self
    }

    /// A request body as Rails' parameter parsers read it: JSON for the JSON
    /// media types, form fields for `application/x-www-form-urlencoded`, and
    /// nothing for anything else, or for an empty body.
    pub fn with_body(mut self, content_type: Option<&str>, body: &[u8]) -> Self {
        self.content_type = content_type.map(str::to_string);
        if body.is_empty() {
            return self;
        }
        if self.is_json() {
            match serde_json::from_slice(body) {
                Ok(json) => self = self.with_json(json),
                Err(_) => self.malformed_body = true,
            }
        } else if self.media_type().as_deref() == Some("application/x-www-form-urlencoded") {
            self.body = parse_query(&String::from_utf8_lossy(body));
            self.params = Params::new(self.body.clone(), self.query.clone());
        }
        self.content_type = content_type.map(str::to_string);
        self
    }

    /// `Content-Type` without its parameters, lowercased.
    pub fn media_type(&self) -> Option<String> {
        let content_type = self.content_type.as_deref()?;
        Some(content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
    }

    pub fn is_json(&self) -> bool {
        self.media_type().is_some_and(|t| JSON_TYPES.contains(&t.as_str()))
    }
}

/// Whether Rails' exceptions app answers JSON: the format param decides,
/// then an Accept Rails reads, then the path's extension.
fn json_errors(format: Option<&str>, accept: Option<String>, extension: Option<&str>) -> bool {
    if let Some(format) = format {
        return format == "json";
    }
    match accept {
        Some(accept) => Request::accepted(&accept).first().is_some_and(|media| JSON_TYPES.contains(&media.as_str())),
        None => extension == Some("json"),
    }
}

/// `wants_json_errors` for a request that failed before it had a `Ctx`
/// (no database connection): the format param from the query, or else
/// the body, as Rails merges them, but not a route's.
pub(crate) fn wants_json_errors_for(target: &str, headers: &[(String, String)], content_type: Option<&str>, body: &[u8]) -> bool {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let header = |name: &str| headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
    // The query's format replaces the body's, whatever it holds.
    let query = parse_query(query);
    let format = match query.get("format") {
        Some(format) => format.as_str().map(str::to_string),
        None => body_format(content_type, body),
    };
    json_errors(format.as_deref(), accept_used(header), extension(path))
}

/// The `format` a JSON or form body sends, parsed as `with_body` does.
fn body_format(content_type: Option<&str>, body: &[u8]) -> Option<String> {
    let media = content_type?.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let params = if JSON_TYPES.contains(&media.as_str()) {
        match serde_json::from_slice(body).ok()? {
            Json::Object(map) => map,
            _ => return None,
        }
    } else if media == "application/x-www-form-urlencoded" {
        parse_query(&String::from_utf8_lossy(body))
    } else {
        return None;
    };
    params.get("format").and_then(Json::as_str).map(str::to_string)
}

/// Rails reads Accept only when it isn't a browser's (`text/html, */*`
/// lists `*/*` among others), or for an XHR.
fn accept_used(header: impl Fn(&str) -> Option<String>) -> Option<String> {
    let accept = header("accept").filter(|accept| !accept.trim().is_empty())?;
    let browser = accept.split(',').count() > 1 && accept.split(',').any(|part| part.trim().starts_with("*/*"));
    let xhr = header("x-requested-with").is_some_and(|x| x.eq_ignore_ascii_case("XMLHttpRequest"));
    (!browser || xhr).then_some(accept)
}

/// The path's extension, which Rails reads after Accept, even with no
/// route to set a `format` param.
fn extension(path: &str) -> Option<&str> {
    let (_, extension) = path.rsplit_once('.')?;
    (!extension.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(extension)
}
