//! Transactions as Active Record runs them: a transaction opened inside
//! another joins it, so only the outermost one commits or rolls back, and
//! `ActiveRecord::Rollback` rolls back the transaction that opened without
//! reaching the code around it.

use crate::{Ctx, Error, Request, Result};

/// What opening a transaction did: joined the one already open, or began
/// one (a savepoint inside a transaction that can't be joined).
pub(crate) struct Opened {
    joined: bool,
    joinable: bool,
}

impl Ctx {
    /// `save`'s and `destroy`'s transaction: `Ok(false)` or an error rolls
    /// it back. Joined to an open one, `Ok(false)` rolls nothing back: Rails
    /// raises `ActiveRecord::Rollback` there, which the join swallows.
    pub fn transaction(&mut self, block: impl FnOnce(&mut Ctx) -> Result<bool>) -> Result<bool> {
        let opened = self.open()?;
        let outcome = block(self);
        let commit = matches!(outcome, Ok(true));
        self.close(opened, commit, outcome.is_err())?;
        outcome
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
        let sql = match (self.depth == 0, commit) {
            (true, true) => "COMMIT".to_string(),
            (true, false) => "ROLLBACK".to_string(),
            (false, true) => format!("RELEASE SAVEPOINT {}", self.savepoint()),
            (false, false) => format!("ROLLBACK TO SAVEPOINT {}", self.savepoint()),
        };
        let closed = self.batch_execute(&sql);
        if failed { Ok(()) } else { closed }
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
