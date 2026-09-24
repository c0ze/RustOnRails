/// `record.errors`: messages per attribute, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Errors {
    entries: Vec<(String, String)>,
}

impl Errors {
    pub fn add(&mut self, attribute: &str, message: impl Into<String>) {
        self.entries.push((attribute.to_string(), message.into()));
    }

    /// `errors[:title]`
    pub fn on(&self, attribute: &str) -> Vec<&str> {
        self.entries.iter().filter(|(a, _)| a == attribute).map(|(_, m)| m.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Attributes with errors, first appearance first, as in `errors.as_json`.
    pub fn attributes(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for (attribute, _) in &self.entries {
            if !seen.contains(&attribute.as_str()) {
                seen.push(attribute);
            }
        }
        seen
    }

    /// "Title can't be blank", as in `errors.full_messages`.
    pub fn full_messages(&self) -> Vec<String> {
        self.entries.iter().map(|(a, m)| format!("{} {m}", humanize(a))).collect()
    }
}

/// `"user_id".humanize` is "User"; `"comments_count"` is "Comments count".
fn humanize(attribute: &str) -> String {
    let words = attribute.strip_suffix("_id").unwrap_or(attribute).replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}
