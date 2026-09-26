//! Rails' cookie store, read and written as Rails 8.1 does it.

use rustonrails::{CookieKey, CookieOptions, Cookies, Session, SessionStore, Value};

/// A throwaway test-environment secret, and a session cookie Rails 8.1
/// wrote with it (the store example's cart after one `add`).
const SECRET: &str = "542312c5836d0a812e864484398fd222707cfb85bb1bd9d086b13a3bd77486eb0d6c28bece72dd6b8f820759c65cd26a47e3e48e3242ea1c595f630d9d36703f";
const RAILS_COOKIE: &str = "8iQGk%2FIKndXZm%2F9SXaqotH%2FjDeF3KxMVFYSMtsbHEj1AibtEKg8arEkTAua3DQtK5jJOalh7YROAzhft8XhX7KMH4gLb4aFV6JuRVMKRvwbrhY6YUFtQx%2Br4HFZu6J%2FADqR%2FSFKBoySQBOVn4CqEv6Rmt6Ch%2FPfFfb3QjGYxQWzuTSipfvBRobPVnjhaaAMt3rNchSOMZARMVTaAPHaNcYwVH7K6v8x8Jcy3mP%2B9ta2tXqIe0Xz3f%2Fv%2FST9wBltiSuQ7TL4S1yU%2Blg%3D%3D--7RpuHzF8d3eeMQrS--sbc4HxLFdfQ8YLvrxdTBuw%3D%3D";

fn session(cookie: Option<&str>) -> Session {
    let store = SessionStore { name: "_store_session", key: Some(CookieKey::derive(SECRET)), options: CookieOptions::session() };
    let cookies = Cookies::parse(cookie.map(|value| format!("_store_session={value}")).as_deref());
    Session::new(Some(store), cookies.get("_store_session"))
}

#[test]
fn test_rails_cookie_is_read() {
    let mut session = session(Some(RAILS_COOKIE));
    assert_eq!(Value::Int(18276415), session.get("product_id").unwrap());
    assert_eq!(Value::Int(1), session.get("quantity").unwrap());
    assert_eq!(Value::from("ann"), session.get("shopper").unwrap());
    assert_eq!(Value::Nil, session.get("missing").unwrap());
}

/// What this side writes, the same key reads back, as Rails would.
#[test]
fn test_a_written_session_reads_back() {
    let mut session = session(Some(RAILS_COOKIE));
    session.set("quantity", 5).unwrap();
    session.set("shopper", Value::Nil).unwrap();
    let header = session.header().unwrap().expect("a loaded session sends its cookie");
    assert!(header.ends_with("; path=/; httponly; samesite=lax"), "{header}");
    let value = header.trim_start_matches("_store_session=").split(';').next().unwrap();
    let mut again = self::session(Some(value));
    assert_eq!(Value::Int(5), again.get("quantity").unwrap());
    assert_eq!(Value::Nil, again.get("shopper").unwrap());
    assert_eq!(Value::Int(18276415), again.get("product_id").unwrap());
}

#[test]
fn test_what_sends_no_cookie() {
    // Nothing touched it, or it was only read with no cookie to read.
    assert_eq!(None, session(Some(RAILS_COOKIE)).header().unwrap());
    let mut fresh = session(None);
    assert_eq!(Value::Nil, fresh.get("product_id").unwrap());
    assert_eq!(Value::Nil, fresh.delete("product_id").unwrap());
    assert_eq!(None, fresh.header().unwrap());
    // A cookie another secret wrote, or one tampered with, isn't a session.
    let mut other = Session::new(
        Some(SessionStore { name: "_store_session", key: Some(CookieKey::derive("another")), options: CookieOptions::session() }),
        Cookies::parse(Some(&format!("_store_session={RAILS_COOKIE}"))).get("_store_session"),
    );
    assert_eq!(Value::Nil, other.get("product_id").unwrap());
    assert_eq!(None, other.header().unwrap());
}

#[test]
fn test_reset_starts_a_new_session() {
    let mut session = session(Some(RAILS_COOKIE));
    session.reset().unwrap();
    assert_eq!(Value::Nil, session.get("product_id").unwrap());
    let header = session.header().unwrap().unwrap();
    let value = header.trim_start_matches("_store_session=").split(';').next().unwrap();
    let key = CookieKey::derive(SECRET);
    let plain = key.decrypt("_store_session", &Cookies::parse(Some(&format!("c={value}"))).get("c").unwrap()).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(1, json.as_object().unwrap().len());
    assert_eq!(32, json["session_id"].as_str().unwrap().len());
}

/// A cookie sealed for another name isn't this one.
#[test]
fn test_the_purpose_is_checked() {
    let key = CookieKey::derive(SECRET);
    let sealed = key.encrypt("_other_session", b"{}");
    assert_eq!(None, key.decrypt("_store_session", &sealed));
    assert_eq!(Some(b"{}".to_vec()), key.decrypt("_other_session", &sealed));
}

#[test]
fn test_plain_cookies() {
    let mut cookies = Cookies::parse(Some("visits=2; a=%2B+b; visits=9"));
    assert_eq!(Some("2".to_string()), cookies.get("visits"));
    assert_eq!(Some("+ b".to_string()), cookies.get("a"));
    cookies.set("visits", "2".into());
    assert!(cookies.headers(Some("lax")).is_empty(), "an unchanged value isn't sent back");
    cookies.set("visits", "3 & 4".into());
    assert_eq!(vec!["visits=3+%26+4; path=/; samesite=lax".to_string()], cookies.headers(Some("lax")));
    assert_eq!(Some("3 & 4".to_string()), cookies.get("visits"));
}

/// The store's options, in Rack's order: path, secure, httponly, samesite.
#[test]
fn test_cookie_attributes() {
    let options = CookieOptions { path: "/app", secure: true, httponly: true, same_site: Some("strict".into()) };
    assert_eq!("; path=/app; secure; httponly; samesite=strict", options.attributes());
    assert_eq!("; path=/; httponly; samesite=lax", CookieOptions::session().attributes());
    let mut cookies = Cookies::parse(None);
    cookies.set("a", "1".into());
    assert_eq!(vec!["a=1; path=/".to_string()], cookies.headers(None));
}

/// Rails' encrypted jar refuses a cookie past 4096 bytes: the request fails
/// rather than a browser silently dropping the session.
#[test]
fn test_a_session_too_big_for_a_cookie() {
    let mut session = session(None);
    session.set("shopper", "x".repeat(5000)).unwrap();
    match session.header() {
        Err(rustonrails::Error::Raised { class, message }) => {
            assert_eq!("ActionDispatch::Cookies::CookieOverflow", class);
            assert!(message.starts_with("_store_session cookie overflowed with size "), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

/// Rack 3.2 unescapes cookie values, not names.
#[test]
fn test_cookie_names_are_taken_as_they_are() {
    let cookies = Cookies::parse(Some("%76isits=5; visits=6"));
    assert_eq!(Some("6".to_string()), cookies.get("visits"));
    assert_eq!(Some("5".to_string()), cookies.get("%76isits"));
}

/// Like Rails, a server whose app keeps a session won't start without the secret.
#[test]
fn test_a_session_store_needs_its_secret() {
    let router = rustonrails::Router::new().session_store("_store_session");
    assert!(router.needs_secret());
    assert!(!router.secret_key_base(Some(SECRET)).needs_secret());
    assert!(!rustonrails::Router::new().needs_secret());
}
