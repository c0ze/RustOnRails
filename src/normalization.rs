//! `normalizes`: Active Model's `NormalizedValueType`, which normalizes a
//! value after the attribute's type casts it, both when it's assigned and
//! when it's queried. Nil is left alone (no `apply_to_nil`).

use crate::{Behavior, Normalizer, Value};

impl<M> Behavior<M> {
    /// `normalizes :email, with: ->(email) { email.strip.downcase }` on a
    /// String attribute.
    pub fn normalizes(mut self, attribute: &'static str, normalizer: Normalizer) -> Self {
        self.normalizers.push((attribute, normalizer));
        self
    }

    /// A cast value, normalized if its attribute declares a normalizer.
    pub(crate) fn normalize(&self, attribute: &str, value: Value) -> Value {
        let normalizer = self.normalizers.iter().find(|(a, _)| *a == attribute).map(|(_, f)| f);
        match (normalizer, value) {
            (Some(normalize), Value::Str(s)) => Value::Str(normalize(s)),
            (_, value) => value,
        }
    }

    pub(crate) fn normalizes_attribute(&self, attribute: &str) -> bool {
        self.normalizers.iter().any(|(a, _)| *a == attribute)
    }

    /// What `where` binds for a column: cast by its type, normalized, and
    /// an enum label as its integer.
    pub(crate) fn query_value(&self, attribute: &str, cast: Value) -> Value {
        self.to_database(attribute, self.normalize(attribute, cast))
    }
}
