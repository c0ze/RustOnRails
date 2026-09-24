use crate::{Behavior, Ctx, Handle, Relation, Result, Value};

/// Column-level plumbing for a model struct. `model!` implements it.
pub trait Record: Clone + Default + PartialEq + Send + 'static {
    const NAME: &'static str;
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];

    /// An unsaved record holding the columns' database defaults, like `Post.new`.
    fn new_record() -> Self;
    fn get(&self, column: &str) -> Value;
    fn set(&mut self, column: &str, value: Value) -> Result<()>;
    /// A query value cast by the column's type (`where(user_id: "5")`).
    fn cast_query(column: &str, value: Value) -> Value;

    fn id(&self) -> Option<i64> {
        match self.get("id") {
            Value::Int(id) => Some(id),
            _ => None,
        }
    }
}

/// A record plus its class-level declarations. Class methods such as
/// `Post.find` are default methods here.
pub trait Model: Record {
    fn behavior() -> &'static Behavior<Self>;

    fn all() -> Relation<Self> {
        Relation::new()
    }

    fn find(ctx: &mut Ctx, id: i64) -> Result<Handle<Self>> {
        Self::all().find(ctx, id)
    }

    fn find_by(ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Option<Handle<Self>>> {
        Self::all().find_by(ctx, column, value)
    }

    fn find_by_bang(ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Handle<Self>> {
        Self::all().find_by_bang(ctx, column, value)
    }

    /// `Post.create(attributes)`: returns the record even when it didn't save.
    fn create(ctx: &mut Ctx, record: Self) -> Result<Handle<Self>> {
        let handle = ctx.build(record);
        ctx.save(handle)?;
        Ok(handle)
    }

    /// `Post.create!(attributes)`
    fn create_bang(ctx: &mut Ctx, record: Self) -> Result<Handle<Self>> {
        let handle = ctx.build(record);
        ctx.save_bang(handle)?;
        Ok(handle)
    }

    /// `Post.insert(attributes)`: one INSERT with no validations or
    /// callbacks; timestamps are filled in as Rails does. Returns the id.
    fn insert(ctx: &mut Ctx, mut record: Self) -> Result<i64> {
        crate::write::fill_timestamps(&mut record, crate::now())?;
        match crate::write::insert_row(ctx, &record)? {
            Value::Int(id) => Ok(id),
            other => Err(crate::Error::Cast { expected: "integer id", value: other }),
        }
    }
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

            fn cast_query(column: &str, value: $crate::Value) -> $crate::Value {
                match column {
                    $(stringify!($field) => <$ty as $crate::FromValue>::serialize(value).map_or($crate::Value::Nil, $crate::Value::from),)*
                    _ => value,
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
