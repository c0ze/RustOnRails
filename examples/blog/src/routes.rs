use rustonrails::{Request, Router, action, health};

use crate::controllers::{CommentsController, PostsController, UsersController};

/// config/routes.rb, in the order Rails matches them.
pub fn routes() -> Router {
    Router::new()
        // resources :users, only: %i[show create] do
        //   get :lookup, on: :collection, constraints: ->(request) { ... }
        .get("/users/lookup(.:format)", action("lookup", UsersController::lookup))
        .constraint(email_given)
        .post("/users(.:format)", action("create", UsersController::create))
        .get("/users/:id(.:format)", action("show", UsersController::show))
        // resources :posts do
        //   resources :comments, only: %i[index create]
        .get("/posts/:post_id/comments(.:format)", action("index", CommentsController::index))
        .post("/posts/:post_id/comments(.:format)", action("create", CommentsController::create))
        .get("/posts(.:format)", action("index", PostsController::index))
        .post("/posts(.:format)", action("create", PostsController::create))
        .get("/posts/:id(.:format)", action("show", PostsController::show))
        .patch("/posts/:id(.:format)", action("update", PostsController::update))
        .put("/posts/:id(.:format)", action("update", PostsController::update))
        .delete("/posts/:id(.:format)", action("destroy", PostsController::destroy))
        // get "up" => "rails/health#show", as: :rails_health_check
        .get("/up(.:format)", Box::new(health))
}

// routes.rb:3  ->(request) { request.query_parameters["email"].present? }
fn email_given(req: &Request) -> bool {
    req.query.get("email").and_then(|email| email.as_str()).is_some_and(|email| !email.trim().is_empty())
}
