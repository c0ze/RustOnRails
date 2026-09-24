use std::fmt;

use crate::Value;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What the record layer raises where Rails would raise an exception.
#[derive(Debug)]
pub enum Error {
    /// `ActiveRecord::RecordNotFound`
    RecordNotFound { model: &'static str, conditions: Option<String> },
    /// `ActiveRecord::RecordInvalid`, from the bang methods.
    RecordInvalid { model: &'static str, messages: Vec<String> },
    /// `ActiveRecord::RecordNotSaved`: a callback stopped `save!`.
    RecordNotSaved { model: &'static str },
    /// `ActiveRecord::RecordNotDestroyed`: a callback stopped `destroy!`.
    RecordNotDestroyed { model: &'static str },
    /// A before callback stopped the chain, like `throw :abort`.
    Abort,
    /// A method called on nil, Ruby's `NoMethodError` for `nil`.
    Nil { what: &'static str },
    /// A value an attribute's type can't hold.
    Cast { expected: &'static str, value: Value },
    UnknownAttribute { model: &'static str, name: String },
    /// Writing a record that was never saved, like `increment!` on a new one.
    NotPersisted { model: &'static str },
    /// Anything the database reported.
    Db(postgres::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::RecordNotFound { model, conditions: Some(c) } => write!(f, "Couldn't find {model} with {c}"),
            Error::RecordNotFound { model, conditions: None } => write!(f, "Couldn't find {model}"),
            Error::RecordInvalid { messages, .. } => write!(f, "Validation failed: {}", messages.join(", ")),
            Error::RecordNotSaved { .. } => write!(f, "Failed to save the record"),
            Error::RecordNotDestroyed { model } => write!(f, "Failed to destroy {model}"),
            Error::Abort => write!(f, "callback chain aborted"),
            Error::Nil { what } => write!(f, "undefined method '{what}' for nil"),
            Error::Cast { expected, value } => write!(f, "can't cast {value:?} to {expected}"),
            Error::UnknownAttribute { model, name } => write!(f, "unknown attribute '{name}' for {model}"),
            Error::NotPersisted { model } => write!(f, "cannot update a new {model}"),
            Error::Db(e) => match e.as_db_error() {
                Some(db) => write!(f, "{}: {}", db.severity(), db.message()),
                None => write!(f, "{e}"),
            },
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<postgres::Error> for Error {
    fn from(e: postgres::Error) -> Self {
        Error::Db(e)
    }
}
