pub mod application;
mod comments;
mod posts;
mod users;

pub use comments::CommentsController;
pub use posts::PostsController;
pub use users::UsersController;
