use std::marker::PhantomData;

use crate::association::Preload;
use crate::pg::quote;
use crate::records::from_row;
use crate::{Ctx, Error, Handle, Model, Result, Value};

#[derive(Clone, Debug)]
enum Filter {
    Eq(String, Value),
    NotEq(String, Value),
    Gte(String, Value),
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
        let value = J::behavior().to_database(column, J::cast_query(column, value.into()));
        self.filters.push(Filter::EqOn(J::TABLE, column.to_string(), value));
        self
    }

    /// `where("title ILIKE ?", pattern)`: a SQL fragment, parenthesized as
    /// Rails does, each `?` bound in order. Rutile counts them when it
    /// compiles the call.
    pub fn where_sql(mut self, sql: &'static str, binds: Vec<Value>) -> Self {
        assert_eq!(sql.matches('?').count(), binds.len(), "`{sql}` has a different number of binds");
        self.filters.push(Filter::Sql(sql, binds));
        self
    }

    /// The SELECT this relation runs, with its parameters. It names the
    /// model's columns rather than `*`, so a column the model doesn't know
    /// (added by a later migration) doesn't break loading.
    pub fn to_sql(&self) -> (String, Vec<Value>) {
        let table = quote(M::TABLE);
        let columns: Vec<String> = M::COLUMNS.iter().map(|c| format!("{table}.{}", quote(c))).collect();
        let mut sql = format!("SELECT {} FROM {table}", columns.join(", "));
        let mut params = Vec::new();
        let mut conditions = 0;
        if let Some(join) = &self.join {
            let through = quote(join.table);
            sql.push_str(&format!(" INNER JOIN {through} ON {table}.{} = {through}.{}", quote("id"), quote(join.target_key)));
            // An owner without an id (unsaved) has nothing through it, as Rails' `none`.
            if join.owner_id.is_nil() {
                sql.push_str(" WHERE 1=0");
            } else {
                params.push(join.owner_id.clone());
                sql.push_str(&format!(" WHERE {through}.{} = ${}", quote(join.owner_key), params.len()));
            }
            conditions += 1;
        }
        for join in &self.joins {
            let joined = quote(join.table);
            sql.push_str(&format!(
                " INNER JOIN {joined} ON {joined}.{} = {}.{}",
                quote(join.column),
                quote(join.other),
                quote(join.other_column)
            ));
        }
        for filter in &self.filters {
            sql.push_str(if conditions == 0 { " WHERE " } else { " AND " });
            conditions += 1;
            let (column, op, value) = match filter {
                Filter::In(column, values) => {
                    let target = format!("{table}.{}", quote(column));
                    let cast: Vec<Value> = values
                        .iter()
                        .map(|v| M::behavior().to_database(column, M::cast_query(column, v.clone())))
                        .filter(|v| !v.is_nil())
                        .collect();
                    if cast.is_empty() {
                        sql.push_str("1=0");
                    } else {
                        let start = params.len();
                        params.extend(cast);
                        let marks: Vec<String> = (start + 1..=params.len()).map(|i| format!("${i}")).collect();
                        sql.push_str(&format!("{target} IN ({})", marks.join(", ")));
                    }
                    continue;
                }
                Filter::EqOn(joined, column, value) => {
                    let target = format!("{}.{}", quote(joined), quote(column));
                    if value.is_nil() {
                        sql.push_str(&format!("{target} IS NULL"));
                    } else {
                        params.push(value.clone());
                        sql.push_str(&format!("{target} = ${}", params.len()));
                    }
                    continue;
                }
                Filter::Sql(fragment, binds) => {
                    let mut parts = fragment.split('?');
                    sql.push('(');
                    sql.push_str(parts.next().unwrap_or_default());
                    for (part, bind) in parts.zip(binds) {
                        params.push(bind.clone());
                        sql.push_str(&format!("${}{part}", params.len()));
                    }
                    sql.push(')');
                    continue;
                }
                Filter::Eq(c, v) => (c, "=", v),
                Filter::NotEq(c, v) => (c, "<>", v),
                Filter::Gte(c, v) => (c, ">=", v),
            };
            let target = format!("{table}.{}", quote(column));
            match M::behavior().to_database(column, M::cast_query(column, value.clone())) {
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
        if let Some(n) = self.offset {
            sql.push_str(&format!(" OFFSET {n}"));
        }
        (sql, params)
    }

    pub(crate) fn fetch(&self, ctx: &mut Ctx) -> Result<Vec<M>> {
        let (sql, params) = self.to_sql();
        ctx.query(&sql, &params)?.iter().map(from_row::<M>).collect()
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

    pub fn count(&self, ctx: &mut Ctx) -> Result<i64> {
        let (sql, params) = self.to_sql();
        let rows = ctx.query(&format!("SELECT COUNT(*) FROM ({sql}) AS subquery"), &params)?;
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

    pub fn exists(&self, ctx: &mut Ctx) -> Result<bool> {
        Ok(!self.clone().limit(1).fetch(ctx)?.is_empty())
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

/// Rails' `sanitize_sql_like` with its default escape character: each
/// backslash doubled, then one before each `%` and `_`, so the string
/// matches itself inside a LIKE pattern.
pub fn sanitize_sql_like(string: &str) -> String {
    let mut out = String::with_capacity(string.len());
    for c in string.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}
