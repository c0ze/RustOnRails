//! `session` with Rails' cookie store: the whole session in one encrypted
//! cookie that Rails and this server both read and write.
//!
//! As in Rails, a session loads when it's written, or read while its
//! cookie holds one; one that loaded is sent back re-encrypted, and one
//! that never did sends nothing.

use std::collections::HashMap;

use serde_json::{Map, Value as Json};

use super::cookies::CookieOptions;
use super::encryptor::CookieKey;
use crate::json::{format_date, format_time, value_json};
use crate::{Error, Result, Value};

/// `ActionDispatch::Cookies::MAX_COOKIE_SIZE`
const MAX_COOKIE_SIZE: usize = 4096;

/// The cookie store's settings: the cookie's name, the key for it, and
/// its attributes.
#[derive(Clone, Debug)]
pub struct SessionStore {
    pub name: &'static str,
    pub key: Option<CookieKey>,
    pub options: CookieOptions,
}

#[derive(Debug, Default)]
pub struct Session {
    store: Option<SessionStore>,
    /// The session cookie as the request sent it.
    cookie: Option<String>,
    data: Option<Map<String, Json>>,
    /// What this request set, as it set it: Rails keeps a Time a Time
    /// until the cookie's JSON is written.
    written: HashMap<String, Value>,
}

impl Session {
    pub fn new(store: Option<SessionStore>, cookie: Option<String>) -> Self {
        Self { store, cookie, data: None, written: HashMap::new() }
    }

    /// `session[:key]`: nil for a key it doesn't have. Rails keeps what
    /// JSON holds; a hash or an array here would need more than a Value.
    pub fn get(&mut self, key: &str) -> Result<Value> {
        if let Some(value) = self.written.get(key) {
            return Ok(value.clone());
        }
        if self.data.is_none() && self.existing()?.is_none() {
            return Ok(Value::Nil);
        }
        let data = self.load()?;
        match data.get(key) {
            None | Some(Json::Null) => Ok(Value::Nil),
            Some(Json::Bool(b)) => Ok(Value::Bool(*b)),
            Some(Json::Number(n)) => Ok(n.as_i64().map_or_else(|| Value::Float(n.as_f64().unwrap_or(f64::NAN)), Value::Int)),
            Some(Json::String(s)) => Ok(Value::Str(s.clone())),
            Some(_) => Err(Error::Type { message: format!("session[:{key}] holds a hash or an array") }),
        }
    }

    /// `session[:key] = value`: stored as its JSON, as Rails' serializer
    /// writes it. nil leaves the key out of the cookie.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) -> Result<()> {
        let value = value.into();
        let json = match &value {
            Value::Time(t) => Json::String(format_time(*t)),
            Value::Date(d) => Json::String(format_date(*d)),
            other => value_json(other.clone()),
        };
        self.load()?.insert(key.to_string(), json);
        self.written.insert(key.to_string(), value);
        Ok(())
    }

    /// `session.delete(:key)`: what it held.
    pub fn delete(&mut self, key: &str) -> Result<Value> {
        let value = self.get(key)?;
        self.written.remove(key);
        if self.data.is_some() {
            self.load()?.shift_remove(key);
        }
        Ok(value)
    }

    /// `reset_session`: an empty session under a new id.
    pub fn reset(&mut self) -> Result<()> {
        self.store()?;
        let mut data = Map::new();
        data.insert("session_id".into(), Json::String(session_id()));
        self.data = Some(data);
        self.written.clear();
        Ok(())
    }

    /// The `Set-Cookie` header for the session, if it loaded.
    pub fn header(&self) -> Result<Option<String>> {
        let Some(data) = &self.data else { return Ok(None) };
        let store = self.store()?;
        let kept: Map<String, Json> = data.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect();
        let value = key(store)?.encrypt(store.name, Json::Object(kept).to_string().as_bytes());
        // Rails' encrypted jar refuses what a browser would drop.
        if value.len() > MAX_COOKIE_SIZE {
            let message = format!("{} cookie overflowed with size {} bytes", store.name, value.len());
            return Err(Error::Raised { class: "ActionDispatch::Cookies::CookieOverflow", message });
        }
        Ok(Some(format!("{}={}{}", store.name, super::cookies::escape(&value), store.options.attributes())))
    }

    fn store(&self) -> Result<&SessionStore> {
        self.store.as_ref().ok_or_else(|| Error::Type { message: "the app has no session store".into() })
    }

    /// The session the cookie holds: one with an id, as Rails requires.
    fn existing(&self) -> Result<Option<Map<String, Json>>> {
        let store = self.store()?;
        let Some(cookie) = &self.cookie else { return Ok(None) };
        let Some(plain) = key(store)?.decrypt(store.name, cookie) else { return Ok(None) };
        match serde_json::from_slice(&plain) {
            Ok(Json::Object(data)) if data.get("session_id").is_some_and(Json::is_string) => Ok(Some(data)),
            _ => Ok(None),
        }
    }

    /// The session, loaded: the cookie's, or a new one with a new id.
    fn load(&mut self) -> Result<&mut Map<String, Json>> {
        if self.data.is_none() {
            let data = match self.existing()? {
                Some(data) => data,
                None => {
                    let mut data = Map::new();
                    data.insert("session_id".into(), Json::String(session_id()));
                    data
                }
            };
            self.data = Some(data);
        }
        Ok(self.data.as_mut().expect("loaded above"))
    }
}

fn key(store: &SessionStore) -> Result<&CookieKey> {
    store.key.as_ref().ok_or_else(|| Error::Type { message: "SECRET_KEY_BASE isn't set, so sessions can't be read or written".into() })
}

/// Rack's session id: 16 random bytes in hex.
fn session_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS has randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
