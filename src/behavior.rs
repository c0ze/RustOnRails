use regex::Regex;

use crate::{Ctx, Handle, Record, Result, Value};

/// A callback or a custom validation, like `before_save :stamp_published_at`.
pub type Hook<M> = fn(&mut Ctx, Handle<M>) -> Result<()>;
/// An `if:` or `unless:` condition.
pub type Cond<M> = fn(&Ctx, Handle<M>) -> bool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    BeforeValidation,
    AfterValidation,
    BeforeSave,
    AfterSave,
    BeforeCreate,
    AfterCreate,
    BeforeUpdate,
    AfterUpdate,
    BeforeDestroy,
    AfterDestroy,
}

/// The built-in validators, one per `validates` option.
pub enum Check {
    Presence,
    Length { minimum: Option<usize>, maximum: Option<usize> },
    Format(Regex),
    Uniqueness,
    Inclusion(Vec<Value>),
    /// What `belongs_to` adds unless `optional: true`: the row must exist.
    Required { foreign_key: &'static str, table: &'static str },
}

pub(crate) enum Validation<M> {
    Check { attribute: &'static str, check: Check },
    Custom(Hook<M>),
}

/// An entry with its `if:` / `unless:` conditions.
pub(crate) struct Guarded<T, M> {
    pub item: T,
    when: Vec<Cond<M>>,
    unless: Vec<Cond<M>>,
}

impl<T, M> Guarded<T, M> {
    fn new(item: T) -> Self {
        Self { item, when: Vec::new(), unless: Vec::new() }
    }

    pub fn applies(&self, ctx: &Ctx, record: Handle<M>) -> bool {
        self.when.iter().all(|c| c(ctx, record)) && !self.unless.iter().any(|c| c(ctx, record))
    }
}

struct EnumDef {
    attribute: &'static str,
    mapping: Vec<(&'static str, i64)>,
}

enum Last {
    Validation,
    Callback,
}

/// A model's class-level declarations, in source order, because order is
/// behavior: validations and callbacks run in the order they were declared.
pub struct Behavior<M> {
    enums: Vec<EnumDef>,
    pub(crate) validations: Vec<Guarded<Validation<M>, M>>,
    pub(crate) callbacks: Vec<Guarded<(Event, Hook<M>), M>>,
    last: Option<Last>,
}

impl<M> Default for Behavior<M> {
    fn default() -> Self {
        Self { enums: Vec::new(), validations: Vec::new(), callbacks: Vec::new(), last: None }
    }
}

impl<M> Behavior<M> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `belongs_to :user`: adds the "must exist" check Rails adds.
    pub fn belongs_to<T: Record>(self, name: &'static str, foreign_key: &'static str) -> Self {
        self.validates(name, Check::Required { foreign_key, table: T::TABLE })
    }

    /// `enum :status, { draft: 0 }`; `validate` is Rails' `validate: true`.
    pub fn enumeration(mut self, attribute: &'static str, mapping: &[(&'static str, i64)], validate: bool) -> Self {
        self.enums.push(EnumDef { attribute, mapping: mapping.to_vec() });
        if !validate {
            return self;
        }
        let labels = mapping.iter().map(|(label, _)| Value::from(*label)).collect();
        self.validates(attribute, Check::Inclusion(labels))
    }

    pub fn validates(mut self, attribute: &'static str, check: Check) -> Self {
        self.validations.push(Guarded::new(Validation::Check { attribute, check }));
        self.last = Some(Last::Validation);
        self
    }

    /// `validate :method`
    pub fn validate(mut self, hook: Hook<M>) -> Self {
        self.validations.push(Guarded::new(Validation::Custom(hook)));
        self.last = Some(Last::Validation);
        self
    }

    pub fn callback(mut self, event: Event, hook: Hook<M>) -> Self {
        self.callbacks.push(Guarded::new((event, hook)));
        self.last = Some(Last::Callback);
        self
    }

    pub fn before_validation(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeValidation, hook) }
    pub fn after_validation(self, hook: Hook<M>) -> Self { self.callback(Event::AfterValidation, hook) }
    pub fn before_save(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeSave, hook) }
    pub fn after_save(self, hook: Hook<M>) -> Self { self.callback(Event::AfterSave, hook) }
    pub fn before_create(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeCreate, hook) }
    pub fn after_create(self, hook: Hook<M>) -> Self { self.callback(Event::AfterCreate, hook) }
    pub fn before_update(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeUpdate, hook) }
    pub fn after_update(self, hook: Hook<M>) -> Self { self.callback(Event::AfterUpdate, hook) }
    pub fn before_destroy(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeDestroy, hook) }
    pub fn after_destroy(self, hook: Hook<M>) -> Self { self.callback(Event::AfterDestroy, hook) }

    /// `if:` on the validation or callback declared just before.
    pub fn when(mut self, cond: Cond<M>) -> Self {
        self.guard_last().when.push(cond);
        self
    }

    /// `unless:` on the validation or callback declared just before.
    pub fn unless(mut self, cond: Cond<M>) -> Self {
        self.guard_last().unless.push(cond);
        self
    }

    fn guard_last(&mut self) -> GuardRef<'_, M> {
        match self.last {
            Some(Last::Validation) => GuardRef::from(self.validations.last_mut().expect("declared")),
            Some(Last::Callback) => GuardRef::from(self.callbacks.last_mut().expect("declared")),
            None => panic!("`when`/`unless` need a validation or callback before them"),
        }
    }

    /// Enum labels become their integers; an unknown label becomes nil, as
    /// Rails' enum type serializes it.
    pub(crate) fn to_database(&self, attribute: &str, value: Value) -> Value {
        match (self.enum_for(attribute), &value) {
            (Some(def), Value::Str(label)) => {
                def.mapping.iter().find(|(l, _)| l == label).map_or(Value::Nil, |(_, i)| Value::Int(*i))
            }
            _ => value,
        }
    }

    /// Enum integers become their labels; an unknown integer becomes nil.
    pub(crate) fn from_database(&self, attribute: &str, value: Value) -> Value {
        match (self.enum_for(attribute), &value) {
            (Some(def), Value::Int(i)) => def.mapping.iter().find(|(_, n)| n == i).map_or(Value::Nil, |(l, _)| Value::from(*l)),
            _ => value,
        }
    }

    fn enum_for(&self, attribute: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.attribute == attribute)
    }
}

/// The condition lists of whichever entry was declared last.
struct GuardRef<'a, M> {
    when: &'a mut Vec<Cond<M>>,
    unless: &'a mut Vec<Cond<M>>,
}

impl<'a, T, M> From<&'a mut Guarded<T, M>> for GuardRef<'a, M> {
    fn from(guarded: &'a mut Guarded<T, M>) -> Self {
        Self { when: &mut guarded.when, unless: &mut guarded.unless }
    }
}
