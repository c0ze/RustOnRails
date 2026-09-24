use std::marker::PhantomData;

use crate::{Ctx, Handle, Model, Result, Value};

/// Loads an association for many owners at once, for `includes`.
pub trait Preload<M>: Sync {
    fn preload(&self, ctx: &mut Ctx, owners: &[Handle<M>]) -> Result<()>;
}

/// `belongs_to :user`, declared as a constant on the owner model:
/// `pub const USER: BelongsTo<Post, User> = BelongsTo::new("user", "user_id");`
pub struct BelongsTo<M, T> {
    pub name: &'static str,
    pub foreign_key: &'static str,
    marker: PhantomData<fn() -> (M, T)>,
}

impl<M, T> BelongsTo<M, T> {
    pub const fn new(name: &'static str, foreign_key: &'static str) -> Self {
        Self { name, foreign_key, marker: PhantomData }
    }
}

impl<M: Model, T: Model> BelongsTo<M, T> {
    /// `comment.post`: the cached target if the foreign key hasn't changed
    /// since it was loaded, otherwise one query.
    pub fn get(&self, ctx: &mut Ctx, owner: Handle<M>) -> Result<Option<Handle<T>>> {
        let key = ctx[owner].get(self.foreign_key);
        if key.is_nil() {
            return Ok(None);
        }
        if let Some(index) = ctx.cached(owner, self.name, &key) {
            return Ok(Some(Handle::from_index(index)));
        }
        let target = T::find_by(ctx, "id", key.clone())?;
        if let Some(target) = target {
            ctx.cache(owner, self.name, key, target.index());
        }
        Ok(target)
    }

    /// `comment.post = post`: sets the foreign key and the cache.
    pub fn set(&self, ctx: &mut Ctx, owner: Handle<M>, target: Handle<T>) -> Result<()> {
        let key = ctx[target].get("id");
        ctx[owner].set(self.foreign_key, key.clone())?;
        ctx.cache(owner, self.name, key, target.index());
        Ok(())
    }
}

impl<M: Model, T: Model> Preload<M> for BelongsTo<M, T> {
    /// `includes(:user)`: one `WHERE id IN (...)` for every owner; owners
    /// that share a target share one loaded record, as in Rails.
    fn preload(&self, ctx: &mut Ctx, owners: &[Handle<M>]) -> Result<()> {
        let mut keys: Vec<Value> = Vec::new();
        for owner in owners {
            let key = ctx[*owner].get(self.foreign_key);
            if !key.is_nil() && !keys.contains(&key) {
                keys.push(key);
            }
        }
        if keys.is_empty() {
            return Ok(());
        }
        let targets = T::all().where_in("id", keys).load(ctx)?;
        for owner in owners {
            let key = ctx[*owner].get(self.foreign_key);
            if let Some(target) = targets.iter().find(|t| ctx[**t].get("id") == key) {
                let index = target.index();
                ctx.cache(*owner, self.name, key, index);
            }
        }
        Ok(())
    }
}
