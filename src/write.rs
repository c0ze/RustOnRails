use crate::pg::{self, quote};
use crate::{Ctx, Model, Record, Result, Time, Value};

/// Sets `created_at` / `updated_at` to `time` where they're nil, as Rails
/// does when creating.
pub(crate) fn fill_timestamps<M: Record>(record: &mut M, time: Time) -> Result<()> {
    for column in ["created_at", "updated_at"] {
        if M::COLUMNS.contains(&column) && record.get(column).is_nil() {
            record.set(column, Value::Time(time))?;
        }
    }
    Ok(())
}

fn database_values<M: Model>(record: &M, columns: &[&str]) -> Result<Vec<Value>> {
    columns.iter().map(|c| database_value(record, c)).collect()
}

/// An integer attribute assigned a number Ruby would make a Bignum was
/// validated as given, like Rails; writing it fails, as Rails raises
/// ActiveModel::RangeError, rather than storing what the cast made of it.
fn database_value<M: Model>(record: &M, column: &str) -> Result<Value> {
    let value = record.get(column);
    if let Some(given) = record.before_type_cast().given(column, &value)
        && crate::records::bignum(&value, Some(given))
    {
        given.to_i()?;
    }
    M::behavior().to_database_for_write(column, value)
}

/// One INSERT of every column (id only when set); returns the new id.
pub(crate) fn insert_row<M: Model>(ctx: &mut Ctx, record: &M) -> Result<Value> {
    let columns: Vec<&str> = M::COLUMNS.iter().copied().filter(|c| *c != "id" || !record.get("id").is_nil()).collect();
    let params = database_values(record, &columns)?;
    let names: Vec<String> = columns.iter().map(|c| quote(c)).collect();
    let placeholders: Vec<String> = (1..=params.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
        quote(M::TABLE),
        names.join(", "),
        placeholders.join(", "),
        quote("id")
    );
    let rows = ctx.query(&sql, &params)?;
    pg::read(&rows[0], 0)
}

/// One UPDATE of the given columns for the row with this id.
pub(crate) fn update_row<M: Model>(ctx: &mut Ctx, id: i64, record: &M, columns: &[&str]) -> Result<()> {
    let mut params = database_values(record, columns)?;
    let sets: Vec<String> = columns.iter().enumerate().map(|(i, c)| format!("{} = ${}", quote(c), i + 1)).collect();
    params.push(Value::Int(id));
    let sql = format!("UPDATE {} SET {} WHERE {} = ${}", quote(M::TABLE), sets.join(", "), quote("id"), params.len());
    ctx.execute(&sql, &params)?;
    Ok(())
}

pub(crate) fn delete_row<M: Model>(ctx: &mut Ctx, id: i64) -> Result<()> {
    let sql = format!("DELETE FROM {} WHERE {} = $1", quote(M::TABLE), quote("id"));
    ctx.execute(&sql, &[Value::Int(id)])?;
    Ok(())
}

/// `update_all(column => nil)` for the rows whose `column` is `key`.
pub(crate) fn nullify_column<M: Model>(ctx: &mut Ctx, column: &str, key: Value) -> Result<()> {
    let name = quote(column);
    ctx.execute(&format!("UPDATE {} SET {name} = NULL WHERE {name} = $1", quote(M::TABLE)), &[key])?;
    Ok(())
}

/// `update_counters`: one atomic `column = COALESCE(column, 0) + by`.
pub(crate) fn increment_column<M: Model>(ctx: &mut Ctx, id: i64, column: &str, by: i64) -> Result<()> {
    let name = quote(column);
    let sql = format!("UPDATE {} SET {name} = COALESCE({name}, 0) + $1 WHERE {} = $2", quote(M::TABLE), quote("id"));
    ctx.execute(&sql, &[Value::Int(by), Value::Int(id)])?;
    Ok(())
}
