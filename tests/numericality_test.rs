mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Check, Ctx, Handle, Model, Number, Numericality, Time, Value, model};

// Each set of options is its own model over the posts table, checked
// against what Rails 8.1 says for the same options and values.
macro_rules! counted {
    ($name:ident, $options:expr) => {
        model! {
            pub struct $name in "posts" { id: i64, user_id: i64, title: String, comments_count: i64 }
        }

        impl Model for $name {
            fn behavior() -> &'static Behavior<Self> {
                static BEHAVIOR: LazyLock<Behavior<$name>> =
                    LazyLock::new(|| Behavior::<$name>::new().validates("comments_count", Check::Numericality($options)));
                &BEHAVIOR
            }
        }
    };
}

counted!(Positive, Numericality { only_integer: true, greater_than: Some(Number::Int(0)), ..Numericality::default() });
counted!(
    Ranged,
    Numericality { greater_than: Some(Number::Float(0.5)), less_than_or_equal_to: Some(Number::Int(10)), ..Numericality::default() }
);
counted!(
    Contrary,
    Numericality {
        greater_than: Some(Number::Int(10)),
        greater_than_or_equal_to: Some(Number::Int(11)),
        equal_to: Some(Number::Int(3)),
        less_than: Some(Number::Int(5)),
        other_than: Some(Number::Int(1)),
        ..Numericality::default()
    }
);
counted!(
    Floats,
    Numericality {
        greater_than: Some(Number::Float(1.0)),
        equal_to: Some(Number::Float(0.1)),
        less_than: Some(Number::Float(1e20)),
        ..Numericality::default()
    }
);
counted!(Hundred, Numericality { less_than: Some(Number::Int(100)), ..Numericality::default() });
counted!(Tie, Numericality { less_than: Some(Number::Int(1234567890123445)), ..Numericality::default() });

/// The messages for `value` assigned as params would assign it.
fn messages<M: Model>(value: impl Into<Value>) -> Vec<String> {
    let mut ctx = support::ctx();
    let record = M::from_attributes(&[("comments_count".into(), value.into())]).unwrap();
    let record = ctx.build(record);
    ctx.is_valid(record).unwrap();
    ctx.errors(record).on("comments_count").into_iter().map(String::from).collect()
}

const NOT_A_NUMBER: &str = "is not a number";
const NOT_AN_INTEGER: &str = "must be an integer";

#[test]
fn test_only_integer_and_greater_than() {
    let cases: Vec<(Value, &[&str])> = vec![
        (0.into(), &["must be greater than 0"]),
        ((-1).into(), &["must be greater than 0"]),
        (1.into(), &[]),
        (1.5.into(), &[NOT_AN_INTEGER]),
        (2.0.into(), &[NOT_AN_INTEGER]),
        (1e20.into(), &[NOT_AN_INTEGER]),
        ("abc".into(), &[NOT_A_NUMBER]),
        ("".into(), &[NOT_A_NUMBER]),
        (Value::Nil, &[NOT_A_NUMBER]),
        (true.into(), &[NOT_A_NUMBER]),
        (false.into(), &["must be greater than 0"]),
        ("12abc".into(), &[NOT_A_NUMBER]),
        ("0x1A".into(), &[NOT_A_NUMBER]),
        (" 0x1A".into(), &[NOT_AN_INTEGER]),
        ("5".into(), &[]),
        ("+5".into(), &[]),
        ("-0".into(), &["must be greater than 0"]),
        ("99999999999999999999".into(), &[]),
        (" 5".into(), &[NOT_AN_INTEGER]),
        (" 5 ".into(), &[NOT_AN_INTEGER]),
        ("5.0".into(), &[NOT_AN_INTEGER]),
        ("1e3".into(), &[NOT_AN_INTEGER]),
        ("1_000".into(), &[NOT_AN_INTEGER]),
        ("1.5e1".into(), &[NOT_AN_INTEGER]),
    ];
    for (value, expected) in cases {
        assert_eq!(expected, messages::<Positive>(value.clone()), "{value:?}");
    }
}

#[test]
fn test_float_bounds() {
    let cases: Vec<(Value, &[&str])> = vec![
        (0.into(), &["must be greater than 0.5"]),
        (1.into(), &[]),
        (1.5.into(), &[]),
        ("5.0".into(), &[]),
        (" 5 ".into(), &[]),
        ("-0".into(), &["must be greater than 0.5"]),
        (false.into(), &["must be greater than 0.5"]),
        ("1e3".into(), &["must be less than or equal to 10"]),
        ("1_000".into(), &["must be less than or equal to 10"]),
        (" 0x1A".into(), &["must be less than or equal to 10"]),
        ("99999999999999999999".into(), &["must be less than or equal to 10"]),
        (1e20.into(), &["must be less than or equal to 10"]),
        ("abc".into(), &[NOT_A_NUMBER]),
        ("0x1A".into(), &[NOT_A_NUMBER]),
    ];
    for (value, expected) in cases {
        assert_eq!(expected, messages::<Ranged>(value.clone()), "{value:?}");
    }
}

/// Every failing comparison adds its message, in Rails' order.
#[test]
fn test_every_comparison_in_order() {
    let high = ["must be greater than 10", "must be greater than or equal to 11", "must be equal to 3"];
    assert_eq!([&high[..], &["must be less than 5"]].concat(), messages::<Contrary>(7));
    assert_eq!([&high[..], &["must be less than 5"]].concat(), messages::<Contrary>("7.5"));
    assert_eq!([&high[..], &["must be other than 1"]].concat(), messages::<Contrary>(1));
    // A float option is a BigDecimal in the message: 1.0 stays "1.0".
    assert_eq!(vec!["must be greater than 1.0", "must be equal to 0.1"], messages::<Floats>(0));
    assert_eq!(vec!["must be equal to 0.1"], messages::<Floats>(2));
}

/// Floats and decimal strings are rounded to 15 significant digits first.
#[test]
fn test_fifteen_significant_digits() {
    assert_eq!(vec!["must be less than 100"], messages::<Hundred>("99.99999999999999"));
    assert_eq!(vec!["must be less than 100"], messages::<Hundred>(99.99999999999999));
    assert!(messages::<Hundred>("99.9999999999999").is_empty());
    // A tie rounds to even, as BigDecimal's dtoa does: ...44|5 stays ...44.
    assert!(messages::<Tie>(1234567890123445.0).is_empty());
    assert_eq!(vec!["must be less than 1234567890123445"], messages::<Tie>(1234567890123445_i64));
}

/// A value the app sets replaces the one params gave.
#[test]
fn test_a_value_the_app_sets_replaces_the_one_given() {
    let mut ctx = support::ctx();
    let post = ctx.build(Positive::from_attributes(&[("comments_count".into(), Value::from("abc"))]).unwrap());
    ctx[post].comments_count = Some(3);
    assert!(ctx.is_valid(post).unwrap());
}

model! {
    pub struct Strict in "posts" { id: i64, user_id: i64, title: String, comments_count: i64, created_at: Time, updated_at: Time }
}

impl Model for Strict {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Strict>> = LazyLock::new(|| {
            Behavior::<Strict>::new()
                .validates("comments_count", Check::Numericality(Numericality { only_integer: true, ..Numericality::default() }))
                .when(|ctx, post| ctx[post].title.as_deref() == Some("strict"))
        });
        &BEHAVIOR
    }
}

/// After a save, Rails validates the value as the column holds it.
#[test]
fn test_a_save_forgets_the_value_as_given() {
    let mut ctx = support::ctx();
    let user = insert_user(&mut ctx);
    let attributes = [("user_id".into(), Value::Int(user)), ("title".into(), "strict".into()), ("comments_count".into(), "5.0".into())];
    let post: Handle<Strict> = ctx.build(Strict::from_attributes(&attributes).unwrap());
    assert!(!ctx.is_valid(post).unwrap());
    ctx[post].title = Some("loose".into());
    ctx.save_bang(post).unwrap();
    assert_eq!(Some(5), ctx[post].comments_count);
    ctx[post].title = Some("strict".into());
    assert!(ctx.is_valid(post).unwrap());
}

fn insert_user(ctx: &mut Ctx) -> i64 {
    let sql = "INSERT INTO users (name, email, created_at, updated_at) VALUES ('U', 'u@example.com', now(), now()) RETURNING id";
    ctx.query(sql, &[]).unwrap()[0].get(0)
}
