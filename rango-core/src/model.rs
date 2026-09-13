//! Model trait and repository over the store.

use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    store::{Cells, Gather, Reader, Row, Slots, Store, StoreError, Value, Widget, Writer},
};

pub use crate::store::{
    Action, Check, Field, Filter, Key, Link, Mass, Name, Only, Op, Order, Page, Pick, Query, Rule,
    Run, Schema, Sort, Table, Tree,
};

/// A domain type mapped to a store table.
pub trait Model: Clone + Send + Sync + 'static {
    /// Table this model maps to.
    fn table() -> Table;
    /// Field definitions for this model's table.
    fn fields() -> Vec<Field>;
    /// Write all non-id values to the given writer.
    fn write(&self, w: &mut dyn Writer);
    /// Read a new value from the given reader.
    fn read(r: &mut dyn Reader) -> Result<Self, StoreError>;
    /// Write just the id values to the given writer.
    fn write_id(&self, w: &mut dyn Writer);
    /// Read just the id values from the given reader.
    fn read_id(&mut self, r: &mut dyn Reader) -> Result<(), StoreError>;

    /// Names of the plain cell columns.
    fn columns() -> Vec<Name> {
        Self::fields()
            .into_iter()
            .filter(|field| !field.id && !field.keyed && !field.many)
            .map(|field| field.name)
            .collect()
    }

    /// Names of the text columns, used for free-text search.
    fn search() -> Vec<Name> {
        Self::fields()
            .into_iter()
            .filter(|field| field.load() == Widget::Text)
            .map(|field| field.name)
            .collect()
    }

    /// Names of the columns the UI shows as read-only.
    fn readonly() -> Vec<Name> {
        Vec::new()
    }

    /// Actions the UI offers, defaulting to wipe.
    fn actions() -> Vec<Action> {
        vec![Action::wipe()]
    }

    /// Schema derived from this model's table and fields.
    fn schema() -> Schema {
        Schema {
            table: Self::table(),
            fields: Self::fields(),
            rules: Vec::new(),
        }
    }

    /// Full schema including any overriding rules.
    fn spec() -> Schema {
        Self::schema()
    }

    /// Cell values of this model in field order.
    fn row(&self) -> Vec<Value> {
        let mut slots = Slots::new();
        self.write(&mut slots);
        slots.values()
    }

    /// Key value of this model.
    fn id(&self) -> Value {
        let mut gather = Gather::new();
        self.write_id(&mut gather);
        gather.value()
    }
}

/// Name of the model's key column.
pub fn id_column<M: Model>() -> Name {
    M::schema().key()
}

/// Key value parsed from a raw string using the model's schema.
pub fn key<M: Model>(raw: &str) -> Value {
    Key::parse(raw, &M::schema()).value()
}

/// Rows linked to this model through a many-to-many `via` field.
pub async fn related<M: Model>(
    store: &Arc<dyn Store>,
    schemas: &[Schema],
    model: &M,
    field: Name,
) -> Result<Vec<Row>, StoreError> {
    let bad = |msg: &str| StoreError::Value(msg.into());
    let here = M::schema();
    let many = here
        .fields
        .iter()
        .find(|entry| entry.name == field)
        .ok_or_else(|| bad("unknown field"))?;
    if !many.many {
        return Err(StoreError::Unsupported("single field needs filter".into()));
    }
    let (through_table, mine, theirs) = match &many.link {
        Some(Link::Via(through, mine, theirs)) => (*through, *mine, *theirs),
        _ => return Err(bad("many field needs via")),
    };
    let through = schemas
        .iter()
        .find(|spec| spec.table == through_table)
        .ok_or_else(|| bad("unknown through"))?;
    let their = through
        .fields
        .iter()
        .find(|entry| entry.name == theirs)
        .ok_or_else(|| bad("unknown through column"))?;
    let (target_table, target_col) = match &their.link {
        Some(Link::To(table, name)) => (*table, *name),
        _ => return Err(bad("through column needs references")),
    };
    let target = schemas
        .iter()
        .find(|spec| spec.table == target_table)
        .ok_or_else(|| bad("unknown target"))?;
    store.define(through).await?;
    store.define(target).await?;
    let links = store
        .scan_query(
            through,
            &Query {
                tree: Tree::Leaf(Filter {
                    field: mine,
                    op: Op::Eq,
                    value: model.id(),
                }),
                sort: Vec::new(),
                page: Page::all(),
                only: Only::Some(vec![theirs]),
                mass: None,
            },
        )
        .await?;
    let mut ids = Vec::new();
    for row in &links {
        if let Some(id) = row.get(0)
            && *id != Value::Null
            && !ids.contains(id)
        {
            ids.push(id.clone());
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let leaves = ids
        .into_iter()
        .map(|id| {
            Tree::Leaf(Filter {
                field: target_col,
                op: Op::Eq,
                value: id,
            })
        })
        .collect();
    store
        .scan_query(
            target,
            &Query {
                tree: Tree::Or(leaves),
                sort: vec![Sort {
                    field: target.key(),
                    order: Order::Asc,
                }],
                page: Page::all(),
                only: Only::All,
                mass: None,
            },
        )
        .await
}

/// Full CRUD access to one model over a store; usable as an axum extractor.
pub struct Repository<M = ()> {
    store: Arc<dyn Store>,
    marker: PhantomData<M>,
}

fn pairs<M: Model>(model: &M) -> Vec<(Name, Value)> {
    let mut values = model.row().into_iter();
    M::fields()
        .into_iter()
        .filter(|field| !field.id && !field.many)
        .map(|field| {
            let value = values.next().unwrap_or(Value::Null);
            let value = match value {
                Value::Null => field.initial(),
                other => other,
            };
            (field.name, value)
        })
        .collect()
}

impl<M: Model> Repository<M> {
    /// Repository over the given store.
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            marker: PhantomData,
        }
    }

    async fn ensure(&self) -> Result<(), StoreError> {
        self.store.define(&M::schema()).await
    }

    /// Insert the model and store its generated key.
    pub async fn save(&self, model: &mut M) -> Result<(), StoreError> {
        self.save_many(std::slice::from_mut(model)).await
    }

    /// Insert a batch of models in one call.
    pub async fn save_many(&self, models: &mut [M]) -> Result<(), StoreError> {
        if models.is_empty() {
            return Ok(());
        }
        self.ensure().await?;
        let batch = models.iter().map(pairs).collect::<Vec<_>>();
        let keys = self.store.create(&M::schema(), &batch).await?;
        for (model, key) in models.iter_mut().zip(keys) {
            let value = key.value();
            model.read_id(&mut Cells::new(std::slice::from_ref(&value)))?;
        }
        Ok(())
    }

    /// Load one model by key value.
    pub async fn get(&self, id: &Value) -> Result<Option<M>, StoreError> {
        let schema = M::schema();
        let mut rows = self
            .scan_query(&Query {
                tree: Tree::Leaf(Filter {
                    field: schema.key(),
                    op: Op::Eq,
                    value: id.clone(),
                }),
                sort: vec![Sort {
                    field: schema.key(),
                    order: Order::Asc,
                }],
                page: Page::all(),
                only: Only::Lone,
                mass: None,
            })
            .await?;
        Ok(rows.pop())
    }

    /// All models ordered by key.
    pub async fn all(&self) -> Result<Vec<M>, StoreError> {
        let schema = M::schema();
        self.scan_query(&Query {
            tree: Tree::And(Vec::new()),
            sort: vec![Sort {
                field: schema.key(),
                order: Order::Asc,
            }],
            page: Page::all(),
            only: Only::All,
            mass: None,
        })
        .await
    }

    /// Models whose given field equals the value.
    pub async fn filter(&self, field: Name, value: &Value) -> Result<Vec<M>, StoreError> {
        let schema = M::schema();
        self.scan_query(&Query {
            tree: Tree::Leaf(Filter {
                field,
                op: Op::Eq,
                value: value.clone(),
            }),
            sort: vec![Sort {
                field: schema.key(),
                order: Order::Asc,
            }],
            page: Page::all(),
            only: Only::All,
            mass: None,
        })
        .await
    }

    /// All models with the given sort applied.
    pub async fn ordered(&self, sort: Sort) -> Result<Vec<M>, StoreError> {
        self.scan_query(&Query {
            tree: Tree::And(Vec::new()),
            sort: vec![sort],
            page: Page::all(),
            only: Only::All,
            mass: None,
        })
        .await
    }

    /// Models matched by the query, refusing projected rows.
    pub async fn scan_query(&self, query: &Query) -> Result<Vec<M>, StoreError> {
        if matches!(query.only, Only::Some(_)) {
            return Err(StoreError::Unsupported("projected rows need rows()".into()));
        }
        let rows = self.rows(query).await?;
        rows.iter().map(|row| M::read(&mut row.cells())).collect()
    }

    /// Raw rows matched by the query.
    pub async fn rows(&self, query: &Query) -> Result<Vec<Row>, StoreError> {
        self.ensure().await?;
        self.store.scan_query(&M::schema(), query).await
    }

    /// Count of rows matched by the query.
    pub async fn total_query(&self, query: &Query) -> Result<usize, StoreError> {
        self.ensure().await?;
        self.store.total_query(&M::schema(), query).await
    }

    /// Aggregate value computed by the query's mass action.
    pub async fn mass(&self, query: &Query) -> Result<Value, StoreError> {
        self.ensure().await?;
        self.store.mass(&M::schema(), query).await
    }

    /// Replace the row for this model's key.
    pub async fn update(&self, model: &M) -> Result<(), StoreError> {
        self.ensure().await?;
        self.store
            .replace(&M::schema(), &Key::of(&model.id())?, &pairs(model))
            .await
    }

    /// Delete the row with the given key value.
    pub async fn delete(&self, id: &Value) -> Result<(), StoreError> {
        self.ensure().await?;
        self.store.remove(&M::schema(), &Key::of(id)?).await
    }
}

impl<M: Model> FromRequestParts<()> for Repository<M> {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, _state: &()) -> Result<Self, Self::Rejection> {
        let store = parts
            .extensions
            .get::<Arc<dyn Store>>()
            .cloned()
            .ok_or_else(|| Error::Server("no store configured".into()))?;
        Ok(Self::new(store))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Storable;

    #[derive(Clone)]
    struct Post {
        id: i64,
        title: String,
    }

    impl Model for Post {
        fn table() -> Table {
            Table("posts")
        }

        fn fields() -> Vec<Field> {
            vec![Field::id(), Field::str("title")]
        }

        fn write(&self, w: &mut dyn Writer) {
            Storable::put(&self.title, w);
        }

        fn read(r: &mut dyn Reader) -> Result<Self, StoreError> {
            Ok(Self {
                id: Storable::take(r)?,
                title: Storable::take(r)?,
            })
        }

        fn write_id(&self, w: &mut dyn Writer) {
            Storable::put(&self.id, w);
        }

        fn read_id(&mut self, r: &mut dyn Reader) -> Result<(), StoreError> {
            self.id = Storable::take(r)?;
            Ok(())
        }
    }

    #[derive(Clone)]
    struct Product {
        sku: String,
        price: rust_decimal::Decimal,
    }

    impl Model for Product {
        fn table() -> Table {
            Table("products")
        }

        fn fields() -> Vec<Field> {
            vec![
                Field::key::<String>("sku"),
                Field::cell::<rust_decimal::Decimal>("price"),
            ]
        }

        fn write(&self, w: &mut dyn Writer) {
            Storable::put(&self.sku, w);
            Storable::put(&self.price, w);
        }

        fn read(r: &mut dyn Reader) -> Result<Self, StoreError> {
            Ok(Self {
                sku: Storable::take(r)?,
                price: Storable::take(r)?,
            })
        }

        fn write_id(&self, w: &mut dyn Writer) {
            Storable::put(&self.sku, w);
        }

        fn read_id(&mut self, r: &mut dyn Reader) -> Result<(), StoreError> {
            self.sku = Storable::take(r)?;
            Ok(())
        }
    }

    #[test]
    fn spec() {
        let spec = Post::spec();
        assert_eq!(spec.table, Table("posts"));
        assert_eq!(spec.rules, Vec::new());
        assert_eq!(Post::spec().key(), Name("id"));
        assert_eq!(Product::spec().key(), Name("sku"));
        assert_eq!(Post::columns(), vec![Name("title")]);
        assert_eq!(Post::search(), vec![Name("title")]);
    }

    #[test]
    fn sorts() {
        let schema = Post::spec();
        assert_eq!(
            Sort::parse("title", &schema),
            Sort {
                field: Name("title"),
                order: Order::Asc,
            }
        );
        assert_eq!(
            Sort::parse("-title", &schema),
            Sort {
                field: Name("title"),
                order: Order::Desc,
            }
        );
        assert_eq!(
            Sort::parse("junk", &schema),
            Sort {
                field: Name("id"),
                order: Order::Asc,
            }
        );
    }

    #[test]
    fn keys() {
        assert_eq!(Key::parse("7", &Post::spec()).value(), Value::int(7));
        assert_eq!(Key::parse("7", &Product::spec()).value(), Value::str("7"));
        assert_eq!(Key::parse("x", &Post::spec()).value(), Value::str("x"));
    }

    #[tokio::test]
    async fn gets() {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-gets.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = crate::store::sqlite::open(&path).await.unwrap();
        let repo = Repository::<Post>::new(store);
        let mut post = Post {
            id: 0,
            title: "hello".into(),
        };
        repo.save(&mut post).await.unwrap();
        let found = repo.get(&Value::int(post.id)).await.unwrap().unwrap();
        assert_eq!(found.title, "hello");
        assert!(repo.get(&Value::int(999)).await.unwrap().is_none());
        let projected = repo
            .scan_query(&Query {
                tree: Tree::And(Vec::new()),
                sort: Vec::new(),
                page: Page::all(),
                only: Only::Some(vec![Name("title")]),
                mass: None,
            })
            .await;
        assert!(matches!(projected, Err(StoreError::Unsupported(_))));
    }
}
