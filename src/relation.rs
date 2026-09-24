use std::marker::PhantomData;

use postgres::Row;

use crate::pg::{self, quote};
use crate::{Ctx, Error, Handle, Model, Result, Value};

#[derive(Clone, Debug)]
enum Filter {
    Eq(String, Value),
    NotEq(String, Value),
    Gte(String, Value),
}

/// A lazy query, like `ActiveRecord::Relation`: nothing runs until `load`,
/// `first`, `count`, `exists` or a finder.
pub struct Relation<M> {
    filters: Vec<Filter>,
    orders: Vec<(String, &'static str)>,
    limit: Option<i64>,
    marker: PhantomData<fn() -> M>,
}

impl<M> Clone for Relation<M> {
    fn clone(&self) -> Self {
        Self { filters: self.filters.clone(), orders: self.orders.clone(), limit: self.limit, marker: PhantomData }
    }
}

impl<M: Model> Default for Relation<M> {
    fn default() -> Self {
        Self { filters: Vec::new(), orders: Vec::new(), limit: None, marker: PhantomData }
    }
}

impl<M: Model> Relation<M> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `where(column: value)`. Enum columns take the label, as in Rails.
    pub fn where_eq(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.filters.push(Filter::Eq(column.to_string(), value.into()));
        self
    }

    /// `where.not(column: value)`
    pub fn where_not(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.filters.push(Filter::NotEq(column.to_string(), value.into()));
        self
    }

    /// `where(column: value..)`
    pub fn where_gte(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.filters.push(Filter::Gte(column.to_string(), value.into()));
        self
    }

    pub fn order_asc(mut self, column: &str) -> Self {
        self.orders.push((column.to_string(), "ASC"));
        self
    }

    pub fn order_desc(mut self, column: &str) -> Self {
        self.orders.push((column.to_string(), "DESC"));
        self
    }

    pub fn limit(mut self, n: i64) -> Self {
        self.limit = Some(n);
        self
    }

    /// The SELECT this relation runs, with its parameters.
    pub fn to_sql(&self) -> (String, Vec<Value>) {
        let table = quote(M::TABLE);
        let mut sql = format!("SELECT {table}.* FROM {table}");
        let mut params = Vec::new();
        for (i, filter) in self.filters.iter().enumerate() {
            sql.push_str(if i == 0 { " WHERE " } else { " AND " });
            let (column, op, value) = match filter {
                Filter::Eq(c, v) => (c, "=", v),
                Filter::NotEq(c, v) => (c, "<>", v),
                Filter::Gte(c, v) => (c, ">=", v),
            };
            let target = format!("{table}.{}", quote(column));
            match M::behavior().to_database(column, value.clone()) {
                Value::Nil if op == "<>" => sql.push_str(&format!("{target} IS NOT NULL")),
                Value::Nil => sql.push_str(&format!("{target} IS NULL")),
                value => {
                    params.push(value);
                    sql.push_str(&format!("{target} {op} ${}", params.len()));
                }
            }
        }
        if !self.orders.is_empty() {
            let orders: Vec<String> = self.orders.iter().map(|(c, dir)| format!("{table}.{} {dir}", quote(c))).collect();
            sql.push_str(&format!(" ORDER BY {}", orders.join(", ")));
        }
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        (sql, params)
    }

    pub(crate) fn fetch(&self, ctx: &mut Ctx) -> Result<Vec<M>> {
        let (sql, params) = self.to_sql();
        ctx.query(&sql, &params)?.iter().map(from_row::<M>).collect()
    }

    pub fn load(&self, ctx: &mut Ctx) -> Result<Vec<Handle<M>>> {
        Ok(self.fetch(ctx)?.into_iter().map(|record| ctx.adopt(record)).collect())
    }

    /// `first`: orders by id unless the relation has an order already.
    pub fn first(&self, ctx: &mut Ctx) -> Result<Option<Handle<M>>> {
        let mut relation = self.clone().limit(1);
        if relation.orders.is_empty() {
            relation = relation.order_asc("id");
        }
        Ok(relation.load(ctx)?.into_iter().next())
    }

    pub fn count(&self, ctx: &mut Ctx) -> Result<i64> {
        let (sql, params) = self.to_sql();
        let rows = ctx.query(&format!("SELECT COUNT(*) FROM ({sql}) AS subquery"), &params)?;
        Ok(rows[0].get(0))
    }

    pub fn exists(&self, ctx: &mut Ctx) -> Result<bool> {
        Ok(!self.clone().limit(1).fetch(ctx)?.is_empty())
    }

    pub fn find(&self, ctx: &mut Ctx, id: i64) -> Result<Handle<M>> {
        let found = self.clone().where_eq("id", id).limit(1).load(ctx)?.into_iter().next();
        found.ok_or_else(|| Error::RecordNotFound { model: M::NAME, conditions: Some(format!("'id'={id}")) })
    }

    pub fn find_by(&self, ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Option<Handle<M>>> {
        Ok(self.clone().where_eq(column, value).limit(1).load(ctx)?.into_iter().next())
    }

    pub fn find_by_bang(&self, ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Handle<M>> {
        self.find_by(ctx, column, value)?.ok_or(Error::RecordNotFound { model: M::NAME, conditions: None })
    }
}

/// Builds a record from a row, turning enum integers back into labels.
pub(crate) fn from_row<M: Model>(row: &Row) -> Result<M> {
    let mut record = M::default();
    for (index, column) in row.columns().iter().enumerate() {
        let value = M::behavior().from_database(column.name(), pg::read(row, index)?);
        record.set(column.name(), value)?;
    }
    Ok(record)
}
