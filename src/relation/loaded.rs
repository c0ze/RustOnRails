//! What a relation answers from its records once it has loaded them, as
//! an `ActiveRecord::Relation` does: `size`, `any?` and `empty?` count
//! them; `count`, `exists?` and the aggregates still ask the database.

use super::Relation;
use crate::{Ctx, FromValue, Handle, Model, Result};

impl<M: Model> Relation<M> {
    pub(crate) fn cached(&self) -> Option<Vec<Handle<M>>> {
        self.loaded.borrow().clone()
    }

    /// `size`: the loaded records' number, or a COUNT query.
    pub fn size(&self, ctx: &mut Ctx) -> Result<i64> {
        match self.cached() {
            Some(records) => Ok(records.len() as i64),
            None => self.count(ctx),
        }
    }

    /// `any?` (and `!empty?`): whether a record was loaded, or exists.
    pub fn is_any(&self, ctx: &mut Ctx) -> Result<bool> {
        match self.cached() {
            Some(records) => Ok(!records.is_empty()),
            None => self.exists(ctx),
        }
    }

    /// `pluck` of a loaded relation reads its records.
    pub(crate) fn pluck_loaded<T: FromValue>(&self, ctx: &Ctx, column: &str) -> Option<Result<Vec<Option<T>>>> {
        let records = self.cached()?;
        Some(records.iter().map(|record| T::from_value(ctx[*record].get(column))).collect())
    }
}
