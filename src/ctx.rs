use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};

use postgres::types::ToSql;
use postgres::{Client, NoTls, Row, Statement};

use crate::{Result, Value};

/// Rails' `statement_limit`: the prepared statements a connection keeps.
const STATEMENT_LIMIT: usize = 1000;

/// A database connection and the statements prepared on it. It outlives
/// any one `Ctx`: the server keeps one per worker, so Postgres parses and
/// plans each query once per connection, as with Rails' statement cache,
/// rather than on every call.
pub struct Connection {
    client: Client,
    statements: HashMap<String, Statement>,
    /// Oldest first; past the limit the oldest is dropped, as Rails does.
    order: VecDeque<String>,
}

impl Connection {
    pub fn new(client: Client) -> Self {
        Self { client, statements: HashMap::new(), order: VecDeque::new() }
    }

    pub fn connect(url: &str) -> Result<Self> {
        Ok(Self::new(Client::connect(url, NoTls)?))
    }

    /// Whether the database closed it (restart, failover, idle kill).
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    fn prepared(&mut self, sql: &str) -> Result<Statement> {
        if let Some(statement) = self.statements.get(sql) {
            return Ok(statement.clone());
        }
        let statement = self.client.prepare(sql)?;
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
    /// Whether the innermost open transaction takes nested ones into it.
    /// The test's own transaction doesn't, like Rails' fixture transaction,
    /// so a transaction a test runs gets a savepoint as it would in Rails.
    pub(crate) joinable: bool,
    /// For each transaction open (not joined), the records it touched and
    /// their state before, to put back if it rolls back.
    pub(crate) frames: Vec<Vec<crate::transaction::Remembered>>,
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
        Self { connection, depth: 0, joinable: false, frames: Vec::new(), tables: HashMap::new() }
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
        Ok(self.connection.client.query(&statement, &refs(params))?)
    }

    pub fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        let statement = self.connection.prepared(sql)?;
        Ok(self.connection.client.execute(&statement, &refs(params))?)
    }

    /// Runs `sql` without keeping its statement: SQL with values written
    /// into it (a `where` fragment's binds), which Rails doesn't prepare
    /// either, since each value would be a statement of its own.
    pub(crate) fn batch_execute(&mut self, sql: &str) -> Result<()> {
        Ok(self.connection.client.batch_execute(sql)?)
    }

    pub(crate) fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        Ok(self.connection.client.query(sql, &refs(params))?)
    }
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
}
