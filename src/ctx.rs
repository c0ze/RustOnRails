use std::any::{Any, TypeId};
use std::collections::HashMap;

use postgres::types::ToSql;
use postgres::{Client, NoTls, Row};

use crate::{Result, Value};

/// One unit of work: a database connection plus every record it loaded or
/// built. Rails' equivalent is the objects a request creates; they all go
/// away when the `Ctx` is dropped. There's no identity map: like Rails,
/// every load makes a new record.
pub struct Ctx {
    pub(crate) client: Client,
    pub(crate) depth: u32,
    pub(crate) tables: HashMap<TypeId, Box<dyn Any + Send>>,
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

    /// Gives the connection back, dropping every record this `Ctx` held;
    /// the server reuses one connection per worker across requests.
    pub fn into_client(self) -> Client {
        self.client
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

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
}
