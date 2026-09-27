use std::marker::PhantomData;

use crate::{Ctx, Handle, InnerJoin, Joinable, Model, Relation, Result, Value};

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

impl<M: Model, T: Model> Joinable for BelongsTo<M, T> {
    /// From a post, `joins(:user)`: `INNER JOIN users ON users.id = posts.user_id`.
    fn inner_join(&self) -> InnerJoin {
        InnerJoin { table: T::TABLE, column: "id", other: M::TABLE, other_column: self.foreign_key }
    }
}

/// `has_many :comments`, declared as a constant on the owner model.
/// `inverse` is the child's `belongs_to` back to the owner, which Rails
/// works out on its own for conventional names; its type ties it to this
/// owner, so a wrong inverse doesn't compile.
pub struct HasMany<M: 'static, T: 'static> {
    pub name: &'static str,
    pub foreign_key: &'static str,
    pub inverse: Option<&'static BelongsTo<T, M>>,
    marker: PhantomData<fn() -> (M, T)>,
}

impl<M, T> HasMany<M, T> {
    pub const fn new(name: &'static str, foreign_key: &'static str, inverse: Option<&'static BelongsTo<T, M>>) -> Self {
        Self { name, foreign_key, inverse, marker: PhantomData }
    }
}

impl<M: Model, T: Model> HasMany<M, T> {
    /// `post.comments`: a relation scoped to the owner. A new owner's is
    /// empty, as in Rails, rather than `post_id IS NULL`, which would find
    /// every orphan (and destroy them, with `dependent: :destroy`).
    pub fn of(&self, ctx: &Ctx, owner: Handle<M>) -> Relation<T> {
        if ctx.is_new_record(owner) {
            return T::all().none();
        }
        T::all().where_eq(self.foreign_key, ctx[owner].get("id"))
    }

    /// `post.comments.new(attributes)`: sets the foreign key and points the
    /// child's `belongs_to` at this very owner, so callbacks on either side
    /// see one record.
    pub fn build(&self, ctx: &mut Ctx, owner: Handle<M>, record: T) -> Result<Handle<T>> {
        let key = ctx[owner].get("id");
        let child = ctx.build(record);
        ctx[child].set(self.foreign_key, key.clone())?;
        if let Some(inverse) = self.inverse {
            ctx.cache(child, inverse.name, key, owner.index());
        }
        Ok(child)
    }

    /// `dependent: :destroy`: destroys each associated record through its
    /// own callbacks; a child that refuses stops the owner's destroy.
    pub fn destroy_all(&self, ctx: &mut Ctx, owner: Handle<M>) -> Result<()> {
        let children = self.of(ctx, owner).load(ctx)?;
        for child in children {
            ctx.destroy_bang(child)?;
        }
        Ok(())
    }

    /// `dependent: :nullify`: Rails' `update_all(foreign_key => nil)`, one
    /// UPDATE with no callbacks and no `updated_at`. A new owner's
    /// association is empty in Rails, whatever its id says.
    pub fn nullify_all(&self, ctx: &mut Ctx, owner: Handle<M>) -> Result<()> {
        if ctx.is_new_record(owner) {
            return Ok(());
        }
        let key = ctx[owner].get("id");
        crate::write::nullify_column::<T>(ctx, self.foreign_key, key)
    }
}

impl<M: Model, T: Model> Joinable for HasMany<M, T> {
    /// From a post, `joins(:comments)`: `INNER JOIN comments ON comments.post_id = posts.id`.
    fn inner_join(&self) -> InnerJoin {
        InnerJoin { table: T::TABLE, column: self.foreign_key, other: M::TABLE, other_column: "id" }
    }
}

/// `has_many :projects, through: :memberships`: the owner's rows in the
/// join table, and the targets they point at. Only the join-model shape:
/// through a has_many, sourced from a belongs_to on the join model.
pub struct HasManyThrough<M: 'static, T: 'static> {
    pub name: &'static str,
    pub join_table: &'static str,
    pub owner_key: &'static str,
    pub target_key: &'static str,
    marker: PhantomData<fn() -> (M, T)>,
}

impl<M: 'static, T: 'static> HasManyThrough<M, T> {
    pub const fn new(name: &'static str, join_table: &'static str, owner_key: &'static str, target_key: &'static str) -> Self {
        Self { name, join_table, owner_key, target_key, marker: PhantomData }
    }
}

impl<M: Model, T: Model> HasManyThrough<M, T> {
    /// `user.projects`: the same SQL Rails generates, an inner join with no
    /// DISTINCT, as a relation every scope and finder works on.
    pub fn of(&self, ctx: &Ctx, owner: Handle<M>) -> Relation<T> {
        let owner_id = if ctx.is_new_record(owner) { Value::Nil } else { ctx[owner].get("id") };
        Relation::new().join_through(self.join_table, self.target_key, self.owner_key, owner_id)
    }
}
