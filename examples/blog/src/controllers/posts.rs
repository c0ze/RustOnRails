use rustonrails::{
    AsJson, Attributes, Controller, Error, Handle, Model, Record, Request, Response, Result, errors_json, status,
};

use super::application;
use crate::models::{Post, PostScopes, User};

// posts_controller.rb
#[derive(Default)]
pub struct PostsController {
    post: Option<Handle<Post>>,
}

impl Controller for PostsController {
    fn wrap_parameters() -> Option<(&'static str, &'static [&'static str])> {
        Some(("post", Post::COLUMNS))
    }

    // before_action :set_post, only: %i[show update destroy]
    fn before(&mut self, req: &mut Request, action: &str) -> Result<Option<Response>> {
        if matches!(action, "show" | "update" | "destroy") {
            self.set_post(req)?;
        }
        Ok(None)
    }

    fn rescue(&mut self, req: &mut Request, error: Error) -> Result<Response> {
        application::rescue(req, error)
    }
}

impl PostsController {
    // def index
    pub fn index(&mut self, req: &mut Request) -> Result<Response> {
        let posts = Post::all().visible().recent().includes(&Post::USER).limit(20).load(&mut req.ctx)?;
        let options = AsJson::<Post>::new().include(&Post::USER, AsJson::<User>::new().only(&["id", "name"]));
        Ok(Response::json(status::OK, options.render_all(&mut req.ctx, &posts)?))
    }

    // def show
    pub fn show(&mut self, req: &mut Request) -> Result<Response> {
        let post = self.post()?;
        Ok(Response::json(status::OK, AsJson::<Post>::new().render(&mut req.ctx, post)?))
    }

    // def create
    pub fn create(&mut self, req: &mut Request) -> Result<Response> {
        let attributes = post_params(req)?;
        let post = req.ctx.build(Post::from_attributes(&attributes)?);
        if req.ctx.save(post)? {
            Ok(Response::json(status::CREATED, AsJson::<Post>::new().render(&mut req.ctx, post)?))
        } else {
            Ok(Response::json(status::UNPROCESSABLE_CONTENT, errors_json(req.ctx.errors(post))))
        }
    }

    // def update
    pub fn update(&mut self, req: &mut Request) -> Result<Response> {
        let post = self.post()?;
        let attributes = post_params(req)?;
        req.ctx.assign(post, &attributes)?;
        if req.ctx.save(post)? {
            Ok(Response::json(status::OK, AsJson::<Post>::new().render(&mut req.ctx, post)?))
        } else {
            Ok(Response::json(status::UNPROCESSABLE_CONTENT, errors_json(req.ctx.errors(post))))
        }
    }

    // def destroy
    pub fn destroy(&mut self, req: &mut Request) -> Result<Response> {
        let post = self.post()?;
        req.ctx.destroy_bang(post)?;
        Ok(Response::head(status::NO_CONTENT))
    }

    // def set_post
    fn set_post(&mut self, req: &mut Request) -> Result<()> {
        let id = req.params.value("id");
        self.post = Some(Post::find(&mut req.ctx, id)?);
        Ok(())
    }

    /// `@post`, which set_post assigned.
    fn post(&self) -> Result<Handle<Post>> {
        self.post.ok_or(Error::Nil { what: "@post" })
    }
}

// def post_params
fn post_params(req: &Request) -> Result<Attributes> {
    req.params.expect("post", &["user_id", "title", "body", "status"])
}
