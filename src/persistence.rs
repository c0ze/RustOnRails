use crate::{Ctx, Error, Event, Handle, Model, Result, validation};

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
