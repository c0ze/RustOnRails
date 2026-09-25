mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Ctx, Model, Record, Time, Value, base58, model};

model! {
    pub struct Account in "accounts" { id: i64, name: String, api_token: String, invite: String, created_at: Time, updated_at: Time }
}

impl Account {
    // normalizes :api_token, with: ->(token) { "tok_#{token}" }
    fn normalize_api_token(token: String) -> String {
        format!("tok_{token}")
    }
}

// `has_secure_token :api_token` (on: :initialize) and
// `has_secure_token :invite, length: 30, on: :create`.
impl Model for Account {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Account>> = LazyLock::new(|| {
            Behavior::<Account>::new()
                .normalizes("api_token", Account::normalize_api_token)
                .has_secure_token("api_token", 24)
                .before_create(|ctx, account| ctx.fill_secure_token(account, "invite", 30))
        });
        &BEHAVIOR
    }
}

/// The blog has no token column; this table lives for one test.
fn ctx() -> Ctx {
    let mut ctx = support::ctx();
    let sql = "CREATE TEMP TABLE accounts (id bigserial PRIMARY KEY, name text, api_token text, invite text, \
               created_at timestamp(6), updated_at timestamp(6))";
    ctx.execute(sql, &[]).unwrap();
    ctx
}

const ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

#[test]
fn test_base58_draws_from_rails_alphabet() {
    let tokens: Vec<String> = (0..200).map(|_| base58(24)).collect();
    for token in &tokens {
        assert_eq!(24, token.len());
        assert!(token.chars().all(|c| ALPHABET.contains(c)), "{token}");
    }
    let mut unique = tokens.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(tokens.len(), unique.len());
    // Every letter of the alphabet turns up, and nothing outside it.
    let seen: String = tokens.concat();
    assert!(ALPHABET.chars().all(|c| seen.contains(c)));
    assert_eq!(58, base58(58).len());
    assert_eq!("", base58(0));
}

/// on: :initialize: `Account.new` already has its token, a given token is
/// kept, and a blank one is replaced, as `query_attribute` reads it.
#[test]
fn test_a_token_is_generated_when_the_record_is_built() {
    let mut ctx = ctx();
    let fresh = ctx.build(Account::from_attributes(&[("name".into(), Value::from("ann"))]).unwrap());
    // Through the attribute writer, so the normalizer applies.
    let token = ctx[fresh].api_token.clone().unwrap();
    assert!(token.starts_with("tok_") && token.len() == 28, "{token}");
    let given = ctx.build(Account::from_attributes(&[("api_token".into(), Value::from("mine"))]).unwrap());
    assert_eq!(Some("tok_mine"), ctx[given].api_token.as_deref());
    // Normalized before the token check, so no longer blank, as in Rails.
    let blank = ctx.build(Account::from_attributes(&[("api_token".into(), Value::from(" \t"))]).unwrap());
    assert_eq!(Some("tok_ \t"), ctx[blank].api_token.as_deref());

    // Saving keeps it; a loaded record is never given a new one.
    ctx.save_bang(fresh).unwrap();
    let id = ctx[fresh].id.unwrap();
    let loaded = Account::find(&mut ctx, id).unwrap();
    assert_eq!(Some(token), ctx[loaded].api_token.clone());
}

/// `Account.create!(api_token: nil)` writes onto a built record; the token
/// it blanks is filled again, as Rails' after_initialize runs after it.
#[test]
fn test_tokens_are_filled_again_after_attribute_writes() {
    let mut ctx = ctx();
    let account = ctx.build(Account::new_record());
    let first = ctx[account].api_token.clone();
    ctx[account].api_token = None;
    ctx.fill_secure_tokens(account).unwrap();
    assert!(ctx[account].api_token.as_ref().is_some_and(|t| t.len() == 28));
    assert_ne!(first, ctx[account].api_token);
    ctx[account].api_token = Some(" \t".into());
    ctx.fill_secure_tokens(account).unwrap();
    assert_eq!(28, ctx[account].api_token.as_ref().unwrap().len(), "blank, as query_attribute reads it");
    ctx[account].api_token = Some("kept".into());
    ctx.fill_secure_tokens(account).unwrap();
    assert_eq!(Some("kept"), ctx[account].api_token.as_deref());
    ctx.save_bang(account).unwrap();
    ctx[account].api_token = None;
    ctx.fill_secure_tokens(account).unwrap();
    assert_eq!(None, ctx[account].api_token, "a saved record isn't initialized again");
}

/// on: :create: nothing until the INSERT, then only when blank.
#[test]
fn test_a_token_on_create_waits_for_the_insert() {
    let mut ctx = ctx();
    let account = ctx.build(Account::new_record());
    assert_eq!(None, ctx[account].invite);
    ctx.save_bang(account).unwrap();
    assert_eq!(30, ctx[account].invite.as_ref().unwrap().len());
    let invite = ctx[account].invite.clone();
    ctx[account].invite = None;
    ctx.save_bang(account).unwrap();
    assert_eq!(None, ctx[account].invite, "an update isn't a create");
    let kept = ctx.build(Account { invite: Some("given".into()), ..Account::new_record() });
    ctx.save_bang(kept).unwrap();
    assert_eq!(Some("given"), ctx[kept].invite.as_deref());
    assert_ne!(invite, ctx[kept].invite);
}
