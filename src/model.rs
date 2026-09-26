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
    /// 1 when a query value lies above everything the column can hold, -1
    /// below, 0 otherwise: Rails' `unboundable?`.
    fn query_bound(column: &str, value: &Value) -> i8;
    #[doc(hidden)]
    fn before_type_cast(&self) -> &BeforeTypeCast;
    #[doc(hidden)]
    fn before_type_cast_mut(&mut self) -> &mut BeforeTypeCast;

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

    fn find(ctx: &mut Ctx, id: impl Into<Value>) -> Result<Handle<Self>> {
        Self::all().find(ctx, id)
    }

    fn find_by(ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Option<Handle<Self>>> {
        Self::all().find_by(ctx, column, value)
    }

    fn find_by_bang(ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Handle<Self>> {
        Self::all().find_by_bang(ctx, column, value)
    }

    /// `Post.new(attributes)`
    fn from_attributes(attributes: &[(String, Value)]) -> Result<Self> {
        let mut record = Self::new_record();
        assign_to(&mut record, attributes)?;
        Ok(record)
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

/// Attribute values by name, in the order given, as `Post.new(title: "x")`
/// or permitted params supply them.
pub type Attributes = Vec<(String, Value)>;

/// `assign_attributes`: casts and sets each value, enums included, then
/// normalizes it, and keeps each value as given.
pub(crate) fn assign_to<M: Model>(record: &mut M, attributes: &[(String, Value)]) -> Result<()> {
    for (name, value) in attributes {
        record.set(name, M::behavior().cast_assignment(name, value.clone()))?;
        if M::behavior().normalizes_attribute(name) {
            let normalized = M::behavior().normalize(name, record.get(name));
            record.set(name, normalized)?;
        }
        let cast = record.get(name);
        record.before_type_cast_mut().assign(name, value.clone(), cast);
    }
    Ok(())
}

/// What `assign_attributes` was given, next to what it cast to: Rails
/// checks numericality against the value as given ("1.5" isn't an
/// integer, though the column holds 1). A save forgets it, as in Rails.
/// A typed write in generated code leaves it, and it counts only while
/// the attribute still holds its cast, so writing that same cast back
/// is the one case where Rails would read the new value instead.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct BeforeTypeCast(Vec<(String, Value, Value)>);

impl BeforeTypeCast {
    fn assign(&mut self, name: &str, given: Value, cast: Value) {
        self.0.retain(|(n, _, _)| n != name);
        self.0.push((name.to_string(), given, cast));
    }

    /// The value as given, if the attribute still holds `current`, its cast.
    pub(crate) fn given(&self, name: &str, current: &Value) -> Option<&Value> {
        self.0.iter().find(|(n, _, cast)| n == name && cast == current).map(|(_, given, _)| given)
    }

    pub(crate) fn forget(&mut self) {
        self.0.clear();
    }
}

/// Records compare by their columns alone.
impl PartialEq for BeforeTypeCast {
    fn eq(&self, _: &Self) -> bool {
        true
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
        $vis struct $name {
            $(pub $field: Option<$ty>,)*
            #[doc(hidden)]
            pub __before_type_cast: $crate::BeforeTypeCast,
        }

        impl $crate::Record for $name {
            const NAME: &'static str = stringify!($name);
            const TABLE: &'static str = $table;
            const COLUMNS: &'static [&'static str] = &[$(stringify!($field)),*];

            fn new_record() -> Self {
                Self { $($field: $crate::__model_default!($($default)?),)* __before_type_cast: Default::default() }
            }

            fn before_type_cast(&self) -> &$crate::BeforeTypeCast {
                &self.__before_type_cast
            }

            fn before_type_cast_mut(&mut self) -> &mut $crate::BeforeTypeCast {
                &mut self.__before_type_cast
            }

            fn get(&self, column: &str) -> $crate::Value {
                match column {
                    $(stringify!($field) => $crate::Value::from(self.$field.clone()),)*
                    _ => $crate::Value::Nil,
                }
            }

            fn cast_query(column: &str, value: $crate::Value) -> $crate::Value {
                match column {
                    $(stringify!($field) => <$ty as $crate::FromValue>::query(value),)*
                    _ => value,
                }
            }

            fn query_bound(column: &str, value: &$crate::Value) -> i8 {
                match column {
                    $(stringify!($field) => <$ty as $crate::FromValue>::bound(value),)*
                    _ => 0,
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
