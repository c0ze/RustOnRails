mod fixtures;
mod support;

use fixtures::Fixtures;
use blog::models::Post;
use rustonrails::{
    AsJson, Controller, Error, Handle, Json, Model, Record, RecordInvalid, Request, Response, Result, Router, action, errors_json, json, status,
};

#[derive(Default)]
struct PostsController {
    post: Option<Handle<Post>>,
}

impl Controller for PostsController {
    fn wrap_parameters() -> Option<(&'static str, Option<&'static [&'static str]>)> {
        Some(("post", Some(Post::COLUMNS)))
    }

    fn before(&mut self, req: &mut Request, action: &str) -> Result<Option<Response>> {
        if req.params.value("halt").is_blank() {
            if matches!(action, "show" | "update") {
                self.set_post(req)?;
            }
            return Ok(None);
        }
        Ok(Some(Response::json(401, json!({"error": "halted"}))))
    }

    fn rescue(&mut self, req: &mut Request, error: Error) -> Result<Response> {
        match error {
            Error::RecordNotFound { .. } => Ok(Response::json(status::NOT_FOUND, json!({"error": "not found"}))),
            // rescue_from ActiveRecord::RecordInvalid, with: :invalid
            Error::RecordInvalid(error) => invalid(req, error),
            other => Err(other),
        }
    }
}

impl PostsController {
    fn set_post(&mut self, req: &mut Request) -> Result<()> {
        let id = req.params.value("id");
        self.post = Some(Post::find(&mut req.ctx, id)?);
        Ok(())
    }

    fn index(&mut self, req: &mut Request) -> Result<Response> {
        Ok(Response::json(status::OK, json!({"count": Post::all().count(&mut req.ctx)?})))
    }

    fn show(&mut self, req: &mut Request) -> Result<Response> {
        let post = self.post.ok_or(Error::Nil { what: "@post" })?;
        Ok(Response::json(status::OK, AsJson::<Post>::new().render(&mut req.ctx, post)?))
    }

    fn create(&mut self, req: &mut Request) -> Result<Response> {
        let attributes = req.params.expect("post", &["user_id", "title", "status"])?;
        let post = req.ctx.build(Post::from_attributes(&attributes)?);
        if req.ctx.save(post)? {
            Ok(Response::json(status::CREATED, AsJson::<Post>::new().render(&mut req.ctx, post)?))
        } else {
            Ok(Response::json(status::UNPROCESSABLE_CONTENT, errors_json(req.ctx.errors(post))))
        }
    }

    /// `Post.create!(post_params)`, leaving the failure to rescue_from.
    fn create_bang(&mut self, req: &mut Request) -> Result<Response> {
        let attributes = req.params.expect("post", &["user_id", "title", "status"])?;
        let post = req.ctx.build(Post::from_attributes(&attributes)?);
        req.ctx.save_bang(post)?;
        Ok(Response::json(status::CREATED, AsJson::<Post>::new().render(&mut req.ctx, post)?))
    }

    fn explode(&mut self, _req: &mut Request) -> Result<Response> {
        Err(Error::Abort)
    }
}

/// `def invalid(error) = render json: error.record.errors, status: :unprocessable_content`
fn invalid(_req: &mut Request, error: RecordInvalid) -> Result<Response> {
    Ok(Response::json(status::UNPROCESSABLE_CONTENT, errors_json(&error.errors)))
}

fn router() -> Router {
    Router::new()
        .get("/posts(.:format)", action("index", PostsController::index))
        .post("/posts(.:format)", action("create", PostsController::create))
        .post("/strict_posts(.:format)", action("create_bang", PostsController::create_bang))
        .get("/explode", action("explode", PostsController::explode))
        .get("/posts/:id(.:format)", action("show", PostsController::show))
}

fn setup() -> (rustonrails::Ctx, Fixtures) {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    (ctx, fx)
}

fn send(req: &mut Request) -> (u16, Json) {
    let response = router().call(req);
    (response.status, response.body_json())
}

#[test]
fn test_before_action_runs_only_for_listed_actions() {
    let (ctx, fx) = setup();
    let mut show = Request::new(ctx, "GET", &format!("/posts/{}", fx.draft));
    assert_eq!(json!("Work in progress"), send(&mut show).1["title"]);
    let mut index = Request::new(show.ctx, "GET", "/posts").with_query("id=nope");
    assert_eq!((200, json!({"count": 3})), send(&mut index));
}

#[test]
fn test_rescue_from_handles_not_found() {
    let (ctx, _) = setup();
    let mut req = Request::new(ctx, "GET", "/posts/0");
    assert_eq!((404, json!({"error": "not found"})), send(&mut req));
}

#[test]
fn test_unwrapped_json_is_wrapped_for_expect() {
    let (ctx, fx) = setup();
    let mut req = Request::new(ctx, "POST", "/posts").with_json(json!({"user_id": fx.alice, "title": "New", "status": 1}));
    let (status, body) = send(&mut req);
    assert_eq!(201, status);
    assert_eq!(json!("published"), body["status"]);
}

#[test]
fn test_invalid_create_renders_errors() {
    let (ctx, fx) = setup();
    let mut req = Request::new(ctx, "POST", "/posts").with_json(json!({"user_id": fx.alice, "title": ""}));
    assert_eq!((422, json!({"title": ["can't be blank"]})), send(&mut req));
}

#[test]
fn test_a_rescue_handler_renders_the_invalid_records_errors() {
    let (ctx, fx) = setup();
    let mut req = Request::new(ctx, "POST", "/strict_posts").with_json(json!({"post": {"user_id": fx.alice, "title": ""}}));
    assert_eq!((422, json!({"title": ["can't be blank"]})), send(&mut req));
}

#[test]
fn test_unrescued_errors_use_rails_statuses() {
    let (ctx, _) = setup();
    let mut missing = Request::new(ctx, "POST", "/posts").with_query("x=1");
    assert_eq!((400, json!({"status": 400, "error": "Bad Request"})), send(&mut missing));
    let mut broken = Request::new(missing.ctx, "GET", "/explode");
    assert_eq!((500, json!({"status": 500, "error": "Internal Server Error"})), send(&mut broken));
}

#[test]
fn test_before_filter_can_halt() {
    let (ctx, fx) = setup();
    let mut req = Request::new(ctx, "GET", &format!("/posts/{}", fx.draft)).with_query("halt=1");
    assert_eq!((401, json!({"error": "halted"})), send(&mut req));
}
