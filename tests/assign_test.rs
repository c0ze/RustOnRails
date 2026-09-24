mod blog;
mod support;

use blog::fixtures;
use blog::Post;
use rustonrails::{Model, Value};

fn attrs(pairs: &[(&str, Value)]) -> Vec<(String, Value)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn test_from_attributes_casts_each_value() {
    let post = Post::from_attributes(&attrs(&[("user_id", Value::from("5")), ("title", Value::from("Hi"))])).unwrap();
    assert_eq!(Some(5), post.user_id);
    assert_eq!(Some("Hi"), post.title.as_deref());
    assert_eq!(Some("draft"), post.status.as_deref());
}

#[test]
fn test_enum_takes_a_label_or_its_integer() {
    let by_number = Post::from_attributes(&attrs(&[("status", Value::Int(1))])).unwrap();
    assert_eq!(Some("published"), by_number.status.as_deref());
    let by_string = Post::from_attributes(&attrs(&[("status", Value::from("1"))])).unwrap();
    assert_eq!(Some("1"), by_string.status.as_deref());
}

#[test]
fn test_unknown_attribute_is_an_error() {
    let error = Post::from_attributes(&attrs(&[("nope", Value::Int(1))])).unwrap_err();
    assert_eq!("unknown attribute 'nope' for Post.", error.to_string());
}

#[test]
fn test_assign_then_save() {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    let post = Post::find(&mut ctx, fx.draft).unwrap();
    ctx.assign(post, &attrs(&[("title", Value::from("Done"))])).unwrap();
    ctx.save_bang(post).unwrap();
    ctx.reload(post).unwrap();
    assert_eq!(Some("Done"), ctx[post].title.as_deref());
}
