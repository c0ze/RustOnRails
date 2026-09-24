use crate::{Behavior, Result, Value};

/// Column-level plumbing for a model struct. `model!` implements it.
pub trait Record: Clone + Default + PartialEq + Send + 'static {
    const NAME: &'static str;
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];

    /// An unsaved record holding the columns' database defaults, like `Post.new`.
    fn new_record() -> Self;
    fn get(&self, column: &str) -> Value;
    fn set(&mut self, column: &str, value: Value) -> Result<()>;

    fn id(&self) -> Option<i64> {
        match self.get("id") {
            Value::Int(id) => Some(id),
            _ => None,
        }
    }
}

/// A record plus its class-level declarations. Class methods such as
/// `Post.find` are default methods here (Task 3).
pub trait Model: Record {
    fn behavior() -> &'static Behavior<Self>;
}

/// Declares a model struct: one `Option` field per column, since any Ruby
/// attribute can be nil until the database says otherwise, plus `Record`.
/// `= default` gives the column's database default.
#[macro_export]
macro_rules! model {
    ($(#[$meta:meta])* $vis:vis struct $name:ident in $table:literal {
        $($field:ident : $ty:ty $(= $default:expr)?),* $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq)]
        $vis struct $name { $(pub $field: Option<$ty>),* }

        impl $crate::Record for $name {
            const NAME: &'static str = stringify!($name);
            const TABLE: &'static str = $table;
            const COLUMNS: &'static [&'static str] = &[$(stringify!($field)),*];

            fn new_record() -> Self {
                Self { $($field: $crate::__model_default!($($default)?)),* }
            }

            fn get(&self, column: &str) -> $crate::Value {
                match column {
                    $(stringify!($field) => $crate::Value::from(self.$field.clone()),)*
                    _ => $crate::Value::Nil,
                }
            }

            fn set(&mut self, column: &str, value: $crate::Value) -> $crate::Result<()> {
                match column {
                    $(stringify!($field) => {
                        self.$field = <$ty as $crate::FromValue>::from_value(value)?;
                        Ok(())
                    })*
                    _ => Err($crate::Error::UnknownAttribute { model: stringify!($name), name: column.to_string() }),
                }
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __model_default {
    () => { None };
    ($default:expr) => { Some(($default).into()) };
}
