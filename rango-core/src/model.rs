//! Model trait and repository over the store.

use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    store::{Cells, Gather, Reader, Row, Slots, Store, StoreError, Value, Widget, Writer},
};

pub use crate::store::{
    Action, Choice, Field, Filter, Key, Link, Mass, Name, Only, Op, Order, Page, Policy, Query, Rule,
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
            .filter(|field| !field.many && field.load() == Widget::Text)
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
            rules: Self::rules(),
        }
    }

    /// Table-level constraints appended to the create statement.
    fn rules() -> Vec<Rule> {
        Vec::new()
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
    Ok(related_many(store, schemas, std::slice::from_ref(model), field)
        .await?
        .remove(&model.id())
        .unwrap_or_default())
}

/// Fetches the rows linked to every given model in one query per table.
pub async fn related_many<M: Model>(
    store: &Arc<dyn Store>,
    schemas: &[Schema],
    models: &[M],
    field: Name,
) -> Result<HashMap<Value, Vec<Row>>, StoreError> {
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
        .ok_or_else(|| bad("unknown link"))?;
    store.define(through).await?;
    store.define(target).await?;
    let ids = models
        .iter()
        .map(|model| model.id())
        .collect::<Vec<_>>();
    let mut picks: HashMap<Value, Vec<Value>> = HashMap::new();
    if !ids.is_empty() {
        let leaves = ids
            .into_iter()
            .map(|id| {
                Tree::Leaf(Filter {
                    field: mine,
                    op: Op::Eq,
                    value: id,
                })
            })
            .collect();
        let links = store
            .scan_query(
                through,
                &Query {
                    tree: Tree::Or(leaves),
                    sort: Vec::new(),
                    page: Page::all(),
                    only: Only::Some(vec![mine, theirs]),
                    mass: None,
                },
            )
            .await?;
        for row in &links {
            let (Some(mine), Some(theirs)) = (row.get(0), row.get(1)) else {
                continue;
            };
            if *theirs != Value::Null {
                picks.entry(mine.clone()).or_default().push(theirs.clone());
            }
        }
    }
    let mut want = Vec::new();
    for ids in picks.values() {
        for id in ids {
            if !want.contains(id) {
                want.push(id.clone());
            }
        }
    }
    let mut found: HashMap<Value, Vec<Row>> = HashMap::new();
    if !want.is_empty() {
        let leaves = want
            .into_iter()
            .map(|id| {
                Tree::Leaf(Filter {
                    field: target_col,
                    op: Op::Eq,
                    value: id,
                })
            })
            .collect();
        let index = target
            .fields
            .iter()
            .filter(|field| !field.many)
            .position(|field| field.name == target_col)
            .ok_or_else(|| bad("unknown column"))?;
        let rows = store
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
            .await?;
        for (mine, ids) in &picks {
            let keep = rows
                .iter()
                .filter(|row| ids.iter().any(|id| row.get(index) == Some(id)))
                .cloned()
                .collect::<Vec<_>>();
            if !keep.is_empty() {
                found.insert(mine.clone(), keep);
            }
        }
    }
    Ok(found)
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

    /// Rows of another model that reference this model by its key.
    pub async fn children<C: Model>(
        &self,
        id: &Value,
    ) -> Result<Vec<C>, StoreError> {
        let key = M::schema().key();
        let fields = C::fields();
        let name = fields
            .iter()
            .find(|field| {
                matches!(
                    field.link,
                    Some(Link::To(table, column))
                        if table == M::table() && column == key
                )
            })
            .map(|field| field.name)
            .ok_or_else(|| StoreError::Value("no child relation".into()))?;
        Repository::<C>::new(self.store.clone()).filter(name, id).await
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
            vec![
                Field::id(),
                Field::str("title"),
                Field::many("tags").link(Link::Via(
                    Table("pins"),
                    Name("post"),
                    Name("tag"),
                )),
            ]
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
    fn schema() {
        let schema = Post::schema();
        assert_eq!(schema.table, Table("posts"));
        assert_eq!(schema.rules, Vec::new());
        assert_eq!(Post::schema().key(), Name("id"));
        assert_eq!(Product::schema().key(), Name("sku"));
        assert_eq!(Post::columns(), vec![Name("title")]);
        assert_eq!(Post::search(), vec![Name("title")]);
    }

    #[test]
    fn sorts() {
        let schema = Post::schema();
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
        assert_eq!(Key::parse("7", &Post::schema()).value(), Value::int(7));
        assert_eq!(Key::parse("7", &Product::schema()).value(), Value::str("7"));
        assert_eq!(Key::parse("x", &Post::schema()).value(), Value::str("x"));
    }

    #[tokio::test]
    async fn gets() {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-gets.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = crate::store::Sqlite::open(&path).await.unwrap();
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

    #[derive(Clone)]
    struct Pin {
        id: i64,
        post: i64,
        tag: i64,
    }

    impl Model for Pin {
        fn table() -> Table {
            Table("pins")
        }

        fn fields() -> Vec<Field> {
            vec![
                Field::id(),
                Field::cell::<i64>("post").references("posts.id"),
                Field::cell::<i64>("tag").references("tags.id"),
            ]
        }

        fn write(&self, w: &mut dyn Writer) {
            Storable::put(&self.post, w);
            Storable::put(&self.tag, w);
        }

        fn read(r: &mut dyn Reader) -> Result<Self, StoreError> {
            Ok(Self {
                id: Storable::take(r)?,
                post: Storable::take(r)?,
                tag: Storable::take(r)?,
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

    #[tokio::test]
    async fn related_batches() {
        let path = std::env::temp_dir()
            .join(format!("rango-test-{}-related.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = crate::store::Sqlite::open(&path).await.unwrap();
        let tag_schema = Schema {
            table: Table("tags"),
            fields: vec![Field::id(), Field::str("name")],
            rules: Vec::new(),
        };
        store.define(&tag_schema).await.unwrap();
        store.define(&Post::schema()).await.unwrap();
        store.define(&Pin::schema()).await.unwrap();
        store
            .create(&tag_schema, &[vec![(Name("name"), Value::str("one"))]])
            .await
            .unwrap();
        store
            .create(&tag_schema, &[vec![(Name("name"), Value::str("two"))]])
            .await
            .unwrap();
        store
            .create(
                &Post::schema(),
                &[vec![(Name("title"), Value::str("one"))]],
            )
            .await
            .unwrap();
        let first = vec![vec![
            (Name("post"), Value::int(1)),
            (Name("tag"), Value::int(1)),
        ]];
        let second = vec![vec![
            (Name("post"), Value::int(1)),
            (Name("tag"), Value::int(2)),
        ]];
        store.create(&Pin::schema(), &first).await.unwrap();
        store.create(&Pin::schema(), &second).await.unwrap();
        let posts = vec![
            Post {
                id: 1,
                title: "one".into(),
            },
            Post {
                id: 2,
                title: "two".into(),
            },
        ];
        let out = related_many(
            &store,
            &[Post::schema(), Pin::schema(), tag_schema],
            &posts,
            Name("tags"),
        )
        .await
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out.get(&Value::int(1)).unwrap().len(), 2);
        assert!(out.contains_key(&Value::int(1)));
        assert!(!out.contains_key(&Value::int(2)));
    }
}
