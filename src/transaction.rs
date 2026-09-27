//! Transactions as Active Record runs them: a transaction opened inside
//! another joins it, so only the outermost one commits or rolls back, and
//! `ActiveRecord::Rollback` rolls back the transaction that opened without
//! reaching the code around it.

use std::any::TypeId;

use crate::{Ctx, Error, Handle, Model, Request, Result};

/// What opening a transaction did: joined the one already open, or began
/// one (a savepoint inside a transaction that can't be joined).
pub(crate) struct Opened {
    joined: bool,
    joinable: bool,
}

/// How `save`'s or `destroy`'s transaction ended: `RolledBack` is an
/// `ActiveRecord::Rollback` a callback raised, which Rails swallows.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Committed,
    Failed,
    RolledBack,
}

/// A record's state when a transaction first touched it, put back if the
/// transaction rolls back, as Rails' `restore_transaction_record_state`:
/// its id, whether it was saved and destroyed. Its attributes stay as
/// they are, so they count as changes again.
pub(crate) struct Remembered {
    key: (TypeId, u32),
    restore: Box<dyn FnOnce(&mut Ctx) + Send>,
}

impl Ctx {
    /// `save`'s and `destroy`'s transaction: `Ok(false)` or an error rolls
    /// it back. Joined to an open one, `Ok(false)` rolls nothing back: Rails
    /// raises `ActiveRecord::Rollback` there, which the join swallows.
    pub fn transaction(&mut self, block: impl FnOnce(&mut Ctx) -> Result<bool>) -> Result<bool> {
        Ok(self.transaction_outcome(block)? == Outcome::Committed)
    }

    pub(crate) fn transaction_outcome(&mut self, block: impl FnOnce(&mut Ctx) -> Result<bool>) -> Result<Outcome> {
        let opened = self.open()?;
        match block(self) {
            Ok(true) => self.close(opened, true, false).map(|()| Outcome::Committed),
            Ok(false) => self.close(opened, false, false).map(|()| Outcome::Failed),
            Err(Error::Rollback) => self.close(opened, false, false).map(|()| Outcome::RolledBack),
            Err(error) => {
                self.close(opened, false, true)?;
                Err(error)
            }
        }
    }

    /// Notes `record`'s state for the transaction open now, the first time
    /// it touches it.
    pub(crate) fn remember<M: Model>(&mut self, record: Handle<M>) {
        let key = (TypeId::of::<M>(), record.index());
        if self.frames.last().is_none_or(|frame| frame.iter().any(|remembered| remembered.key == key)) {
            return;
        }
        let slot = self.slot(record);
        let (saved, destroyed, id) = (slot.saved.clone(), slot.destroyed, slot.record.get("id"));
        let restore = Box::new(move |ctx: &mut Ctx| {
            let slot = ctx.slot_mut(record);
            slot.saved = saved;
            slot.destroyed = destroyed;
            slot.record.set("id", id).expect("an id read from the record fits it");
        });
        self.frames.last_mut().expect("checked above").push(Remembered { key, restore });
    }

    /// `ActiveRecord::Base.transaction do ... end` in app code: the block's
    /// value, or None when it raised `ActiveRecord::Rollback`. Any other
    /// error rolls back and goes on up, as the exception would.
    pub fn transaction_block<T>(&mut self, block: impl FnOnce(&mut Ctx) -> Result<T>) -> Result<Option<T>> {
        let opened = self.open()?;
        let outcome = block(self);
        self.settle(opened, outcome)
    }

    pub(crate) fn open(&mut self) -> Result<Opened> {
        if self.depth > 0 && self.joinable {
            return Ok(Opened { joined: true, joinable: true });
        }
        let begin = if self.depth == 0 { "BEGIN".to_string() } else { format!("SAVEPOINT {}", self.savepoint()) };
        self.batch_execute(&begin)?;
        self.depth += 1;
        self.frames.push(Vec::new());
        let opened = Opened { joined: false, joinable: self.joinable };
        self.joinable = true;
        Ok(opened)
    }

    pub(crate) fn settle<T>(&mut self, opened: Opened, outcome: Result<T>) -> Result<Option<T>> {
        let commit = outcome.is_ok();
        let failed = matches!(outcome, Err(ref error) if !matches!(error, Error::Rollback));
        self.close(opened, commit, failed)?;
        match outcome {
            Ok(value) => Ok(Some(value)),
            Err(Error::Rollback) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Commits or rolls back what `open` began; a joined transaction is
    /// left to its owner. After an error the rollback's own failure is
    /// dropped: the original error says more.
    fn close(&mut self, opened: Opened, commit: bool, failed: bool) -> Result<()> {
        if opened.joined {
            return Ok(());
        }
        self.depth -= 1;
        self.joinable = opened.joinable;
        let frame = self.frames.pop().unwrap_or_default();
        let sql = match (self.depth == 0, commit) {
            (true, true) => "COMMIT".to_string(),
            (true, false) => "ROLLBACK".to_string(),
            (false, true) => format!("RELEASE SAVEPOINT {}", self.savepoint()),
            (false, false) => format!("ROLLBACK TO SAVEPOINT {}", self.savepoint()),
        };
        if !commit {
            self.restore_frame(frame);
            let closed = self.batch_execute(&sql);
            return if failed { Ok(()) } else { closed };
        }
        match self.batch_execute(&sql) {
            Ok(()) => {
                // A committed savepoint's records roll back with the transaction around it.
                if let Some(parent) = self.frames.last_mut() {
                    let keep: Vec<Remembered> = frame.into_iter().filter(|r| parent.iter().all(|p| p.key != r.key)).collect();
                    parent.extend(keep);
                }
                Ok(())
            }
            // A COMMIT can fail on its own, on a deferred constraint: the
            // database has rolled back, so the records go back too.
            Err(error) => {
                self.restore_frame(frame);
                Err(error)
            }
        }
    }

    fn restore_frame(&mut self, frame: Vec<Remembered>) {
        for remembered in frame.into_iter().rev() {
            (remembered.restore)(self);
        }
    }

    fn savepoint(&self) -> String {
        format!("active_record_{}", self.depth)
    }
}

impl Request {
    /// `transaction do ... end` in a controller: the block gets the whole
    /// request, since it may read params as well as records.
    pub fn transaction_block<T>(&mut self, block: impl FnOnce(&mut Request) -> Result<T>) -> Result<Option<T>> {
        let opened = self.ctx.open()?;
        let outcome = block(self);
        self.ctx.settle(opened, outcome)
    }
}
