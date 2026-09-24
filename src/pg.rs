use std::error::Error as StdError;

use bytes::BytesMut;
use postgres::Row;
use postgres::types::{IsNull, ToSql, Type, to_sql_checked};

use crate::{Result, Time, Value};

/// Double-quotes an identifier. Identifiers come from generated code, never
/// from user input; values always go as parameters.
pub(crate) fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// Reads one column of a row as a `Value`, going by its Postgres type.
pub(crate) fn read(row: &Row, index: usize) -> Result<Value> {
    let ty = row.columns()[index].type_().clone();
    let value = if ty == Type::INT8 {
        row.try_get::<_, Option<i64>>(index)?.map(Value::Int)
    } else if ty == Type::INT4 {
        row.try_get::<_, Option<i32>>(index)?.map(Value::from)
    } else if ty == Type::INT2 {
        row.try_get::<_, Option<i16>>(index)?.map(|v| Value::Int(v.into()))
    } else if ty == Type::BOOL {
        row.try_get::<_, Option<bool>>(index)?.map(Value::Bool)
    } else if ty == Type::FLOAT8 {
        row.try_get::<_, Option<f64>>(index)?.map(Value::Float)
    } else if ty == Type::TIMESTAMP {
        row.try_get::<_, Option<Time>>(index)?.map(Value::Time)
    } else {
        row.try_get::<_, Option<String>>(index)?.map(Value::Str)
    };
    Ok(value.unwrap_or(Value::Nil))
}

impl ToSql for Value {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> std::result::Result<IsNull, Box<dyn StdError + Sync + Send>> {
        let fits = match self {
            Value::Nil => true,
            Value::Bool(_) => *ty == Type::BOOL,
            Value::Int(_) => [Type::INT2, Type::INT4, Type::INT8, Type::FLOAT8].contains(ty),
            Value::Float(_) => *ty == Type::FLOAT8,
            Value::Str(_) => <String as ToSql>::accepts(ty),
            Value::Time(_) => *ty == Type::TIMESTAMP,
        };
        if !fits {
            // Binding by variant alone would write text bytes into a binary
            // int8, which Postgres reads back as some other number.
            return Err(format!("can't bind {self:?} to a {ty} parameter").into());
        }
        match self {
            Value::Nil => Ok(IsNull::Yes),
            Value::Bool(b) => b.to_sql(ty, out),
            Value::Int(i) if *ty == Type::INT4 => i32::try_from(*i)?.to_sql(ty, out),
            Value::Int(i) if *ty == Type::INT2 => i16::try_from(*i)?.to_sql(ty, out),
            Value::Int(i) if *ty == Type::FLOAT8 => (*i as f64).to_sql(ty, out),
            Value::Int(i) => i.to_sql(ty, out),
            Value::Float(f) => f.to_sql(ty, out),
            Value::Str(s) => s.to_sql(ty, out),
            Value::Time(t) => t.to_sql(ty, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}
