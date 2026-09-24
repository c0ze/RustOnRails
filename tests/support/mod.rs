#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Once;

use postgres::{Client, NoTls};
use rustonrails::Ctx;

const SCHEMA: &str = include_str!("schema.sql");

pub fn url() -> String {
    std::env::var("RUSTONRAILS_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:54329/rustonrails_test".into())
}

/// A `Ctx` inside a transaction that is never committed: dropping it rolls
/// everything back, so tests can run in parallel on one database.
pub fn ctx() -> Ctx {
    static SETUP: Once = Once::new();
    SETUP.call_once(prepare_database);
    let client = Client::connect(&url(), NoTls).expect("connect to the test database");
    Ctx::rolled_back(client).expect("begin the test transaction")
}

/// Creates and loads the test database without opening a `Ctx`, for code
/// that connects on its own, like the server.
pub fn prepare() {
    drop(ctx());
}

/// Creates the database on first use and reloads schema.sql whenever it
/// changes. An advisory lock keeps concurrent test runs from racing.
fn prepare_database() {
    let url = url();
    let (base, name) = url.rsplit_once('/').expect("database URL ends in /name");
    let mut admin = Client::connect(&format!("{base}/postgres"), NoTls).expect("connect to postgres");
    admin.batch_execute("SELECT pg_advisory_lock(7351)").unwrap();
    if admin.query("SELECT 1 FROM pg_database WHERE datname = $1", &[&name]).unwrap().is_empty() {
        admin.batch_execute(&format!("CREATE DATABASE \"{name}\"")).unwrap();
    }
    let mut db = Client::connect(&url, NoTls).unwrap();
    let mut hasher = DefaultHasher::new();
    SCHEMA.hash(&mut hasher);
    let digest = hasher.finish().to_string();
    let loaded: Option<String> =
        db.query_opt("SELECT digest FROM public.rustonrails_schema", &[]).ok().flatten().map(|row| row.get(0));
    if loaded.as_deref() != Some(digest.as_str()) {
        db.batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;").unwrap();
        db.batch_execute(SCHEMA).unwrap();
        db.batch_execute("CREATE TABLE public.rustonrails_schema (digest text NOT NULL)").unwrap();
        db.execute("INSERT INTO public.rustonrails_schema VALUES ($1)", &[&digest]).unwrap();
    }
    admin.batch_execute("SELECT pg_advisory_unlock(7351)").unwrap();
}
