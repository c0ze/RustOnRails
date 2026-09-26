//! Aggregates, `pluck` and `find_each`: what a relation computes in SQL.

use super::{Filter, Relation};
use crate::pg::{quote, read};
use crate::{Ctx, FromValue, Handle, Model, Result, Value};

impl<M: Model> Relation<M> {
    /// `sum(:column)`: 0 when no row matches. The value is cast by the
    /// column's type, as Rails deserializes it.
    pub fn sum<T: FromValue + Default>(&self, ctx: &mut Ctx, column: &str) -> Result<T> {
        Ok(self.aggregate::<T>(ctx, "SUM", column)?.unwrap_or_default())
    }

    /// `minimum(:column)`: nil when no row matches.
    pub fn minimum<T: FromValue>(&self, ctx: &mut Ctx, column: &str) -> Result<Option<T>> {
        self.aggregate(ctx, "MIN", column)
    }

    /// `maximum(:column)`
    pub fn maximum<T: FromValue>(&self, ctx: &mut Ctx, column: &str) -> Result<Option<T>> {
        self.aggregate(ctx, "MAX", column)
    }

    /// `SELECT FUNCTION("table"."column") ...` without the order, keeping
    /// the limit and offset as Rails does: a limit leaves the one row, an
    /// offset past it leaves none, which reads as nil.
    fn aggregate<T: FromValue>(&self, ctx: &mut Ctx, function: &str, column: &str) -> Result<Option<T>> {
        let (sql, params) = self.select_sql(&format!("{function}({}.{})", quote(M::TABLE), quote(column)), false);
        let rows = self.run(ctx, &sql, &params)?;
        let value = match rows.first() {
            Some(row) => M::behavior().from_database(column, read(row, 0)?),
            None => Value::Nil,
        };
        T::from_value(value)
    }

    /// `pluck(:column)`: the column of each row, in the relation's order,
    /// enum integers as their labels.
    pub fn pluck<T: FromValue>(&self, ctx: &mut Ctx, column: &str) -> Result<Vec<Option<T>>> {
        let (sql, params) = self.select_sql(&format!("{}.{}", quote(M::TABLE), quote(column)), true);
        let rows = self.run(ctx, &sql, &params)?;
        rows.iter().map(|row| T::from_value(M::behavior().from_database(column, read(row, 0)?))).collect()
    }

    /// `find_each(batch_size: size)`: the records in batches by id, each
    /// batch a query of its own, so memory holds one batch at a time.
    pub fn batches(&self, size: i64) -> Batches<M> {
        Batches { remaining: self.limit, relation: self.clone(), size, last: None, done: false }
    }
}

/// Rails' `in_batches` over an unloaded relation: ordered by id (any
/// order the relation had is ignored, as Rails does with a warning), each
/// batch after the last id seen, and a limit on the relation caps the
/// records across all batches.
pub struct Batches<M: 'static> {
    relation: Relation<M>,
    size: i64,
    remaining: Option<i64>,
    last: Option<i64>,
    done: bool,
}

impl<M: Model> Batches<M> {
    /// The next batch, loaded into the `Ctx` with its `includes`, or None
    /// when there are no more.
    pub fn next(&mut self, ctx: &mut Ctx) -> Result<Option<Vec<Handle<M>>>> {
        if self.done {
            return Ok(None);
        }
        let limit = self.remaining.map_or(self.size, |remaining| remaining.min(self.size));
        let mut batch = self.relation.clone();
        batch.orders = vec![("id".to_string(), "ASC")];
        batch.limit = Some(limit);
        if let Some(last) = self.last {
            batch.filters.push(Filter::Gt("id".to_string(), Value::Int(last)));
        }
        let records = batch.load(ctx)?;
        let found = records.len() as i64;
        self.last = records.last().and_then(|record| ctx[*record].id());
        if let Some(remaining) = &mut self.remaining {
            *remaining -= found;
        }
        self.done = found < limit || self.remaining == Some(0);
        Ok(if records.is_empty() { None } else { Some(records) })
    }
}
