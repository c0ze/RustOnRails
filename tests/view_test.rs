//! Action View's output as Rails 8.1 writes it: escaping, content_for,
//! layouts, link_to and route helpers' path segments.

use rustonrails::{Error, Value, View, html_escape, link_to, path_segment};

#[test]
fn test_escaping_is_erb_utils() {
    assert_eq!("&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt;", html_escape("<a href=\"x\">Tom & 'Jerry'</a>"));
    assert_eq!("caf\u{e9} \u{2014}", html_escape("caf\u{e9} \u{2014}"));
}

/// A template, then its layout around it, with what content_for kept.
#[test]
fn test_a_template_in_a_layout() {
    let mut view = View::default();
    view.content_for_append("title", &html_escape("Tea & cake"));
    view.text("<h1>");
    view.append("<Mug>");
    view.raw("<b>bold</b>");
    view.text("</h1>\n");
    view.lay_out();
    view.text("<title>");
    view.raw(&view.content_for("title").unwrap_or_else(|| "Store".to_string()));
    view.text("</title><main>");
    view.append_content();
    view.append_yield("footer");
    view.text("</main>");
    let response = view.response(200);
    assert_eq!("text/html; charset=utf-8", response.content_type.unwrap());
    assert_eq!(
        "<title>Tea &amp; cake</title><main><h1>&lt;Mug&gt;<b>bold</b></h1>\n</main>",
        String::from_utf8(response.body).unwrap()
    );
}

/// content_for reads nil for nothing or blank, as `.presence` does.
#[test]
fn test_content_for_reads() {
    let mut view = View::default();
    assert_eq!(None, view.content_for("title"));
    assert!(!view.has_content_for("title"));
    view.content_for_append("title", "  ");
    assert_eq!(None, view.content_for("title"));
    assert!(!view.has_content_for("title"));
    view.content_for_append("title", "A");
    view.content_for_append("title", "B");
    assert_eq!(Some("  AB".to_string()), view.content_for("title"));
    assert!(view.has_content_for("title"));
}

/// The attributes as given, href last, each escaped.
#[test]
fn test_link_to() {
    assert_eq!("<a href=\"/shop\">Shop</a>", link_to("Shop", "/shop", &[]));
    assert_eq!(
        "<a class=\"a &quot;b&quot;\" title=\"x&amp;y\" href=\"/q?a=1&amp;b=2\">&lt;i&gt;</a>",
        link_to("&lt;i&gt;", "/q?a=1&b=2", &[("class", "a \"b\""), ("title", "x&y")])
    );
}

/// Journey's escape_segment: unreserved and sub-delims stay, the rest is %XX.
#[test]
fn test_path_segments() {
    assert_eq!("42", path_segment(42_i64, "products", "show", "id").unwrap());
    assert_eq!("a-b.c_d~!$&'()*+,;=:@", path_segment("a-b.c_d~!$&'()*+,;=:@", "p", "s", "id").unwrap());
    assert_eq!("a%20b%2Fc%3F%23%25%C3%A9", path_segment("a b/c?#%\u{e9}", "p", "s", "id").unwrap());
    assert_eq!("7", path_segment(Value::Int(7), "p", "s", "id").unwrap());
    assert_eq!("x", path_segment(&"x".to_string(), "p", "s", "id").unwrap());
    match path_segment(None::<i64>, "storefront", "show", "id") {
        Err(Error::Raised { class, message }) => {
            assert_eq!("ActionController::UrlGenerationError", class);
            assert_eq!(
                "No route matches {action: \"show\", controller: \"storefront\", id: nil}, missing required keys: [:id]",
                message
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(path_segment(Value::Nil, "p", "s", "id").is_err());
}
