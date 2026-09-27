//! Test support, like Rails' `test_help`: one database per run, loaded
//! from a schema file, and a `Ctx` per test inside a transaction that is
//! never committed, so tests can run in parallel.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Once;


use crate::Ctx;

/// `RUSTONRAILS_TEST_DATABASE_URL`, or the Rutile repo's local cluster.
pub fn database_url() -> String {
    std::env::var("RUSTONRAILS_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:54329/rustonrails_test".into())
}

/// Creates the database on first use and reloads `schema` whenever it
/// changes. An advisory lock keeps concurrent test runs from racing.
pub fn prepare(schema: &str) {
    static PREPARED: Once = Once::new();
    PREPARED.call_once(|| load(schema));
}

/// A `Ctx` for one test.
pub fn ctx(schema: &str) -> Ctx {
    prepare(schema);
    let client = crate::connect::connect(&database_url()).expect("connect to the test database");
    Ctx::rolled_back(client).expect("begin the test transaction")
}

fn load(schema: &str) {
    let url = database_url();
    // The admin connection keeps the URL's host, user and TLS settings.
    let name = crate::connect::database_name(&url).expect("parse the test database URL").expect("the URL names a database");
    let mut admin = crate::connect::connect_to(&url, Some("postgres")).expect("connect to postgres");
    admin.batch_execute("SELECT pg_advisory_lock(7351)").unwrap();
    if admin.query("SELECT 1 FROM pg_database WHERE datname = $1", &[&name]).unwrap().is_empty() {
        admin.batch_execute(&format!("CREATE DATABASE \"{name}\"")).unwrap();
    }
    let mut db = crate::connect::connect(&url).unwrap();
    let mut hasher = DefaultHasher::new();
    schema.hash(&mut hasher);
    let digest = hasher.finish().to_string();
    let loaded: Option<String> =
        db.query_opt("SELECT digest FROM public.rustonrails_schema", &[]).ok().flatten().map(|row| row.get(0));
    if loaded.as_deref() != Some(digest.as_str()) {
        // Only a database this loaded, or an empty one, is dropped: a URL
        // pointed at an app's own test database mustn't wipe it.
        let tables: i64 = db.query_one("SELECT count(*) FROM pg_tables WHERE schemaname = 'public'", &[]).unwrap().get(0);
        assert!(loaded.is_some() || tables == 0, "{url} has tables RustOnRails didn't load; refusing to drop them");
        db.batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;").unwrap();
        db.batch_execute(schema).unwrap();
        db.batch_execute("CREATE TABLE public.rustonrails_schema (digest text NOT NULL)").unwrap();
        db.execute("INSERT INTO public.rustonrails_schema VALUES ($1)", &[&digest]).unwrap();
    }
    admin.batch_execute("SELECT pg_advisory_unlock(7351)").unwrap();
}

/// One column of a row as the record layer reads it, for tests of the
/// Postgres types it decodes.
pub fn read(row: &postgres::Row, index: usize) -> crate::Result<crate::Value> {
    crate::pg::read(row, index)
}
