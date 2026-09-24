#![allow(dead_code)]

use rustonrails::Ctx;

const SCHEMA: &str = include_str!("../../db/schema.sql");

pub fn ctx() -> Ctx {
    rustonrails::testing::ctx(SCHEMA)
}
