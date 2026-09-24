use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use postgres::types::ToSql;
use postgres::{Client, NoTls, Row};

use crate::{Errors, Model, Result, Value};

/// Refers to a record loaded into a `Ctx`, the way a Ruby variable refers
/// to an object: copying a handle doesn't copy the record.
pub struct Handle<M> {
    index: u32,
    marker: PhantomData<fn() -> M>,
}

impl<M> Clone for Handle<M> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M> Copy for Handle<M> {}
impl<M> PartialEq for Handle<M> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}
impl<M> Eq for Handle<M> {}
impl<M> std::fmt::Debug for Handle<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Handle({})", self.index)
    }
}

/// What the record table keeps for each record.
pub(crate) struct Slot<M> {
    pub record: M,
    /// The record as last loaded or saved; `None` until it's in the database.
    pub saved: Option<M>,
    pub errors: Errors,
    pub destroyed: bool,
}

/// One unit of work: a database connection plus every record it loaded or
/// built. Rails' equivalent is the objects a request creates; they all go
/// away when the `Ctx` is dropped. There's no identity map: like Rails,
/// every load makes a new record.
pub struct Ctx {
    client: Client,
    depth: u32,
    tables: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl Ctx {
    pub fn connect(url: &str) -> Result<Self> {
        Ok(Self::new(Client::connect(url, NoTls)?))
    }

    pub fn new(client: Client) -> Self {
        Self { client, depth: 0, tables: HashMap::new() }
    }

    /// Opens a transaction that is never committed; dropping the `Ctx`
    /// rolls it back. For tests, like Rails' transactional fixtures.
    pub fn rolled_back(mut client: Client) -> Result<Self> {
        client.batch_execute("BEGIN")?;
        Ok(Self { client, depth: 1, tables: HashMap::new() })
    }

    pub fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        Ok(self.client.query(sql, &refs(params))?)
    }

    pub fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        Ok(self.client.execute(sql, &refs(params))?)
    }

    /// `transaction do ... end`, keeping the block's outcome the way `save`
    /// does: `Ok(false)` or an error rolls back. Nested calls use savepoints.
    pub fn transaction(&mut self, block: impl FnOnce(&mut Ctx) -> Result<bool>) -> Result<bool> {
        let name = format!("rustonrails_{}", self.depth);
        let (begin, commit, rollback) = if self.depth == 0 {
            ("BEGIN".to_string(), "COMMIT".to_string(), "ROLLBACK".to_string())
        } else {
            (format!("SAVEPOINT {name}"), format!("RELEASE SAVEPOINT {name}"), format!("ROLLBACK TO SAVEPOINT {name}"))
        };
        self.client.batch_execute(&begin)?;
        self.depth += 1;
        let outcome = block(self);
        self.depth -= 1;
        match outcome {
            Ok(true) => {
                self.client.batch_execute(&commit)?;
                Ok(true)
            }
            Ok(false) => {
                self.client.batch_execute(&rollback)?;
                Ok(false)
            }
            Err(error) => {
                // The original error says more than a failed rollback would.
                self.client.batch_execute(&rollback).ok();
                Err(error)
            }
        }
    }
}

impl Ctx {
    /// `Post.new(...)`: adds an unsaved record.
    pub fn build<M: Model>(&mut self, record: M) -> Handle<M> {
        self.push(Slot { record, saved: None, errors: Errors::default(), destroyed: false })
    }

    /// Adds a record just read from the database.
    pub(crate) fn adopt<M: Model>(&mut self, record: M) -> Handle<M> {
        let saved = Some(record.clone());
        self.push(Slot { record, saved, errors: Errors::default(), destroyed: false })
    }

    fn push<M: Model>(&mut self, slot: Slot<M>) -> Handle<M> {
        let slots = self.slots_mut::<M>();
        slots.push(slot);
        Handle { index: (slots.len() - 1) as u32, marker: PhantomData }
    }

    fn slots_mut<M: Model>(&mut self) -> &mut Vec<Slot<M>> {
        self.tables
            .entry(TypeId::of::<M>())
            .or_insert_with(|| Box::new(Vec::<Slot<M>>::new()))
            .downcast_mut()
            .expect("record table holds one model type")
    }

    pub(crate) fn slot<M: Model>(&self, record: Handle<M>) -> &Slot<M> {
        let slots: &Vec<Slot<M>> = self
            .tables
            .get(&TypeId::of::<M>())
            .and_then(|table| table.downcast_ref())
            .expect("handle belongs to this Ctx");
        &slots[record.index as usize]
    }

    pub(crate) fn slot_mut<M: Model>(&mut self, record: Handle<M>) -> &mut Slot<M> {
        &mut self.slots_mut::<M>()[record.index as usize]
    }

    pub fn errors<M: Model>(&self, record: Handle<M>) -> &Errors {
        &self.slot(record).errors
    }

    pub fn errors_mut<M: Model>(&mut self, record: Handle<M>) -> &mut Errors {
        &mut self.slot_mut(record).errors
    }

    pub fn is_new_record<M: Model>(&self, record: Handle<M>) -> bool {
        self.slot(record).saved.is_none()
    }

    pub fn is_persisted<M: Model>(&self, record: Handle<M>) -> bool {
        let slot = self.slot(record);
        slot.saved.is_some() && !slot.destroyed
    }

    pub fn is_destroyed<M: Model>(&self, record: Handle<M>) -> bool {
        self.slot(record).destroyed
    }

    /// `changed`: columns that differ from the last saved state, or from
    /// the column defaults for a record that was never saved.
    pub fn changed<M: Model>(&self, record: Handle<M>) -> Vec<&'static str> {
        let slot = self.slot(record);
        let base = slot.saved.clone().unwrap_or_else(M::new_record);
        M::COLUMNS.iter().copied().filter(|c| base.get(c) != slot.record.get(c)).collect()
    }

    /// `attribute_changed?(column)`
    pub fn attribute_changed<M: Model>(&self, record: Handle<M>, column: &str) -> bool {
        self.changed(record).contains(&column)
    }
}

impl<M: Model> Index<Handle<M>> for Ctx {
    type Output = M;

    fn index(&self, record: Handle<M>) -> &M {
        &self.slot(record).record
    }
}

impl<M: Model> IndexMut<Handle<M>> for Ctx {
    fn index_mut(&mut self, record: Handle<M>) -> &mut M {
        &mut self.slot_mut(record).record
    }
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
}
