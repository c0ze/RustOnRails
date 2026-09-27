mod support;

use rustonrails::{Behavior, Ctx, Error, Model, Record, Time, Value, model, now};

model! {
    pub struct Author in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

model! {
    pub struct Article in "posts" {
        id: i64, user_id: i64, title: String, body: String, status: String = "draft".to_string(), published_at: Time,
        comments_count: i64 = 0, created_at: Time, updated_at: Time,
    }
}

impl Model for Author {
    fn behavior() -> &'static Behavior<Self> {
        static B: std::sync::LazyLock<Behavior<Author>> = std::sync::LazyLock::new(Behavior::new);
        &B
    }
}

impl Model for Article {
    fn behavior() -> &'static Behavior<Self> {
        static B: std::sync::LazyLock<Behavior<Article>> =
            std::sync::LazyLock::new(|| Behavior::new().enumeration("status", &[("draft", 0), ("published", 1)], false));
        &B
    }
}

fn author(ctx: &mut Ctx) -> i64 {
    let record = Author {
        name: Some("ann".into()), email: Some("ann@example.com".into()), created_at: Some(now()), updated_at: Some(now()),
        ..Author::default()
    };
    Author::insert(ctx, record).unwrap()
}

/// Articles titled "A".."E" with 1..5 comments; B and D published.
fn articles(ctx: &mut Ctx) -> i64 {
    let user = author(ctx);
    for (i, title) in ["A", "B", "C", "D", "E"].iter().enumerate() {
        let record = Article {
            user_id: Some(user), title: Some(title.to_string()), body: Some("b".into()),
            status: Some(if i % 2 == 1 { "published" } else { "draft" }.into()), comments_count: Some(i as i64 + 1),
            created_at: Some(now()), updated_at: Some(now()), ..Article::new_record()
        };
        let handle = ctx.build(record);
        ctx.save_bang(handle).unwrap();
    }
    user
}

#[test]
fn test_count_keeps_a_limit_and_an_offset() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    let all = Article::all().order_desc("title");
    assert_eq!(5, all.count(&mut ctx).unwrap());
    assert_eq!(2, all.clone().limit(2).count(&mut ctx).unwrap());
    assert_eq!(2, all.clone().offset(3).count(&mut ctx).unwrap());
    assert_eq!(0, all.clone().limit(0).count(&mut ctx).unwrap());
    assert_eq!(2, all.where_eq("status", "published").count(&mut ctx).unwrap());
}

#[test]
fn test_sum_minimum_and_maximum_ignore_the_order() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    let all = Article::all().order_desc("title");
    assert_eq!(15, all.sum::<i64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(Some(1), all.minimum::<i64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(Some("E".to_string()), all.maximum::<String>(&mut ctx, "title").unwrap());
    assert_eq!(6, all.clone().where_eq("status", "published").sum::<i64>(&mut ctx, "comments_count").unwrap());
}

/// A limit stays on the aggregate's one row, so it sums every row; an
/// offset past that row leaves none: 0 for a sum, nil for the others.
#[test]
fn test_aggregates_keep_rails_limit_and_offset() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    assert_eq!(15, Article::all().limit(2).sum::<i64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(0, Article::all().offset(1).sum::<i64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(None, Article::all().offset(1).maximum::<i64>(&mut ctx, "comments_count").unwrap());
}

#[test]
fn test_aggregates_over_no_rows() {
    let mut ctx = support::ctx();
    let none = Article::all().where_eq("title", "nothing");
    assert_eq!(0, none.sum::<i64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(0.0, none.sum::<f64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(None, none.minimum::<i64>(&mut ctx, "comments_count").unwrap());
    assert_eq!(None, none.maximum::<Time>(&mut ctx, "published_at").unwrap());
}

/// SUM of a bigint column is a numeric, read back as an Integer.
#[test]
fn test_sum_of_a_bigint_column() {
    let mut ctx = support::ctx();
    let user = articles(&mut ctx);
    assert_eq!(user * 5, Article::all().sum::<i64>(&mut ctx, "user_id").unwrap());
}

#[test]
fn test_numeric_values_read_as_their_decimal_text() {
    let mut ctx = support::ctx();
    let cases = [
        ("0::numeric", Value::Int(0)),
        ("12345678901234567::numeric", Value::Int(12345678901234567)),
        ("-10000::numeric", Value::Int(-10000)),
        ("1.50::numeric", Value::Str("1.50".into())),
        ("-0.000012::numeric", Value::Str("-0.000012".into())),
        ("123456.7::numeric(10,3)", Value::Str("123456.700".into())),
        ("'NaN'::numeric", Value::Str("NaN".into())),
        ("NULL::numeric", Value::Nil),
    ];
    for (sql, expected) in cases {
        let rows = ctx.query(&format!("SELECT {sql}"), &[]).unwrap();
        assert_eq!(expected, rustonrails::testing::read(&rows[0], 0).unwrap(), "{sql}");
    }
    let rows = ctx.query("SELECT 99999999999999999999::numeric", &[]).unwrap();
    assert!(matches!(rustonrails::testing::read(&rows[0], 0), Err(Error::Overflow { .. })));
}

#[test]
fn test_pluck_keeps_the_order_and_reads_enum_labels() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    let titles = Article::all().order_desc("title").limit(3).pluck::<String>(&mut ctx, "title").unwrap();
    assert_eq!(vec![Some("E".to_string()), Some("D".into()), Some("C".into())], titles);
    let statuses = Article::all().order_asc("title").limit(2).pluck::<String>(&mut ctx, "status").unwrap();
    assert_eq!(vec![Some("draft".to_string()), Some("published".into())], statuses);
}

#[test]
fn test_exists_drops_the_order_and_keeps_the_offset() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    assert!(Article::all().order_desc("title").exists(&mut ctx).unwrap());
    assert!(Article::all().offset(4).exists(&mut ctx).unwrap());
    assert!(!Article::all().offset(5).exists(&mut ctx).unwrap());
    assert!(!Article::all().limit(0).exists(&mut ctx).unwrap());
    assert!(!Article::all().where_eq("title", "nothing").exists(&mut ctx).unwrap());
}

/// `find_each(batch_size: 2)`: every record once, in id order whatever
/// the relation's order, with the relation's limit capping the total.
#[test]
fn test_batches_walk_the_ids() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    let mut batches = Article::all().order_desc("title").batches(2);
    let mut sizes = Vec::new();
    let mut titles = Vec::new();
    while let Some(batch) = batches.next(&mut ctx).unwrap() {
        sizes.push(batch.len());
        titles.extend(batch.iter().map(|record| ctx[*record].title.clone().unwrap()));
    }
    assert_eq!(vec![2, 2, 1], sizes);
    assert_eq!(vec!["A", "B", "C", "D", "E"], titles);

    let mut capped = Article::all().limit(3).batches(2);
    let mut seen = 0;
    while let Some(batch) = capped.next(&mut ctx).unwrap() {
        seen += batch.len();
    }
    assert_eq!(3, seen);

    let mut filtered = Article::all().where_eq("status", "published").batches(1000);
    assert_eq!(2, filtered.next(&mut ctx).unwrap().unwrap().len());
    assert!(filtered.next(&mut ctx).unwrap().is_none());
}

#[test]
fn test_pluck_present_reads_a_not_null_column() {
    let mut ctx = support::ctx();
    articles(&mut ctx);
    assert_eq!(vec![1, 2, 3, 4, 5], Article::all().order_asc("title").pluck_present::<i64>(&mut ctx, "comments_count").unwrap());
}

#[test]
fn test_sum_integers_fails_where_ruby_raises() {
    assert_eq!(6, rustonrails::sum_integers(0, [1, 2, 3]).unwrap());
    assert_eq!(10, rustonrails::sum_integers(4, [Some(1), Some(2), Some(3)]).unwrap());
    assert_eq!(0, rustonrails::sum_integers(0, Vec::<i64>::new()).unwrap());
    assert!(matches!(rustonrails::sum_integers(0, [Some(1), None]), Err(Error::NilCoerced { into: "Integer" })));
    assert_eq!(i64::MIN, rustonrails::sum_integers(0, [i64::MIN, 1, -1]).unwrap());
    assert!(matches!(rustonrails::sum_integers(0, [i64::MAX, 1]), Err(Error::Overflow { value }) if value == "9223372036854775808"));
}

/// From a Float, Ruby adds each element in turn: 0.1 + 0.2 + 0.3 is
/// 0.6000000000000001, where `[0.1, 0.2, 0.3].sum` from 0 compensates.
#[test]
fn test_sum_floats_adds_in_turn() {
    assert_eq!(0.6000000000000001, rustonrails::sum_floats(0.0, [0.1, 0.2, 0.3]).unwrap());
    assert_eq!(3.0, rustonrails::sum_floats(0.0, [1, 2].map(|item| item as f64)).unwrap());
    assert!(matches!(rustonrails::sum_floats(0.0, [Some(1.0), None]), Err(Error::NilCoerced { into: "Float" })));
}
