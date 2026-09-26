mod fixtures;
mod support;

use blog::models::{Post, PostScopes, User};
use rustonrails::{AsJson, Model, Record, Time, errors_json, format_time, json};

#[test]
fn test_times_use_activesupport_format() {
    let time = Time::parse_from_str("2026-09-24 17:25:29.674995", "%Y-%m-%d %H:%M:%S%.f").unwrap();
    assert_eq!("2026-09-24T17:25:29.674Z", format_time(time));
}

#[test]
fn test_record_renders_columns_in_order_with_labels() {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    let post = Post::find(&mut ctx, fx.draft).unwrap();
    let rendered = AsJson::<Post>::new().render(&mut ctx, post).unwrap();
    let keys: Vec<&String> = rendered.as_object().unwrap().keys().collect();
    assert_eq!(Post::COLUMNS.to_vec(), keys.iter().map(|k| k.as_str()).collect::<Vec<_>>());
    assert_eq!(json!("draft"), rendered["status"]);
    assert_eq!(json!(null), rendered["published_at"]);
    assert_eq!(json!(fx.draft), rendered["id"]);
    assert!(rendered["created_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn test_only_except_and_include() {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    let posts = Post::all().visible().recent().includes(&Post::USER).load(&mut ctx).unwrap();
    let options = AsJson::<Post>::new().include(&Post::USER, AsJson::<User>::new().only(&["id", "name"]));
    let rendered = options.render_all(&mut ctx, &posts).unwrap();
    assert_eq!(json!({"id": fx.bob, "name": "Bob"}), rendered[0]["user"]);
    let slim = AsJson::<Post>::new().only(&["title", "id"]).render(&mut ctx, posts[0]).unwrap();
    assert_eq!(json!({"id": fx.published_new, "title": "Fresh news"}), slim);
    let without = AsJson::<Post>::new().except(&["body"]).render(&mut ctx, posts[0]).unwrap();
    assert!(without.get("body").is_none() && without.get("title").is_some());
}

#[test]
fn test_include_many() {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    let post = Post::find(&mut ctx, fx.published_old).unwrap();
    let options = AsJson::<Post>::new().only(&["id"]).include_many(&Post::COMMENTS, AsJson::new().only(&["body"]));
    assert_eq!(json!({"id": fx.published_old, "comments": [{"body": "Nice one"}]}), options.render(&mut ctx, post).unwrap());
}

#[test]
fn test_errors_render_by_attribute() {
    let mut ctx = support::ctx();
    let post = ctx.build(Post::new_record());
    ctx.is_valid(post).unwrap();
    let rendered = errors_json(ctx.errors(post));
    assert_eq!(json!(["must exist"]), rendered["user"]);
    assert_eq!(json!(["can't be blank"]), rendered["title"]);
}

#[test]
fn test_render_option_renders_nil_as_null() {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    let post = Post::find(&mut ctx, fx.draft).unwrap();
    let options = AsJson::<Post>::new();
    assert_eq!(json!(null), options.render_option(&mut ctx, None).unwrap());
    assert_eq!(options.render(&mut ctx, post).unwrap(), options.render_option(&mut ctx, Some(post)).unwrap());
}

/// Rails leaves a nil belongs_to out of `include:` rather than writing null.
#[test]
fn test_a_nil_association_is_left_out_of_include() {
    let mut ctx = support::ctx();
    let post = ctx.build(Post { title: Some("t".into()), body: Some("b".into()), ..Post::new_record() });
    let json = AsJson::<Post>::new().include(&Post::USER, AsJson::new()).render(&mut ctx, post).unwrap();
    assert!(json.get("user").is_none(), "{json}");
}
