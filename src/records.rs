use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};
use std::sync::LazyLock;

use postgres::Row;
use regex::Regex;

use crate::{Ctx, Errors, Model, Result, Value, pg};

/// Refers to a record loaded into a `Ctx`, the way a Ruby variable refers
/// to an object: copying a handle doesn't copy the record.
pub struct Handle<M> {
    index: u32,
    marker: PhantomData<fn() -> M>,
}

impl<M> Handle<M> {
    pub(crate) fn from_index(index: u32) -> Self {
        Self { index, marker: PhantomData }
    }

    pub(crate) fn index(self) -> u32 {
        self.index
    }
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
    /// Loaded `belongs_to` targets by association name, with the foreign
    /// key each was loaded for: Rails' association cache.
    pub associations: HashMap<&'static str, (Value, u32)>,
}

impl<M> Slot<M> {
    pub fn new(record: M, saved: Option<M>) -> Self {
        Self { record, saved, errors: Errors::default(), destroyed: false, associations: HashMap::new() }
    }
}

impl Ctx {
    /// `Post.new(...)`: adds an unsaved record, after the model's
    /// `has_secure_token`s fill in the tokens it wasn't given, as Rails'
    /// after_initialize does.
    pub fn build<M: Model>(&mut self, mut record: M) -> Handle<M> {
        for (attribute, length) in &M::behavior().tokens {
            crate::secure_token::fill(&mut record, attribute, *length).expect("has_secure_token names a String column");
        }
        self.push(Slot::new(record, None))
    }

    /// Adds a record just read from the database.
    pub(crate) fn adopt<M: Model>(&mut self, record: M) -> Handle<M> {
        let saved = Some(record.clone());
        self.push(Slot::new(record, saved))
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
        M::COLUMNS
            .iter()
            .copied()
            .filter(|c| {
                let (old, new) = (base.get(c), slot.record.get(c));
                old != new || number_to_non_number(&old, &new, slot.record.before_type_cast().given(c, &new))
            })
            .collect()
    }

    /// `attribute_changed?(column)`
    pub fn attribute_changed<M: Model>(&self, record: Handle<M>, column: &str) -> bool {
        self.changed(record).contains(&column)
    }

    /// `assign_attributes(attributes)` on a record in this `Ctx`.
    pub fn assign<M: Model>(&mut self, record: Handle<M>, attributes: &[(String, Value)]) -> Result<()> {
        crate::model::assign_to(&mut self[record], attributes)
    }

    /// The cached target of `name`, if it was loaded for this foreign key.
    pub(crate) fn cached<M: Model>(&self, owner: Handle<M>, name: &str, key: &Value) -> Option<u32> {
        self.slot(owner).associations.get(name).filter(|(k, _)| k == key).map(|(_, index)| *index)
    }

    pub(crate) fn cache<M: Model>(&mut self, owner: Handle<M>, name: &'static str, key: Value, index: u32) {
        self.slot_mut(owner).associations.insert(name, (key, index));
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

/// Ruby's `/\A\s*[+-]?\d/`, with Ruby's ASCII `\s` and `\d`.
static NUMERIC_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t\r\n\x0b\x0c]*[+-]?[0-9]").expect("regex"));

/// Active Model's numeric types also count as changed a number given a
/// value that doesn't start like one ("abc", false), though both cast to 0.
fn number_to_non_number(old: &Value, new: &Value, given: Option<&Value>) -> bool {
    let non_number = match given {
        Some(Value::Str(s)) => !NUMERIC_START.is_match(s),
        Some(Value::Bool(_)) => true,
        _ => false,
    };
    matches!(new, Value::Int(_) | Value::Float(_)) && !old.is_nil() && non_number
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
