mod support;

use rustonrails::{BelongsTo, HasMany, Model, Time, model, now, sanitize_sql_like};

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

impl Article {
    // belongs_to :user
    pub const AUTHOR: BelongsTo<Article, Author> = BelongsTo::new("user", "user_id");
}

impl Author {
    // has_many :comments
    pub const REMARKS: HasMany<Author, Remark> = HasMany::new("comments", "user_id", None);
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
        user_id: Some(user_id), title: Some(title.into()), body: Some("b".into()), status: Some(0), comments_count: Some(0),
        created_at: Some(now()), updated_at: Some(now()), ..Article::default()
    };
    Article::insert(ctx, record).unwrap()
}

fn remark(ctx: &mut rustonrails::Ctx, user_id: i64, post_id: i64, body: &str) {
    let record = Remark { post_id: Some(post_id), user_id: Some(user_id), body: Some(body.into()), created_at: Some(now()), updated_at: Some(now()), ..Remark::default() };
    Remark::insert(ctx, record).unwrap();
}

/// `Article.joins(user: :comments).where(comments: { body: "hi" })`: the
/// SQL Rails writes, duplicates and all, every column qualified.
#[test]
fn test_joins_follow_associations_like_rails() {
    let mut ctx = support::ctx();
    let (ann, bob) = (author(&mut ctx, "ann"), author(&mut ctx, "bob"));
    let (a, b, c) = (article(&mut ctx, ann, "A"), article(&mut ctx, ann, "B"), article(&mut ctx, bob, "C"));
    remark(&mut ctx, ann, c, "hi");
    remark(&mut ctx, ann, c, "hi");
    remark(&mut ctx, bob, a, "no");
    let joined = Article::all().joins(&Article::AUTHOR).joins(&Author::REMARKS).where_on::<Remark>("body", "hi");

    let titles: Vec<String> =
        joined.clone().order_asc("title").load(&mut ctx).unwrap().into_iter().map(|h| ctx[h].title.clone().unwrap()).collect();
    assert_eq!(vec!["A", "A", "B", "B"], titles);
    assert_eq!(4, joined.count(&mut ctx).unwrap());
    let found = joined.find(&mut ctx, b).unwrap();
    assert_eq!(Some(b), ctx[found].id);
    assert!(joined.find(&mut ctx, c).is_err());
    let (sql, _) = joined.order_asc("id").to_sql();
    let expected = r#"FROM "posts" INNER JOIN "users" ON "users"."id" = "posts"."user_id" INNER JOIN "comments" ON "comments"."user_id" = "users"."id" WHERE "comments"."body" = $1 ORDER BY "posts"."id" ASC"#;
    assert!(sql.contains(expected), "{sql}");
}

/// A joined column's value is cast by its own model: a param string finds
/// an integer key, as Rails' type casting does.
#[test]
fn test_where_on_casts_by_the_joined_model() {
    let mut ctx = support::ctx();
    let ann = author(&mut ctx, "ann");
    let a = article(&mut ctx, ann, "A");
    remark(&mut ctx, ann, a, "hi");
    let found = Author::all().joins(&Author::REMARKS).where_on::<Remark>("post_id", a.to_string()).load(&mut ctx).unwrap();
    assert_eq!(vec![Some(ann)], found.into_iter().map(|h| ctx[h].id).collect::<Vec<_>>());
}

/// `offset` skips rows after the order, and `include?` on an offset
/// relation looks in its rows, as Rails does.
#[test]
fn test_offset_skips_rows() {
    let mut ctx = support::ctx();
    let ann = author(&mut ctx, "ann");
    let ids: Vec<i64> = ["A", "B", "C"].iter().map(|t| article(&mut ctx, ann, t)).collect();
    let second = Article::all().order_asc("id").offset(1).limit(1);
    let loaded = second.load(&mut ctx).unwrap();
    assert_eq!(vec![Some(ids[1])], loaded.iter().map(|h| ctx[*h].id).collect::<Vec<_>>());
    let (first, third) = (Article::find(&mut ctx, ids[0]).unwrap(), Article::find(&mut ctx, ids[2]).unwrap());
    assert!(!second.contains(&mut ctx, first).unwrap());
    assert!(Article::all().order_asc("id").offset(1).contains(&mut ctx, third).unwrap());
    assert!(!Article::all().order_asc("id").offset(3).contains(&mut ctx, third).unwrap());
    assert!(second.to_sql().0.ends_with("LIMIT 1 OFFSET 1"));
}

/// `where("title ILIKE ? AND body = ?", ...)`: parenthesized as Rails
/// writes it, its binds numbered after the conditions before it.
#[test]
fn test_a_sql_fragment_binds_in_order() {
    let mut ctx = support::ctx();
    let ann = author(&mut ctx, "ann");
    let notes = article(&mut ctx, ann, "Draft notes");
    article(&mut ctx, ann, "Other");
    let relation = Article::all().where_eq("status", 0).where_sql("title ILIKE ? AND body = ?", vec!["%notes".into(), "b".into()]);
    let (sql, _) = relation.to_sql();
    assert!(sql.contains(r#"WHERE "posts"."status" = $1 AND (title ILIKE $2 AND body = $3)"#), "{sql}");
    let found = relation.load(&mut ctx).unwrap();
    assert_eq!(vec![Some(notes)], found.into_iter().map(|h| ctx[h].id).collect::<Vec<_>>());
}

/// Rails' sanitize_sql_like: a search for "50%" finds "50% off" only.
#[test]
fn test_sanitize_sql_like_matches_the_string_itself() {
    assert_eq!(r"50\%\_off\\", sanitize_sql_like(r"50%_off\"));
    let mut ctx = support::ctx();
    let ann = author(&mut ctx, "ann");
    let titles = ["50% off", "500 off", "a_b", "axb", r"back\slash"];
    let ids: Vec<i64> = titles.iter().map(|t| article(&mut ctx, ann, t)).collect();
    let search = |ctx: &mut rustonrails::Ctx, q: &str| -> Vec<i64> {
        let pattern = format!("%{}%", sanitize_sql_like(q));
        let found = Article::all().where_sql("title ILIKE ?", vec![pattern.into()]).order_asc("id").load(ctx).unwrap();
        found.into_iter().map(|h| ctx[h].id.unwrap()).collect()
    };
    assert_eq!(vec![ids[0]], search(&mut ctx, "50%"));
    assert_eq!(vec![ids[2]], search(&mut ctx, "a_b"));
    assert_eq!(vec![ids[4]], search(&mut ctx, r"k\s"));
}
