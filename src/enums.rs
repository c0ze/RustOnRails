use crate::{Behavior, Error, Result, Value};

/// `enum :status, { draft: 0, published: 1 }`: labels in memory, integers
/// in the database.
pub(crate) struct EnumDef {
    pub attribute: &'static str,
    pub mapping: Vec<(&'static str, i64)>,
}

impl<M> Behavior<M> {
    /// Enum labels become their integers and numeric strings pass through as
    /// integers; an unknown label becomes nil, as Rails' enum type
    /// serializes it.
    pub(crate) fn to_database(&self, attribute: &str, value: Value) -> Value {
        match (self.enum_for(attribute), &value) {
            (Some(def), Value::Str(label)) => match def.mapping.iter().find(|(l, _)| l == label) {
                Some((_, i)) => Value::Int(*i),
                None => label.trim().parse().map_or(Value::Nil, Value::Int),
            },
            _ => value,
        }
    }

    /// `to_database` for INSERT and UPDATE: anything but a label is an
    /// error, as Rails raises on assigning it rather than writing it. That
    /// includes an integer the enum doesn't map (assigned 99, held as
    /// "99") and a numeric string, which Rails' enum type doesn't read.
    pub(crate) fn to_database_for_write(&self, attribute: &str, value: Value) -> Result<Value> {
        match (self.enum_for(attribute), &value) {
            (Some(def), Value::Str(label)) => match def.mapping.iter().find(|(l, _)| l == label) {
                Some((_, i)) => Ok(Value::Int(*i)),
                None => Err(Error::InvalidEnum { attribute: def.attribute, value: label.clone() }),
            },
            _ => Ok(self.to_database(attribute, value)),
        }
    }

    /// Enum integers become their labels; an unknown integer becomes nil.
    pub(crate) fn from_database(&self, attribute: &str, value: Value) -> Value {
        match (self.enum_for(attribute), &value) {
            (Some(def), Value::Int(i)) => def.mapping.iter().find(|(_, n)| n == i).map_or(Value::Nil, |(l, _)| Value::from(*l)),
            _ => value,
        }
    }

    pub(crate) fn enum_for(&self, attribute: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.attribute == attribute)
    }

    /// Assigning to an enum takes a label or the label's integer
    /// (`status = 1` gives "published"); anything else stays as given, for
    /// the inclusion check or the write-time error.
    pub(crate) fn cast_assignment(&self, attribute: &str, value: Value) -> Value {
        match (self.enum_for(attribute), &value) {
            (Some(def), Value::Int(i)) => def
                .mapping
                .iter()
                .find(|(_, n)| n == i)
                .map_or_else(|| Value::Str(i.to_string()), |(label, _)| Value::from(*label)),
            _ => value,
        }
    }
}
