use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};

use postgres::error::{Severity, SqlState};
use postgres::types::ToSql;
use postgres::{Client, Row, Statement};

use crate::{Result, Value};

/// Rails' `statement_limit`: the prepared statements a connection keeps.
const STATEMENT_LIMIT: usize = 1000;

/// A database connection and the statements prepared on it. It outlives
/// any one `Ctx`: the server keeps one per worker, so Postgres parses and
/// plans each query once per connection, as with Rails' statement cache,
/// rather than on every call.
pub struct Connection {
    client: Client,
    /// Set when a call failed in a way that ends the connection.
    broken: bool,
    statements: HashMap<String, Statement>,
    /// Oldest first; past the limit the oldest is dropped, as Rails does.
    order: VecDeque<String>,
}

impl Connection {
    pub fn new(client: Client) -> Self {
        Self { client, broken: false, statements: HashMap::new(), order: VecDeque::new() }
    }

    /// With TLS as the URL's `sslmode` asks, as libpq reads it.
    pub fn connect(url: &str) -> Result<Self> {
        Ok(Self::new(crate::connect::connect(url)?))
    }

    /// Whether the database closed it (restart, failover, idle kill). The
    /// driver only marks itself closed once it reads the end of the socket,
    /// which can be a request after the FATAL error that announced it.
    pub fn is_closed(&self) -> bool {
        self.broken || self.client.is_closed()
    }

    /// Passes a call's result through, noting an error that ends the
    /// connection. "cached plan must not change result type" means a
    /// migration changed a table under the prepared statements: like Rails'
    /// statement cache, they're dropped and the next query prepares afresh.
    fn check<T>(&mut self, result: std::result::Result<T, postgres::Error>) -> Result<T> {
        if let Err(error) = &result {
            if ends_connection(error) {
                self.broken = true;
            } else if error.code() == Some(&SqlState::FEATURE_NOT_SUPPORTED) {
                self.statements.clear();
                self.order.clear();
            }
        }
        Ok(result?)
    }

    fn prepared(&mut self, sql: &str) -> Result<Statement> {
        if let Some(statement) = self.statements.get(sql) {
            return Ok(statement.clone());
        }
        let prepared = self.client.prepare(sql);
        let statement = self.check(prepared)?;
        if self.order.len() == STATEMENT_LIMIT {
            let oldest = self.order.pop_front().expect("the limit isn't zero");
            self.statements.remove(&oldest);
        }
        self.order.push_back(sql.to_string());
        self.statements.insert(sql.to_string(), statement.clone());
        Ok(statement)
    }
}

/// One unit of work: a database connection plus every record it loaded or
/// built. Rails' equivalent is the objects a request creates; they all go
/// away when the `Ctx` is dropped. There's no identity map: like Rails,
/// every load makes a new record.
pub struct Ctx {
    pub(crate) connection: Connection,
    pub(crate) depth: u32,
    pub(crate) tables: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl Ctx {
    pub fn connect(url: &str) -> Result<Self> {
        Ok(Self::resume(Connection::connect(url)?))
    }

    pub fn new(client: Client) -> Self {
        Self::resume(Connection::new(client))
    }

    /// A fresh unit of work on a connection that keeps its prepared statements.
    pub fn resume(connection: Connection) -> Self {
        Self { connection, depth: 0, tables: HashMap::new() }
    }

    /// Opens a transaction that is never committed; dropping the `Ctx`
    /// rolls it back. For tests, like Rails' transactional fixtures.
    pub fn rolled_back(mut client: Client) -> Result<Self> {
        client.batch_execute("BEGIN")?;
        Ok(Self { depth: 1, ..Self::new(client) })
    }

    /// Gives the connection back, with its prepared statements, dropping
    /// every record this `Ctx` held; the server reuses one connection per
    /// worker across requests.
    pub fn into_connection(self) -> Connection {
        self.connection
    }

    pub fn into_client(self) -> Client {
        self.connection.client
    }

    /// Runs `sql` as a statement prepared once on this connection.
    pub fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        let statement = self.connection.prepared(sql)?;
        let rows = self.connection.client.query(&statement, &refs(params));
        self.connection.check(rows)
    }

    pub fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        let statement = self.connection.prepared(sql)?;
        let count = self.connection.client.execute(&statement, &refs(params));
        self.connection.check(count)
    }

    /// Runs `sql` without keeping its statement: SQL with values written
    /// into it (a `where` fragment's binds), which Rails doesn't prepare
    /// either, since each value would be a statement of its own.
    pub(crate) fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        let rows = self.connection.client.query(sql, &refs(params));
        self.connection.check(rows)
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
        self.batch_execute(&begin)?;
        self.depth += 1;
        let outcome = block(self);
        self.depth -= 1;
        match outcome {
            Ok(true) => {
                self.batch_execute(&commit)?;
                Ok(true)
            }
            Ok(false) => {
                self.batch_execute(&rollback)?;
                Ok(false)
            }
            Err(error) => {
                // The original error says more than a failed rollback would.
                self.batch_execute(&rollback).ok();
                Err(error)
            }
        }
    }

    fn batch_execute(&mut self, sql: &str) -> Result<()> {
        let done = self.connection.client.batch_execute(sql);
        self.connection.check(done)
    }
}

/// A FATAL or PANIC from the server (it's terminating this backend), a
/// socket error, or a driver that has already seen the connection close.
fn ends_connection(error: &postgres::Error) -> bool {
    error.is_closed()
        || error.as_db_error().is_some_and(|e| matches!(e.parsed_severity(), Some(Severity::Fatal | Severity::Panic)))
        || std::error::Error::source(error).is_some_and(|e| e.is::<std::io::Error>())
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
}
