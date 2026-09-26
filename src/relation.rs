use std::marker::PhantomData;

use crate::association::Preload;
use crate::records::from_row;
use crate::{Ctx, Error, Handle, Model, Result, Value};

mod calculate;
mod sql;

pub use calculate::Batches;
pub use sql::sanitize_sql_like;

#[derive(Clone, Debug)]
enum Filter {
    Eq(String, Value),
    NotEq(String, Value),
    Gte(String, Value),
    /// `where(id: (last + 1)..)` as batches write it: `id > last`.
    Gt(String, Value),
    In(String, Vec<Value>),
    /// `where(memberships: { user_id: 1 })`: a joined table's column, the
    /// value already cast by that table's model.
    EqOn(&'static str, String, Value),
    /// `where("title ILIKE ?", pattern)`
    Sql(&'static str, Vec<Value>),
}

/// `has_many :through`'s inner join: the join table, its key to the
/// target, and its key to the owner with the owner's id.
#[derive(Clone, Debug)]
struct Join {
    table: &'static str,
    target_key: &'static str,
    owner_key: &'static str,
    owner_id: Value,
}

/// `joins(:project)`: `INNER JOIN table ON table.column = other.other_column`,
/// the keys of one association.
#[derive(Clone, Copy, Debug)]
pub struct InnerJoin {
    pub(crate) table: &'static str,
    pub(crate) column: &'static str,
    pub(crate) other: &'static str,
    pub(crate) other_column: &'static str,
}

/// An association `joins` can follow.
pub trait Joinable {
    fn inner_join(&self) -> InnerJoin;
}

/// A lazy query, like `ActiveRecord::Relation`: nothing runs until `load`,
/// `first`, `count`, `exists` or a finder.
pub struct Relation<M: 'static> {
    join: Option<Join>,
    joins: Vec<InnerJoin>,
    filters: Vec<Filter>,
    orders: Vec<(String, &'static str)>,
    limit: Option<i64>,
    offset: Option<i64>,
    includes: Vec<&'static dyn Preload<M>>,
    marker: PhantomData<fn() -> M>,
}

impl<M: 'static> Clone for Relation<M> {
    fn clone(&self) -> Self {
        Self {
            join: self.join.clone(),
            joins: self.joins.clone(),
            filters: self.filters.clone(),
            orders: self.orders.clone(),
            limit: self.limit,
            offset: self.offset,
            includes: self.includes.clone(),
            marker: PhantomData,
        }
    }
}

impl<M: Model> Default for Relation<M> {
    fn default() -> Self {
        Self {
            join: None,
            joins: Vec::new(),
            filters: Vec::new(),
            orders: Vec::new(),
            limit: None,
            offset: None,
            includes: Vec::new(),
            marker: PhantomData,
        }
    }
}

impl<M: Model> Relation<M> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows of `table` whose `owner_key` is `owner_id`, joined to this
    /// relation's table on `target_key`: what `has_many :through` builds.
    pub fn join_through(mut self, table: &'static str, target_key: &'static str, owner_key: &'static str, owner_id: Value) -> Self {
        self.join = Some(Join { table, target_key, owner_key, owner_id });
        self
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

    /// `where(column: [a, b])`; an empty list matches nothing (`1=0`).
    pub fn where_in(mut self, column: &str, values: Vec<Value>) -> Self {
        self.filters.push(Filter::In(column.to_string(), values));
        self
    }

    /// `includes(:user)`: preloads the association after `load`.
    pub fn includes(mut self, association: &'static dyn Preload<M>) -> Self {
        self.includes.push(association);
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

    /// `offset(n)`: skips `n` rows after the order.
    pub fn offset(mut self, n: i64) -> Self {
        self.offset = Some(n);
        self
    }

    /// `joins(:project)`; `joins(project: :memberships)` is two calls.
    pub fn joins(mut self, association: &impl Joinable) -> Self {
        self.joins.push(association.inner_join());
        self
    }

    /// `where(memberships: { user_id: 1 })`: a column of a joined table,
    /// cast by that table's model.
    pub fn where_on<J: Model>(mut self, column: &str, value: impl Into<Value>) -> Self {
        let value = J::behavior().query_value(column, J::cast_query(column, value.into()));
        self.filters.push(Filter::EqOn(J::TABLE, column.to_string(), value));
        self
    }

    /// `where("title ILIKE ?", pattern)`: a SQL fragment, parenthesized as
    /// Rails does, each `?` replaced by its bind quoted as Rails' `quote`
    /// would. Rutile counts them when it compiles the call.
    pub fn where_sql(mut self, sql: &'static str, binds: Vec<Value>) -> Self {
        assert_eq!(sql.matches('?').count(), binds.len(), "`{sql}` has a different number of binds");
        self.filters.push(Filter::Sql(sql, binds));
        self
    }

    pub(crate) fn fetch(&self, ctx: &mut Ctx) -> Result<Vec<M>> {
        let (sql, params) = self.to_sql();
        self.run(ctx, &sql, &params)?.iter().map(from_row::<M>).collect()
    }

    /// A relation with a SQL fragment has its binds written into the SQL,
    /// so its statement isn't kept: Rails doesn't prepare one either.
    fn run(&self, ctx: &mut Ctx, sql: &str, params: &[Value]) -> Result<Vec<postgres::Row>> {
        if self.filters.iter().any(|filter| matches!(filter, Filter::Sql(..))) {
            ctx.query_once(sql, params)
        } else {
            ctx.query(sql, params)
        }
    }

    pub fn load(&self, ctx: &mut Ctx) -> Result<Vec<Handle<M>>> {
        let handles: Vec<Handle<M>> = self.fetch(ctx)?.into_iter().map(|record| ctx.adopt(record)).collect();
        for association in &self.includes {
            association.preload(ctx, &handles)?;
        }
        Ok(handles)
    }

    /// `first`: orders by id unless the relation has an order already.
    pub fn first(&self, ctx: &mut Ctx) -> Result<Option<Handle<M>>> {
        let mut relation = self.clone().limit(1);
        if relation.orders.is_empty() {
            relation = relation.order_asc("id");
        }
        Ok(relation.load(ctx)?.into_iter().next())
    }

    /// `count`: `SELECT COUNT(*)` without the order; with a limit or an
    /// offset, the count of a subquery that keeps them, as Rails writes it.
    /// `limit(0)` is 0 without a query.
    pub fn count(&self, ctx: &mut Ctx) -> Result<i64> {
        if self.limit == Some(0) {
            return Ok(0);
        }
        let (sql, params) = if self.limit.is_some() || self.offset.is_some() {
            let (inner, params) = self.select_sql("1 AS one", true);
            (format!("SELECT COUNT(*) FROM ({inner}) subquery_for_count"), params)
        } else {
            self.select_sql("COUNT(*)", false)
        };
        let rows = self.run(ctx, &sql, &params)?;
        Ok(rows[0].get(0))
    }

    /// `include?(record)` on a relation that isn't loaded: an `exists?`
    /// query on the record's id. Nil, or a record without an id, is false.
    pub fn contains(&self, ctx: &mut Ctx, record: impl Into<Option<Handle<M>>>) -> Result<bool> {
        let Some(id) = record.into().and_then(|record| ctx[record].id()) else { return Ok(false) };
        // Like Rails, a relation with a limit or offset is loaded and
        // searched: filtering by the id first would change which rows they keep.
        if self.limit.is_some() || self.offset.is_some() {
            return Ok(self.fetch(ctx)?.iter().any(|row| row.id() == Some(id)));
        }
        self.clone().where_eq("id", id).exists(ctx)
    }

    /// `exists?`: `SELECT 1 AS one ... LIMIT 1`, without the order and
    /// keeping any offset. `limit(0)` exists nowhere, without a query.
    pub fn exists(&self, ctx: &mut Ctx) -> Result<bool> {
        if self.limit == Some(0) {
            return Ok(false);
        }
        let (sql, params) = self.clone().limit(1).select_sql("1 AS one", false);
        Ok(!self.run(ctx, &sql, &params)?.is_empty())
    }

    /// `find(id)`: the id is cast like any query value, so a param string
    /// works and one that isn't a number finds nothing.
    pub fn find(&self, ctx: &mut Ctx, id: impl Into<Value>) -> Result<Handle<M>> {
        let id = id.into();
        let found = self.clone().where_eq("id", id.clone()).limit(1).load(ctx)?.into_iter().next();
        let conditions = Some(format!("'id'={}", id.to_ruby_string()));
        found.ok_or(Error::RecordNotFound { model: M::NAME, conditions })
    }

    pub fn find_by(&self, ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Option<Handle<M>>> {
        Ok(self.clone().where_eq(column, value).limit(1).load(ctx)?.into_iter().next())
    }

    pub fn find_by_bang(&self, ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Handle<M>> {
        self.find_by(ctx, column, value)?.ok_or(Error::RecordNotFound { model: M::NAME, conditions: None })
    }
}
