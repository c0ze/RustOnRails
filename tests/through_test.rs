mod support;

use rustonrails::{Error, HasManyThrough, Model, Record, Time, model, now};

model! {
    pub struct Author in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

model! {
    pub struct Article in "posts" {
        id: i64, user_id: i64, title: String, body: String, status: i64 = 0, published_at: Time,
        comments_count: i64 = 0, created_at: Time, updated_at: Time,
    }
}

model! {
    pub struct Remark in "comments" { id: i64, post_id: i64, user_id: i64, body: String, created_at: Time, updated_at: Time }
}

impl Author {
    // has_many :commented, through: :comments, source: :post
    pub const COMMENTED: HasManyThrough<Author, Article> = HasManyThrough::new("commented", "comments", "user_id", "post_id");
}

impl Model for Author {
    fn behavior() -> &'static rustonrails::Behavior<Self> {
        static B: std::sync::LazyLock<rustonrails::Behavior<Author>> = std::sync::LazyLock::new(rustonrails::Behavior::new);
        &B
    }
}
impl Model for Article {
    fn behavior() -> &'static rustonrails::Behavior<Self> {
        static B: std::sync::LazyLock<rustonrails::Behavior<Article>> = std::sync::LazyLock::new(rustonrails::Behavior::new);
        &B
    }
}
impl Model for Remark {
    fn behavior() -> &'static rustonrails::Behavior<Self> {
        static B: std::sync::LazyLock<rustonrails::Behavior<Remark>> = std::sync::LazyLock::new(rustonrails::Behavior::new);
        &B
    }
}

fn author(ctx: &mut rustonrails::Ctx, name: &str) -> i64 {
    let record = Author { name: Some(name.into()), email: Some(format!("{name}@example.com")), created_at: Some(now()), updated_at: Some(now()), ..Author::default() };
    Author::insert(ctx, record).unwrap()
}

fn article(ctx: &mut rustonrails::Ctx, user_id: i64, title: &str) -> i64 {
    let record = Article {
        user_id: Some(user_id), title: Some(title.into()), body: Some("b".into()), status: Some(1), comments_count: Some(0),
        created_at: Some(now()), updated_at: Some(now()), ..Article::default()
    };
    Article::insert(ctx, record).unwrap()
}

fn remark(ctx: &mut rustonrails::Ctx, user_id: i64, post_id: i64) {
    let record = Remark { post_id: Some(post_id), user_id: Some(user_id), body: Some("r".into()), created_at: Some(now()), updated_at: Some(now()), ..Remark::default() };
    Remark::insert(ctx, record).unwrap();
}

#[test]
fn test_a_through_relation_joins_like_rails() {
    let mut ctx = support::ctx();
    let (u1, u2) = (author(&mut ctx, "ann"), author(&mut ctx, "bob"));
    let (a, b, c) = (article(&mut ctx, u2, "A"), article(&mut ctx, u2, "B"), article(&mut ctx, u1, "C"));
    remark(&mut ctx, u1, a);
    remark(&mut ctx, u1, a);
    remark(&mut ctx, u1, b);
    remark(&mut ctx, u2, c);
    let ann = Author::find(&mut ctx, u1).unwrap();
    let commented = Author::COMMENTED.of(&ctx, ann);

    let titles: Vec<String> = commented.clone().order_asc("title").load(&mut ctx).unwrap().into_iter().map(|h| ctx[h].title.clone().unwrap()).collect();
    assert_eq!(vec!["A", "A", "B"], titles);
    let narrowed = commented.clone().where_eq("title", "B").load(&mut ctx).unwrap();
    assert_eq!(vec![Some(b)], narrowed.into_iter().map(|h| ctx[h].id).collect::<Vec<_>>());
    assert!(matches!(commented.find(&mut ctx, c), Err(Error::RecordNotFound { .. })));
    let found = commented.find(&mut ctx, b).unwrap();
    assert_eq!(Some(b), ctx[found].id);
    let (bh, ch) = (Article::find(&mut ctx, b).unwrap(), Article::find(&mut ctx, c).unwrap());
    assert!(commented.contains(&mut ctx, bh).unwrap());
    assert!(!commented.contains(&mut ctx, ch).unwrap());
    assert!(!commented.contains(&mut ctx, None).unwrap());
    assert_eq!(3, commented.count(&mut ctx).unwrap());
    let (sql, _) = commented.to_sql();
    assert!(sql.contains(r#"FROM "posts" INNER JOIN "comments" ON "posts"."id" = "comments"."post_id" WHERE "comments"."user_id" = $1"#), "{sql}");
}

#[test]
fn test_an_unsaved_owner_has_nothing_through() {
    let mut ctx = support::ctx();
    let nobody = ctx.build(Author::new_record());
    assert!(Author::COMMENTED.of(&ctx, nobody).load(&mut ctx).unwrap().is_empty());
}

/// Rails loads a limited relation for `include?`: filtering by the id first
/// would change which rows the limit keeps.
#[test]
fn test_include_on_a_limited_relation_looks_in_its_rows() {
    let mut ctx = support::ctx();
    let u1 = author(&mut ctx, "ann");
    let (a, b) = (article(&mut ctx, u1, "A"), article(&mut ctx, u1, "B"));
    remark(&mut ctx, u1, a);
    remark(&mut ctx, u1, b);
    let ann = Author::find(&mut ctx, u1).unwrap();
    let first = Author::COMMENTED.of(&ctx, ann).order_asc("id").limit(1);
    let (ah, bh) = (Article::find(&mut ctx, a).unwrap(), Article::find(&mut ctx, b).unwrap());
    assert!(first.contains(&mut ctx, ah).unwrap());
    assert!(!first.contains(&mut ctx, bh).unwrap());
    assert!(!first.clone().limit(0).contains(&mut ctx, ah).unwrap());
}
