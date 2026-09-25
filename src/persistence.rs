use crate::{BeforeTypeCast, Ctx, Error, Event, Handle, Model, Record, Result, Value, now, validation, write};

impl Ctx {
    /// `valid?`: runs the validation callbacks and the validations,
    /// replacing `errors`. A before_validation abort makes it false.
    pub fn is_valid<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        self.errors_mut(record).clear();
        match self.run_callbacks(record, Event::BeforeValidation) {
            Err(Error::Abort) => return Ok(false),
            other => other?,
        }
        validation::run(self, record)?;
        self.run_callbacks(record, Event::AfterValidation)?;
        Ok(self.errors(record).is_empty())
    }

    /// `save`: false when validations fail or a before callback aborts.
    /// Everything runs in a transaction (a savepoint when nested); on
    /// failure the record keeps its unsaved state and id.
    pub fn save<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        let before = (self.slot(record).saved.clone(), self[record].get("id"), self[record].before_type_cast().clone());
        let outcome = self.transaction(|ctx| ctx.create_or_update(record));
        match outcome {
            Ok(true) => Ok(true),
            Ok(false) | Err(Error::Abort) => {
                self.restore(record, before)?;
                Ok(false)
            }
            Err(error) => {
                self.restore(record, before)?;
                Err(error)
            }
        }
    }

    /// `save!`
    pub fn save_bang<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        if self.save(record)? {
            return Ok(());
        }
        let messages = self.errors(record).full_messages();
        if messages.is_empty() {
            Err(Error::RecordNotSaved { model: M::NAME })
        } else {
            Err(Error::RecordInvalid { model: M::NAME, messages })
        }
    }

    /// `update(attributes)`: assigns in the closure, then saves.
    pub fn update<M: Model>(&mut self, record: Handle<M>, assign: impl FnOnce(&mut M)) -> Result<bool> {
        assign(&mut self[record]);
        self.save(record)
    }

    /// `update!(attributes)`
    pub fn update_bang<M: Model>(&mut self, record: Handle<M>, assign: impl FnOnce(&mut M)) -> Result<()> {
        assign(&mut self[record]);
        self.save_bang(record)
    }

    fn create_or_update<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        if !self.is_valid(record)? {
            return Ok(false);
        }
        self.run_callbacks(record, Event::BeforeSave)?;
        if self.is_new_record(record) {
            self.run_callbacks(record, Event::BeforeCreate)?;
            self.insert(record)?;
            self.run_callbacks(record, Event::AfterCreate)?;
        } else {
            self.run_callbacks(record, Event::BeforeUpdate)?;
            self.write_changes(record)?;
            self.run_callbacks(record, Event::AfterUpdate)?;
        }
        self.run_callbacks(record, Event::AfterSave)?;
        Ok(true)
    }

    fn insert<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        write::fill_timestamps(&mut self[record], now())?;
        let snapshot = self[record].clone();
        let id = write::insert_row(self, &snapshot)?;
        self[record].set("id", id)?;
        // Rails' changes_applied: attributes read as the database has them.
        self[record].before_type_cast_mut().forget();
        let saved = self[record].clone();
        self.slot_mut(record).saved = Some(saved);
        Ok(())
    }

    /// Writes changed columns only; with no changes there's no UPDATE and
    /// `updated_at` stays, as in Rails.
    fn write_changes<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        let mut columns = self.changed(record);
        if columns.is_empty() {
            self[record].before_type_cast_mut().forget();
            return Ok(());
        }
        if M::COLUMNS.contains(&"updated_at") && !columns.contains(&"updated_at") {
            self[record].set("updated_at", Value::Time(now()))?;
            columns.push("updated_at");
        }
        let id = self.saved_id(record)?;
        let snapshot = self[record].clone();
        write::update_row(self, id, &snapshot, &columns)?;
        self[record].before_type_cast_mut().forget();
        self.slot_mut(record).saved = Some(snapshot);
        Ok(())
    }

    /// `destroy`: runs the destroy callbacks and deletes the row. False
    /// when a before_destroy callback aborts.
    pub fn destroy<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        let outcome = self.transaction(|ctx| {
            ctx.run_callbacks(record, Event::BeforeDestroy)?;
            if let Ok(id) = ctx.saved_id(record) {
                write::delete_row::<M>(ctx, id)?;
            }
            ctx.slot_mut(record).destroyed = true;
            ctx.run_callbacks(record, Event::AfterDestroy)?;
            Ok(true)
        });
        match outcome {
            Ok(done) => Ok(done),
            Err(error) => {
                self.slot_mut(record).destroyed = false;
                if matches!(error, Error::Abort) { Ok(false) } else { Err(error) }
            }
        }
    }

    /// `destroy!`
    pub fn destroy_bang<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        if self.destroy(record)? { Ok(()) } else { Err(Error::RecordNotDestroyed { model: M::NAME }) }
    }

    /// `reload`: reads the row again, dropping unsaved changes and errors.
    pub fn reload<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        let id = self.saved_id(record)?;
        let found = M::all().where_eq("id", id).limit(1).fetch(self)?.into_iter().next();
        let fresh = found.ok_or_else(|| Error::RecordNotFound { model: M::NAME, conditions: Some(format!("'id'={id}")) })?;
        let slot = self.slot_mut(record);
        slot.saved = Some(fresh.clone());
        slot.record = fresh;
        slot.errors.clear();
        slot.associations.clear();
        Ok(())
    }

    /// `increment!(column, by)`: bumps the value in memory and in the row
    /// with one UPDATE; no validations, no callbacks, no `updated_at`.
    pub fn increment_bang<M: Model>(&mut self, record: Handle<M>, column: &'static str, by: i64) -> Result<()> {
        let id = self.saved_id(record)?;
        let current = match self[record].get(column) {
            Value::Int(i) => i,
            _ => 0,
        };
        self[record].set(column, Value::Int(current + by))?;
        write::increment_column::<M>(self, id, column, by)?;
        let value = self[record].get(column);
        if let Some(saved) = self.slot_mut(record).saved.as_mut() {
            saved.set(column, value)?;
        }
        Ok(())
    }

    pub(crate) fn saved_id<M: Model>(&self, record: Handle<M>) -> Result<i64> {
        self.slot(record).saved.as_ref().and_then(Record::id).ok_or(Error::NotPersisted { model: M::NAME })
    }

    /// Puts back what a rolled-back save changed: the saved state, the id
    /// and the values as given.
    fn restore<M: Model>(&mut self, record: Handle<M>, (saved, id, given): (Option<M>, Value, BeforeTypeCast)) -> Result<()> {
        self.slot_mut(record).saved = saved;
        *self[record].before_type_cast_mut() = given;
        self[record].set("id", id)
    }

    /// Runs one event's callbacks in declaration order.
    pub(crate) fn run_callbacks<M: Model>(&mut self, record: Handle<M>, event: Event) -> Result<()> {
        for entry in &M::behavior().callbacks {
            let (on, hook) = entry.item;
            if on == event && entry.applies(self, record) {
                hook(self, record)?;
            }
        }
        Ok(())
    }
}
