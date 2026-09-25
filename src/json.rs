use chrono::Datelike;
use serde_json::{Map, Value as Json};

use crate::{BelongsTo, Ctx, Date, Errors, HasMany, Handle, Model, Result, Time, Value};

/// ActiveSupport's JSON time format: ISO 8601 in UTC with milliseconds.
pub fn format_time(time: Time) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// `Date#to_s` and its JSON: Ruby's `%Y-%m-%d`, whose year has at least
/// four digits and no `+` (chrono's `%Y` writes "+10000").
pub fn format_date(date: Date) -> String {
    let sign = if date.year() < 0 { "-" } else { "" };
    format!("{sign}{:04}-{:02}-{:02}", date.year().unsigned_abs(), date.month(), date.day())
}

/// An attribute value the way `as_json` writes it.
pub fn value_json(value: Value) -> Json {
    match value {
        Value::Nil => Json::Null,
        Value::Bool(b) => Json::Bool(b),
        Value::Int(i) => Json::from(i),
        Value::Float(f) => serde_json::Number::from_f64(f).map_or(Json::Null, Json::Number),
        Value::Str(s) => Json::String(s),
        Value::Time(t) => Json::String(format_time(t)),
        Value::Date(d) => Json::String(format_date(d)),
    }
}

/// `errors.as_json`: messages by attribute, attributes in first-error order.
pub fn errors_json(errors: &Errors) -> Json {
    let mut map = Map::new();
    for attribute in errors.attributes() {
        map.insert(attribute.to_string(), errors.on(attribute).into_iter().map(Json::from).collect());
    }
    Json::Object(map)
}

/// `record.as_json(only:, except:, include:)`: columns in table order, then
/// included associations.
pub struct AsJson<M> {
    only: Option<&'static [&'static str]>,
    except: &'static [&'static str],
    include: Vec<Box<dyn Nested<M>>>,
}

impl<M: Model> Default for AsJson<M> {
    fn default() -> Self {
        Self { only: None, except: &[], include: Vec::new() }
    }
}

impl<M: Model> AsJson<M> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn only(mut self, columns: &'static [&'static str]) -> Self {
        self.only = Some(columns);
        self
    }

    pub fn except(mut self, columns: &'static [&'static str]) -> Self {
        self.except = columns;
        self
    }

    /// `include: { user: { ... } }` for a `belongs_to`.
    pub fn include<T: Model>(mut self, association: &'static BelongsTo<M, T>, options: AsJson<T>) -> Self {
        self.include.push(Box::new(One { association, options }));
        self
    }

    /// `include: { comments: { ... } }` for a `has_many`.
    pub fn include_many<T: Model>(mut self, association: &'static HasMany<M, T>, options: AsJson<T>) -> Self {
        self.include.push(Box::new(Many { association, options }));
        self
    }

    pub fn render(&self, ctx: &mut Ctx, record: Handle<M>) -> Result<Json> {
        let mut map = Map::new();
        for column in M::COLUMNS {
            if self.only.is_some_and(|only| !only.contains(column)) || self.except.contains(column) {
                continue;
            }
            map.insert(column.to_string(), value_json(ctx[record].get(column)));
        }
        for nested in &self.include {
            map.insert(nested.key().to_string(), nested.render(ctx, record)?);
        }
        Ok(Json::Object(map))
    }

    /// `render json: @post` when `@post` may be nil, which renders `null`.
    pub fn render_option(&self, ctx: &mut Ctx, record: Option<Handle<M>>) -> Result<Json> {
        record.map_or(Ok(Json::Null), |record| self.render(ctx, record))
    }

    pub fn render_all(&self, ctx: &mut Ctx, records: &[Handle<M>]) -> Result<Json> {
        let rendered = records.iter().map(|record| self.render(ctx, *record)).collect::<Result<Vec<_>>>()?;
        Ok(Json::Array(rendered))
    }
}

trait Nested<M> {
    fn key(&self) -> &'static str;
    fn render(&self, ctx: &mut Ctx, owner: Handle<M>) -> Result<Json>;
}

struct One<M: 'static, T: 'static> {
    association: &'static BelongsTo<M, T>,
    options: AsJson<T>,
}

impl<M: Model, T: Model> Nested<M> for One<M, T> {
    fn key(&self) -> &'static str {
        self.association.name
    }

    fn render(&self, ctx: &mut Ctx, owner: Handle<M>) -> Result<Json> {
        match self.association.get(ctx, owner)? {
            Some(target) => self.options.render(ctx, target),
            None => Ok(Json::Null),
        }
    }
}

struct Many<M: 'static, T: 'static> {
    association: &'static HasMany<M, T>,
    options: AsJson<T>,
}

impl<M: Model, T: Model> Nested<M> for Many<M, T> {
    fn key(&self) -> &'static str {
        self.association.name
    }

    fn render(&self, ctx: &mut Ctx, owner: Handle<M>) -> Result<Json> {
        let children = self.association.of(ctx, owner).load(ctx)?;
        self.options.render_all(ctx, &children)
    }
}
