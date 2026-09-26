use serde_json::{Map, Value as Json};

use super::{Params, parse_query};
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
            ctx,
        }
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
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
    /// (Rails' `as: :json`). A body that isn't an object adds no params.
    pub fn with_json(mut self, body: Json) -> Self {
        self.content_type = Some("application/json".to_string());
        self.body = match body {
            Json::Object(map) => map,
            _ => Map::new(),
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
