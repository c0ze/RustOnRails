use crate::behavior::Validation;
use crate::pg::quote;
use crate::{Check, Ctx, Handle, Model, Result, Value};

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

/// Runs the model's validations in declaration order, adding to `errors`.
pub(crate) fn run<M: Model>(ctx: &mut Ctx, record: Handle<M>) -> Result<()> {
    for entry in &M::behavior().validations {
        if !entry.applies(ctx, record) {
            continue;
        }
        match &entry.item {
            Validation::Custom(hook) => hook(ctx, record)?,
            Validation::Check { attribute, check } => check_one(ctx, record, attribute, check)?,
        }
    }
    Ok(())
}

fn check_one<M: Model>(ctx: &mut Ctx, record: Handle<M>, attribute: &str, check: &Check) -> Result<()> {
    let value = ctx[record].get(attribute);
    let message = match check {
        Check::Presence => value.is_blank().then(|| "can't be blank".to_string()),
        Check::Length { minimum, maximum } => length_message(&value, *minimum, *maximum),
        Check::Format(regex) => (!regex.is_match(&value.to_ruby_string())).then(|| "is invalid".to_string()),
        Check::Inclusion(allowed) => (!allowed.contains(&value)).then(|| "is not included in the list".to_string()),
        Check::Uniqueness => taken(ctx, record, attribute, value)?.then(|| "has already been taken".to_string()),
        Check::Required { foreign_key, table } => missing(ctx, record, foreign_key, table)?.then(|| "must exist".to_string()),
    };
    if let Some(message) = message {
        ctx.errors_mut(record).add(attribute, message);
    }
    Ok(())
}

/// nil counts as length 0, as `nil.to_s.length` does in Rails.
fn length_message(value: &Value, minimum: Option<usize>, maximum: Option<usize>) -> Option<String> {
    let length = value.to_ruby_string().chars().count();
    let unit = |n: usize| if n == 1 { "character" } else { "characters" };
    if let Some(max) = maximum.filter(|max| length > *max) {
        return Some(format!("is too long (maximum is {max} {})", unit(max)));
    }
    minimum.filter(|min| length < *min).map(|min| format!("is too short (minimum is {min} {})", unit(min)))
}

fn taken<M: Model>(ctx: &mut Ctx, record: Handle<M>, attribute: &str, value: Value) -> Result<bool> {
    let mut others = M::all().where_eq(attribute, value);
    if let Some(id) = ctx.slot(record).saved.as_ref().and_then(|saved| saved.id()) {
        others = others.where_not("id", id);
    }
    others.exists(ctx)
}

/// Rails 7.1+ checks a required `belongs_to` only when the foreign key is
/// nil or changed; then the referenced row has to exist.
fn missing<M: Model>(ctx: &mut Ctx, record: Handle<M>, foreign_key: &str, table: &str) -> Result<bool> {
    let key = ctx[record].get(foreign_key);
    if key.is_nil() {
        return Ok(true);
    }
    if !ctx.attribute_changed(record, foreign_key) {
        return Ok(false);
    }
    let sql = format!("SELECT 1 FROM {} WHERE {} = $1 LIMIT 1", quote(table), quote("id"));
    Ok(ctx.query(&sql, &[key])?.is_empty())
}
