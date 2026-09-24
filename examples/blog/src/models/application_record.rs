use rustonrails::{Model, Relation, Time};

// application_record.rb:4  scope :created_since, ->(time) { where(created_at: time..) }
pub trait ApplicationRecordScopes {
    fn created_since(self, time: Time) -> Self;
}

impl<M: Model> ApplicationRecordScopes for Relation<M> {
    fn created_since(self, time: Time) -> Self {
        self.where_gte("created_at", time)
    }
}
