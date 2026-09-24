use serde_json::{Map, Value as Json};

use super::{Params, parse_query};
use crate::Ctx;

/// One request: what the client sent, the merged `params`, and the `Ctx`
/// the controller works in.
pub struct Request {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub query: Map<String, Json>,
    pub body: Map<String, Json>,
    pub params: Params,
    pub ctx: Ctx,
}

impl Request {
    pub fn new(ctx: Ctx, method: &str, path: &str) -> Self {
        Self {
            method: method.to_uppercase(),
            path: path.to_string(),
            content_type: None,
            query: Map::new(),
            body: Map::new(),
            params: Params::default(),
            ctx,
        }
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

    pub fn is_json(&self) -> bool {
        self.content_type.as_deref().is_some_and(|t| t.starts_with("application/json"))
    }
}
