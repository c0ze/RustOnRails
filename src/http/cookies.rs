//! `cookies` in a controller: what the request's `Cookie` header holds,
//! and the `Set-Cookie` headers assignments make, as Rack writes them.

use form_urlencoded::byte_serialize;

/// The request's cookies, and the ones set during it, in order.
#[derive(Clone, Debug, Default)]
pub struct Cookies {
    incoming: Vec<(String, String)>,
    set: Vec<(String, String)>,
}

impl Cookies {
    /// Rack's parsing: `a=1; b=2`, each part URL-unescaped, the first of a
    /// repeated name winning.
    pub fn parse(header: Option<&str>) -> Self {
        let mut incoming: Vec<(String, String)> = Vec::new();
        for part in header.unwrap_or("").split(';').map(str::trim).filter(|part| !part.is_empty()) {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            let (name, value) = (unescape(name), unescape(value));
            if !incoming.iter().any(|(n, _)| *n == name) {
                incoming.push((name, value));
            }
        }
        Self { incoming, set: Vec::new() }
    }

    /// `cookies[:name]`: what was set during the request, or sent with it.
    pub fn get(&self, name: &str) -> Option<String> {
        let set = self.set.iter().rev().find(|(n, _)| n == name);
        set.or_else(|| self.incoming.iter().find(|(n, _)| n == name)).map(|(_, value)| value.clone())
    }

    /// `cookies[:name] = value`. As Rails' jar does, a value the request
    /// already carries isn't sent back.
    pub fn set(&mut self, name: &str, value: String) {
        self.set.retain(|(n, _)| n != name);
        if self.incoming.iter().any(|(n, v)| n == name && *v == value) {
            return;
        }
        self.set.push((name.to_string(), value));
    }

    /// The `Set-Cookie` headers, in the order the cookies were set:
    /// `path=/` and `samesite=lax`, Rails 8.1's defaults.
    pub fn headers(&self) -> Vec<String> {
        self.set.iter().map(|(name, value)| format!("{}={}; path=/; samesite=lax", escape(name), escape(value))).collect()
    }
}

/// Rack's `escape`: `URI.encode_www_form_component`.
pub(crate) fn escape(text: &str) -> String {
    byte_serialize(text.as_bytes()).collect()
}

/// Rack's `unescape`: `+` is a space and `%XX` a byte; a `%` that
/// doesn't start one stays as it is.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() && hex(bytes[i + 1]).is_some() && hex(bytes[i + 2]).is_some() => {
                out.push((hex(bytes[i + 1]).unwrap_or(0) * 16 + hex(bytes[i + 2]).unwrap_or(0)) as u8);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
