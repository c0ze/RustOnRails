//! `has_secure_token`: a random token for each new record.

use crate::{Behavior, Blank, Ctx, Handle, Model, Result, Value};

/// ActiveSupport's `SecureRandom::BASE58_ALPHABET`: digits and letters
/// without 0, O, I and l.
const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// `SecureRandom.base58(length)`: each character drawn uniformly from the
/// alphabet, from the operating system's random source.
pub fn base58(length: usize) -> String {
    let mut token = String::with_capacity(length);
    let mut bytes = [0u8; 64];
    while token.len() < length {
        getrandom::fill(&mut bytes).expect("the operating system's random source failed");
        // 58 of every 64 values map evenly onto the alphabet; the rest are redrawn.
        for byte in bytes.iter().map(|b| b % 64).filter(|b| *b < 58).take(length - token.len()) {
            token.push(BASE58[byte as usize] as char);
        }
    }
    token
}

impl<M> Behavior<M> {
    /// `has_secure_token :api_token`, generated `on: :initialize` (Rails
    /// 7.1's default): a record gets its token when it's built, unless it
    /// was given one.
    pub fn has_secure_token(mut self, attribute: &'static str, length: usize) -> Self {
        self.tokens.push((attribute, length));
        self
    }
}

/// The token callback's body: only when the attribute is blank, as
/// `query_attribute` reads a String. Rails assigns through the attribute
/// writer, so a normalizer on the attribute applies.
pub(crate) fn fill<M: Model>(record: &mut M, attribute: &str, length: usize) -> Result<()> {
    let blank = match record.get(attribute) {
        Value::Str(s) => s.is_blank(),
        other => other.is_nil(),
    };
    if !blank {
        return Ok(());
    }
    record.set(attribute, M::behavior().normalize(attribute, Value::Str(base58(length))))
}

impl Ctx {
    /// Rails runs after_initialize once `new` has assigned every attribute.
    /// `build` fills the tokens of a record built with its attributes;
    /// generated code that writes attributes onto a built record one by one
    /// (`User.create!(api_token: nil, ...)`) calls this after the writes, so
    /// a token they left blank is filled.
    pub fn fill_secure_tokens<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        if !self.is_new_record(record) {
            return Ok(());
        }
        for (attribute, length) in &M::behavior().tokens {
            fill(&mut self[record], attribute, *length)?;
        }
        Ok(())
    }

    /// `has_secure_token ..., on: :create`: a before_create hook, which
    /// generated code declares as
    /// `.before_create(|ctx, user| ctx.fill_secure_token(user, "api_token", 24))`.
    pub fn fill_secure_token<M: Model>(&mut self, record: Handle<M>, attribute: &str, length: usize) -> Result<()> {
        if !self.is_new_record(record) {
            return Ok(());
        }
        fill(&mut self[record], attribute, length)
    }
}
