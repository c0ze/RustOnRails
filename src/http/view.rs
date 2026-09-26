//! ERB views as Action View renders them. Rutile compiles a template from
//! the Ruby Rails' ERB handler makes of it, so the text between tags,
//! trimmed as Rails trims it, arrives here as it is, and each `<%= %>`
//! value is escaped as Action View's output buffer escapes it.

use std::collections::HashMap;

use super::Response;
use crate::{Error, Result, Value};

/// A render in progress: the output buffer, the template's output once
/// the layout takes over, and what `content_for` collected.
#[derive(Debug, Default)]
pub struct View {
    out: String,
    content: String,
    content_for: HashMap<&'static str, String>,
}

impl View {
    /// Template text, already HTML (`safe_append`).
    pub fn text(&mut self, html: &str) {
        self.out.push_str(html);
    }

    /// `<%= value %>`: escaped (`append`).
    pub fn append(&mut self, text: &str) {
        self.out.push_str(&html_escape(text));
    }

    /// `<%== value %>` and `raw(value)`: as it is (`safe_expr_append`).
    pub fn raw(&mut self, html: &str) {
        self.out.push_str(html);
    }

    /// `content_for :name, value`: added to what's there, escaped unless
    /// it's HTML already.
    pub fn content_for_append(&mut self, name: &'static str, html: &str) {
        self.content_for.entry(name).or_default().push_str(html);
    }

    /// `content_for(:name)`: nil when what was given is blank.
    pub fn content_for(&self, name: &str) -> Option<String> {
        self.content_for.get(name).filter(|html| !html.trim().is_empty()).cloned()
    }

    /// `content_for?(:name)`
    pub fn has_content_for(&self, name: &str) -> bool {
        self.content_for(name).is_some()
    }

    /// The template is done: its output is what the layout's `yield` gives.
    pub fn lay_out(&mut self) {
        self.content = std::mem::take(&mut self.out);
    }

    /// `<%= yield %>` in a layout: the template's output.
    pub fn append_content(&mut self) {
        self.out.push_str(&self.content);
    }

    /// `<%= yield :name %>`: what content_for collected, "" for nothing.
    pub fn append_yield(&mut self, name: &str) {
        if let Some(html) = self.content_for.get(name) {
            self.out.push_str(html);
        }
    }

    /// The page, with Rails' HTML content type.
    pub fn response(self, status: u16) -> Response {
        Response::html(status, self.out)
    }
}

/// `ERB::Util.html_escape`: `& < > " '`.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `link_to name, href, class: ...`: the attributes as given, then href,
/// as Action View writes them. `name` is HTML already.
pub fn link_to(name: &str, href: &str, attributes: &[(&str, &str)]) -> String {
    let mut tag = String::from("<a");
    for (key, value) in attributes.iter().chain([&("href", href)]) {
        tag.push_str(&format!(" {key}=\"{}\"", html_escape(value)));
    }
    format!("{tag}>{name}</a>")
}

/// What a route helper puts in a path segment for a value, as `to_param`
/// gives it; nil is a missing key.
pub trait ToParam {
    fn to_param(&self) -> Option<String>;
}

impl ToParam for i64 {
    fn to_param(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl ToParam for Option<i64> {
    fn to_param(&self) -> Option<String> {
        self.map(|id| id.to_string())
    }
}

impl ToParam for str {
    fn to_param(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl<T: ToParam + ?Sized> ToParam for &T {
    fn to_param(&self) -> Option<String> {
        (**self).to_param()
    }
}

impl ToParam for String {
    fn to_param(&self) -> Option<String> {
        Some(self.clone())
    }
}

impl ToParam for Value {
    fn to_param(&self) -> Option<String> {
        (!matches!(self, Value::Nil)).then(|| self.to_s())
    }
}

/// A path segment for `value`, escaped as Journey escapes one; a missing
/// value is Rails' UrlGenerationError for the route.
pub fn path_segment(value: impl ToParam, controller: &str, action: &str, key: &str) -> Result<String> {
    let Some(param) = value.to_param() else {
        let message = format!(
            "No route matches {{action: \"{action}\", controller: \"{controller}\", {key}: nil}}, missing required keys: [:{key}]"
        );
        return Err(Error::Raised { class: "ActionController::UrlGenerationError", message });
    };
    let mut out = String::with_capacity(param.len());
    for byte in param.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':' | b'@' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    Ok(out)
}
