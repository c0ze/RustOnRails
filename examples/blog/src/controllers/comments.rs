use rustonrails::{
    AsJson, Attributes, Controller, Error, Handle, Model, Record, Request, Response, Result, errors_json, status,
};

use super::application;
use crate::models::{Comment, Post};

// comments_controller.rb
#[derive(Default)]
pub struct CommentsController {
    post: Option<Handle<Post>>,
}

impl Controller for CommentsController {
    fn wrap_parameters() -> Option<(&'static str, &'static [&'static str])> {
        Some(("comment", Comment::COLUMNS))
    }

    // before_action :set_post
    fn before(&mut self, req: &mut Request, _action: &str) -> Result<Option<Response>> {
        self.set_post(req)?;
        Ok(None)
    }

    fn rescue(&mut self, req: &mut Request, error: Error) -> Result<Response> {
        application::rescue(req, error)
    }
}

impl CommentsController {
    // def index
    pub fn index(&mut self, req: &mut Request) -> Result<Response> {
        let post = self.post()?;
        let comments = Post::COMMENTS.of(&req.ctx, post).order_asc("created_at").load(&mut req.ctx)?;
        Ok(Response::json(status::OK, AsJson::<Comment>::new().render_all(&mut req.ctx, &comments)?))
    }

    // def create
    pub fn create(&mut self, req: &mut Request) -> Result<Response> {
        let post = self.post()?;
        let attributes = comment_params(req)?;
        let comment = Post::COMMENTS.build(&mut req.ctx, post, Comment::from_attributes(&attributes)?)?;
        if req.ctx.save(comment)? {
            Ok(Response::json(status::CREATED, AsJson::<Comment>::new().render(&mut req.ctx, comment)?))
        } else {
            Ok(Response::json(status::UNPROCESSABLE_CONTENT, errors_json(req.ctx.errors(comment))))
        }
    }

    // def set_post
    fn set_post(&mut self, req: &mut Request) -> Result<()> {
        let id = req.params.value("post_id");
        self.post = Some(Post::find(&mut req.ctx, id)?);
        Ok(())
    }

    fn post(&self) -> Result<Handle<Post>> {
        self.post.ok_or(Error::Nil { what: "@post" })
    }
}

// def comment_params
fn comment_params(req: &Request) -> Result<Attributes> {
    req.params.expect("comment", &["user_id", "body"])
}
