use serde_json::{Map, Value as Json};

use crate::{Attributes, Error, Result, Value};

/// `params`: body parameters, query parameters over them, and after
/// routing the path parameters over both, as Rails merges them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params(Map<String, Json>);

impl Params {
    pub fn new(body: Map<String, Json>, query: Map<String, Json>) -> Self {
        let mut map = body;
        map.extend(query);
        Self(map)
    }

    pub fn merge_path(&mut self, path: Map<String, Json>) {
        self.0.extend(path);
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        self.0.get(key)
    }

    /// `params[:id]` as a scalar; nested hashes and arrays are nil.
    pub fn value(&self, key: &str) -> Value {
        self.get(key).map_or(Value::Nil, scalar)
    }

    /// `params.fetch(:page, 1)`: the value when the key is there (a null
    /// too, as Rails' fetch), `default` when it isn't.
    pub fn fetch(&self, key: &str, default: impl Into<Value>) -> Value {
        self.get(key).map_or_else(|| default.into(), scalar)
    }

    /// `params.require(:user)`: the nested hash, or `ParameterMissing` when
    /// it's absent, empty or not a hash.
    pub fn require(&self, key: &'static str) -> Result<Params> {
        match self.get(key) {
            Some(Json::Object(map)) if !map.is_empty() => Ok(Params(map.clone())),
            _ => Err(Error::ParameterMissing { key }),
        }
    }

    /// `permit(:name, :email)`: the listed keys that hold scalars, in the
    /// order listed; anything else is dropped, as Rails does by default.
    pub fn permit(&self, keys: &[&str]) -> Attributes {
        keys.iter()
            .filter_map(|key| self.get(key).filter(|v| !v.is_array() && !v.is_object()).map(|v| (key.to_string(), scalar(v))))
            .collect()
    }

    /// `params.expect(post: [:title, ...])`: like `require` + `permit`,
    /// except that a hash with nothing permitted is also missing.
    pub fn expect(&self, key: &'static str, keys: &[&str]) -> Result<Attributes> {
        let permitted = self.require(key)?.permit(keys);
        if permitted.is_empty() {
            return Err(Error::ParameterMissing { key });
        }
        Ok(permitted)
    }

    /// `wrap_parameters`: when `name` isn't a parameter yet, puts the body
    /// keys listed in `include` under it. Query parameters are never wrapped.
    pub fn wrap(&mut self, body: &Map<String, Json>, name: &str, include: &[&str]) {
        if self.0.contains_key(name) {
            return;
        }
        let wrapped: Map<String, Json> =
            body.iter().filter(|(k, _)| include.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        self.0.insert(name.to_string(), Json::Object(wrapped));
    }
}

fn scalar(value: &Json) -> Value {
    match value {
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n.as_i64().map_or_else(|| n.as_f64().map_or(Value::Nil, Value::Float), Value::Int),
        Json::String(s) => Value::Str(s.clone()),
        _ => Value::Nil,
    }
}

/// A query string as Rails parses it: `a=1`, `b[c]=2` and `d[]=3`.
pub fn parse_query(query: &str) -> Map<String, Json> {
    let mut map = Map::new();
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        let value = Json::String(value.into_owned());
        match key.split_once('[') {
            Some((base, "]")) => {
                let entry = map.entry(base.to_string()).or_insert_with(|| Json::Array(Vec::new()));
                if let Json::Array(items) = entry {
                    items.push(value);
                }
            }
            Some((base, rest)) if rest.ends_with(']') => {
                let entry = map.entry(base.to_string()).or_insert_with(|| Json::Object(Map::new()));
                if let Json::Object(inner) = entry {
                    inner.insert(rest.trim_end_matches(']').to_string(), value);
                }
            }
            _ => {
                map.insert(key.into_owned(), value);
            }
        }
    }
    map
}
