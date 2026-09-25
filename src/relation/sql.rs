//! The SQL a `Relation` runs.

use super::{Filter, Relation};
use crate::pg::quote;
use crate::{Model, Value};

impl<M: Model> Relation<M> {
    /// The SELECT this relation runs, with its parameters. It names the
    /// model's columns rather than `*`, so a column the model doesn't know
    /// (added by a later migration) doesn't break loading.
    pub fn to_sql(&self) -> (String, Vec<Value>) {
        let table = quote(M::TABLE);
        let columns: Vec<String> = M::COLUMNS.iter().map(|c| format!("{table}.{}", quote(c))).collect();
        let mut sql = format!("SELECT {} FROM {table}", columns.join(", "));
        let mut params = Vec::new();
        let mut conditions = 0;
        // Every JOIN comes before the WHERE, as Rails writes them.
        if let Some(join) = &self.join {
            let through = quote(join.table);
            sql.push_str(&format!(" INNER JOIN {through} ON {table}.{} = {through}.{}", quote("id"), quote(join.target_key)));
        }
        for join in &self.joins {
            let joined = quote(join.table);
            sql.push_str(&format!(
                " INNER JOIN {joined} ON {joined}.{} = {}.{}",
                quote(join.column),
                quote(join.other),
                quote(join.other_column)
            ));
        }
        if let Some(join) = &self.join {
            let through = quote(join.table);
            // An owner without an id (unsaved) has nothing through it, as Rails' `none`.
            if join.owner_id.is_nil() {
                sql.push_str(" WHERE 1=0");
            } else {
                params.push(join.owner_id.clone());
                sql.push_str(&format!(" WHERE {through}.{} = ${}", quote(join.owner_key), params.len()));
            }
            conditions += 1;
        }
        for filter in &self.filters {
            sql.push_str(if conditions == 0 { " WHERE " } else { " AND " });
            conditions += 1;
            let (column, op, value) = match filter {
                Filter::In(column, values) => {
                    let target = format!("{table}.{}", quote(column));
                    let cast: Vec<Value> = values
                        .iter()
                        .map(|v| M::behavior().to_database(column, M::cast_query(column, v.clone())))
                        .filter(|v| !v.is_nil())
                        .collect();
                    if cast.is_empty() {
                        sql.push_str("1=0");
                    } else {
                        let start = params.len();
                        params.extend(cast);
                        let marks: Vec<String> = (start + 1..=params.len()).map(|i| format!("${i}")).collect();
                        sql.push_str(&format!("{target} IN ({})", marks.join(", ")));
                    }
                    continue;
                }
                Filter::EqOn(joined, column, value) => {
                    let target = format!("{}.{}", quote(joined), quote(column));
                    if value.is_nil() {
                        sql.push_str(&format!("{target} IS NULL"));
                    } else {
                        params.push(value.clone());
                        sql.push_str(&format!("{target} = ${}", params.len()));
                    }
                    continue;
                }
                Filter::Sql(fragment, binds) => {
                    let mut parts = fragment.split('?');
                    sql.push('(');
                    sql.push_str(parts.next().unwrap_or_default());
                    for (part, bind) in parts.zip(binds) {
                        sql.push_str(&literal(bind));
                        sql.push_str(part);
                    }
                    sql.push(')');
                    continue;
                }
                Filter::Eq(c, v) => (c, "=", v),
                Filter::NotEq(c, v) => (c, "<>", v),
                Filter::Gte(c, v) => (c, ">=", v),
            };
            let target = format!("{table}.{}", quote(column));
            match M::behavior().to_database(column, M::cast_query(column, value.clone())) {
                Value::Nil if op == "<>" => sql.push_str(&format!("{target} IS NOT NULL")),
                Value::Nil => sql.push_str(&format!("{target} IS NULL")),
                value => {
                    params.push(value);
                    sql.push_str(&format!("{target} {op} ${}", params.len()));
                }
            }
        }
        if !self.orders.is_empty() {
            let orders: Vec<String> = self.orders.iter().map(|(c, dir)| format!("{table}.{} {dir}", quote(c))).collect();
            sql.push_str(&format!(" ORDER BY {}", orders.join(", ")));
        }
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        if let Some(n) = self.offset {
            sql.push_str(&format!(" OFFSET {n}"));
        }
        (sql, params)
    }
}

/// A fragment's bind as Rails' `quote` writes it into the SQL, so Postgres
/// types it from where it stands, as it does Rails' SQL. Strings are E''
/// literals, so a backslash or a quote stays data whatever
/// standard_conforming_strings says; a negative number is parenthesized so
/// `x-?` can't become a `--` comment.
fn literal(value: &Value) -> String {
    match value {
        Value::Nil => "NULL".to_string(),
        Value::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_string(),
        Value::Int(i) if *i < 0 => format!("({i})"),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_nan() => "'NaN'".to_string(),
        Value::Float(f) if f.is_infinite() => (if *f > 0.0 { "'Infinity'" } else { "'-Infinity'" }).to_string(),
        Value::Float(f) if f.is_sign_negative() => format!("({f:?})"),
        Value::Float(f) => format!("{f:?}"),
        Value::Str(s) => format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''")),
        Value::Time(t) => format!("'{}'", t.format("%Y-%m-%d %H:%M:%S%.6f")),
    }
}

/// Rails' `sanitize_sql_like` with its default escape character: each
/// backslash doubled, then one before each `%` and `_`, so the string
/// matches itself inside a LIKE pattern.
pub fn sanitize_sql_like(string: &str) -> String {
    let mut out = String::with_capacity(string.len());
    for c in string.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}
