#![allow(dead_code)]

use rustonrails::Ctx;

const SCHEMA: &str = include_str!("../../examples/blog/db/schema.sql");

pub fn ctx() -> Ctx {
    rustonrails::testing::ctx(SCHEMA)
}

pub fn url() -> String {
    rustonrails::testing::database_url()
}

/// Loads the test database without opening a `Ctx`, for code that
/// connects on its own, like the server.
pub fn prepare() {
    rustonrails::testing::prepare(SCHEMA);
}
