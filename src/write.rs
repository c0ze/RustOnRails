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

/// One INSERT of every column (id only when set); returns the new id.
pub(crate) fn insert_row<M: Model>(ctx: &mut Ctx, record: &M) -> Result<Value> {
    let columns: Vec<&str> = M::COLUMNS.iter().copied().filter(|c| *c != "id" || !record.get("id").is_nil()).collect();
    let params: Vec<Value> = columns.iter().map(|c| M::behavior().to_database(c, record.get(c))).collect();
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
    let mut params: Vec<Value> = columns.iter().map(|c| M::behavior().to_database(c, record.get(c))).collect();
    let sets: Vec<String> = columns.iter().enumerate().map(|(i, c)| format!("{} = ${}", quote(c), i + 1)).collect();
    params.push(Value::Int(id));
    let sql = format!("UPDATE {} SET {} WHERE {} = ${}", quote(M::TABLE), sets.join(", "), quote("id"), params.len());
    ctx.execute(&sql, &params)?;
    Ok(())
}
