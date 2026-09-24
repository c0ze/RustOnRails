use rustonrails::{AsJson, Attributes, Controller, Error, Model, Record, Request, Response, Result, errors_json, status};

use super::application;
use crate::models::User;

// users_controller.rb
#[derive(Default)]
pub struct UsersController;

impl Controller for UsersController {
    fn wrap_parameters() -> Option<(&'static str, &'static [&'static str])> {
        Some(("user", User::COLUMNS))
    }

    fn rescue(&mut self, req: &mut Request, error: Error) -> Result<Response> {
        application::rescue(req, error)
    }
}

impl UsersController {
    // def show
    pub fn show(&mut self, req: &mut Request) -> Result<Response> {
        let id = req.params.value("id");
        let user = User::find(&mut req.ctx, id)?;
        Ok(Response::json(status::OK, AsJson::<User>::new().render(&mut req.ctx, user)?))
    }

    // def lookup
    pub fn lookup(&mut self, req: &mut Request) -> Result<Response> {
        let email = req.params.value("email").to_ruby_string().trim().to_lowercase();
        let user = User::find_by_bang(&mut req.ctx, "email", email)?;
        Ok(Response::json(status::OK, AsJson::<User>::new().render(&mut req.ctx, user)?))
    }

    // def create
    pub fn create(&mut self, req: &mut Request) -> Result<Response> {
        let attributes = user_params(req)?;
        let user = req.ctx.build(User::from_attributes(&attributes)?);
        if req.ctx.save(user)? {
            Ok(Response::json(status::CREATED, AsJson::<User>::new().render(&mut req.ctx, user)?))
        } else {
            Ok(Response::json(status::UNPROCESSABLE_CONTENT, errors_json(req.ctx.errors(user))))
        }
    }
}

// def user_params
fn user_params(req: &Request) -> Result<Attributes> {
    Ok(req.params.require("user")?.permit(&["name", "email"]))
}
