use std::fmt;

use crate::{Errors, Value};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// `ActiveRecord::RecordInvalid`, from the bang methods: the invalid
/// record's model and its errors, which a `rescue_from` handler taking the
/// exception renders as `error.record.errors`. Clone, as a Ruby local
/// copied from it would be.
#[derive(Clone, Debug)]
pub struct RecordInvalid {
    pub model: &'static str,
    pub errors: Errors,
}

/// What the record layer raises where Rails would raise an exception.
#[derive(Debug)]
pub enum Error {
    /// `ActiveRecord::RecordNotFound`
    RecordNotFound { model: &'static str, conditions: Option<String> },
    RecordInvalid(RecordInvalid),
    /// `ActiveRecord::RecordNotSaved`: a callback stopped `save!`.
    RecordNotSaved { model: &'static str },
    /// `ActiveRecord::RecordNotDestroyed`: a callback stopped `destroy!`.
    RecordNotDestroyed { model: &'static str },
    /// A before callback stopped the chain, like `throw :abort`.
    Abort,
    /// `raise ActiveRecord::Rollback`: rolls back the transaction block
    /// around it, which then gives nil.
    Rollback,
    /// A method called on nil, Ruby's `NoMethodError` for `nil`.
    Nil { what: &'static str },
    /// nil where a number is added, Ruby's TypeError.
    NilCoerced { into: &'static str },
    /// A method the value's class doesn't have, Ruby's `NoMethodError`.
    NoMethod { what: &'static str, value: Value },
    /// Ruby's ZeroDivisionError: an Integer divided by 0, or any `%` by 0.
    ZeroDivision,
    /// Ruby's TypeError: `1 + "a"`, `"a" + 1`.
    Type { message: String },
    /// Ruby's ArgumentError: `1 < "a"`, `"a" * -1`.
    Argument { message: String },
    /// An Integer Ruby would promote to a Bignum.
    Overflow { value: String },
    /// A value an attribute's type can't hold.
    Cast { expected: &'static str, value: Value },
    UnknownAttribute { model: &'static str, name: String },
    /// Writing an enum label the enum doesn't define (Rails' ArgumentError).
    InvalidEnum { attribute: &'static str, value: String },
    /// Writing a record that was never saved, like `increment!` on a new one.
    NotPersisted { model: &'static str },
    /// `ActionController::ParameterMissing`
    ParameterMissing { key: &'static str },
    /// Redis refused or couldn't be reached (the jobs' queue).
    Redis { message: String },
    /// A Ruby exception of a class nothing here rescues, such as Active
    /// Job's, named for the error a worker records.
    Raised { class: &'static str, message: String },
    /// Anything the database reported.
    Db(postgres::Error),
    /// A database URL whose TLS settings can't be used: an `sslmode`
    /// libpq doesn't know, an `sslrootcert` that can't be read.
    Connect(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::RecordNotFound { model, conditions: Some(c) } => write!(f, "Couldn't find {model} with {c}"),
            Error::RecordNotFound { model, conditions: None } => write!(f, "Couldn't find {model}"),
            Error::RecordInvalid(invalid) => write!(f, "Validation failed: {}", invalid.errors.full_messages().join(", ")),
            Error::RecordNotSaved { .. } => write!(f, "Failed to save the record"),
            Error::RecordNotDestroyed { model } => write!(f, "Failed to destroy {model}"),
            Error::Abort => write!(f, "callback chain aborted"),
            Error::Rollback => write!(f, "ActiveRecord::Rollback"),
            Error::Nil { what } => write!(f, "undefined method '{what}' for nil"),
            Error::NilCoerced { into } => write!(f, "nil can't be coerced into {into}"),
            Error::NoMethod { what, value } => write!(f, "undefined method '{what}' for {value:?}"),
            Error::Type { message } | Error::Argument { message } => write!(f, "{message}"),
            Error::ZeroDivision => write!(f, "divided by 0"),
            Error::Redis { message } | Error::Raised { message, .. } => write!(f, "{message}"),
            Error::Overflow { value } => write!(f, "{value} doesn't fit in a 64-bit integer"),
            Error::Cast { expected, value } => write!(f, "can't cast {value:?} to {expected}"),
            Error::UnknownAttribute { model, name } => write!(f, "unknown attribute '{name}' for {model}."),
            Error::InvalidEnum { attribute, value } => write!(f, "'{value}' is not a valid {attribute}"),
            Error::NotPersisted { model } => write!(f, "cannot update a new {model}"),
            Error::ParameterMissing { key } => write!(f, "param is missing or the value is empty or invalid: {key}"),
            Error::Connect(message) => write!(f, "{message}"),
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
