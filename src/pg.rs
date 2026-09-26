use std::error::Error as StdError;

use bytes::BytesMut;
use postgres::Row;
use postgres::types::{FromSql, IsNull, ToSql, Type, to_sql_checked};

use crate::{Date, Error, Result, Time, Value};

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
    } else if ty == Type::DATE {
        row.try_get::<_, Option<Date>>(index)?.map(Value::Date)
    } else if ty == Type::NUMERIC {
        // `SUM` of a bigint column is a numeric. A whole number is an
        // Integer, as Rails casts it by the column's type; one Ruby would
        // make a Bignum fails, and a fraction stays its decimal text.
        match row.try_get::<_, Option<Numeric>>(index)? {
            Some(Numeric(text)) if text.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()) => {
                Some(Value::Int(text.parse().map_err(|_| Error::Overflow { value: text.clone() })?))
            }
            Some(Numeric(text)) => Some(Value::Str(text)),
            None => None,
        }
    } else {
        row.try_get::<_, Option<String>>(index)?.map(Value::Str)
    };
    Ok(value.unwrap_or(Value::Nil))
}

/// A `numeric` as its decimal text, read from Postgres' binary format:
/// base-10000 digits, the weight of the first, a sign and a display scale.
struct Numeric(String);

impl<'a> FromSql<'a> for Numeric {
    fn from_sql(_: &Type, raw: &'a [u8]) -> std::result::Result<Self, Box<dyn StdError + Sync + Send>> {
        let word = |i: usize| -> std::result::Result<u16, Box<dyn StdError + Sync + Send>> {
            let bytes = raw.get(2 * i..2 * i + 2).ok_or("a numeric shorter than its header says")?;
            Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
        };
        let (count, weight, sign, scale) = (word(0)? as usize, word(1)? as i16 as i64, word(2)?, word(3)? as usize);
        match sign {
            0xC000 => return Ok(Numeric("NaN".into())),
            0xD000 => return Ok(Numeric("Infinity".into())),
            0xF000 => return Ok(Numeric("-Infinity".into())),
            _ => {}
        }
        let digits = (0..count).map(|i| word(4 + i)).collect::<std::result::Result<Vec<_>, _>>()?;
        // Digit i carries weight `weight - i`, in base 10000.
        let digit = |w: i64| -> u16 { usize::try_from(weight - w).ok().and_then(|i| digits.get(i).copied()).unwrap_or(0) };
        let mut text = if sign == 0x4000 { "-".to_string() } else { String::new() };
        if weight < 0 {
            text.push('0');
        } else {
            text.push_str(&digit(weight).to_string());
            for w in (0..weight).rev() {
                text.push_str(&format!("{:04}", digit(w)));
            }
        }
        if scale > 0 {
            let mut fraction = String::new();
            let mut w = -1;
            while fraction.len() < scale {
                fraction.push_str(&format!("{:04}", digit(w)));
                w -= 1;
            }
            text.push('.');
            text.push_str(&fraction[..scale]);
        }
        Ok(Numeric(text))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
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
            Value::Date(_) => *ty == Type::DATE,
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
            Value::Date(d) => d.to_sql(ty, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}
