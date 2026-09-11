use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    store::{ColumnKind, Row, Store, StoreError, Value},
};

pub use crate::store::{
    Check, Field, Filter, Key, Link, Mass, Name, Only, Op, Order, Page, Pick, Query, Rule, Schema,
    Sort, Table, Tree, Type,
};

pub trait Model: Clone + Send + Sync + 'static {
    fn table() -> Table;
    fn fields() -> Vec<Field>;
    fn row(&self) -> Vec<Value>;
    fn from_row(row: &Row) -> Result<Self, StoreError>;
    fn set_id(&mut self, id: Value);
    fn id(&self) -> Value;

    fn columns() -> Vec<Name> {
        Self::fields()
            .iter()
            .filter(|f| !matches!(f.kind.flat(), Type::Id | Type::Key))
            .map(|f| f.name)
            .collect()
    }

    fn search() -> Vec<Name> {
        Self::fields()
            .iter()
            .filter(|f| matches!(f.kind.flat(), Type::Str))
            .map(|f| f.name)
            .collect()
    }

    fn readonly() -> Vec<Name> {
        Vec::new()
    }

    fn ddl() -> String {
        Self::schema().ddl()
    }

    fn schema() -> Schema {
        Schema {
            table: Self::table(),
            fields: Self::fields(),
            rules: Vec::new(),
        }
    }

    fn spec() -> Schema {
        Self::schema()
    }
}

fn kinds<M: Model>() -> Vec<ColumnKind> {
    M::schema().kinds()
}

pub fn id_column<M: Model>() -> Name {
    M::schema().key()
}

pub fn key<M: Model>(raw: &str) -> Value {
    Key::parse(raw, &M::schema()).value()
}

pub struct Repository<M = ()> {
    store: Arc<dyn Store>,
    marker: PhantomData<M>,
}

fn pairs<M: Model>(model: &M) -> Vec<(Name, Value)> {
    let mut values = model.row().into_iter();
    M::fields()
        .into_iter()
        .filter(|field| field.kind != Type::Id)
        .map(|field| {
            let value = values.next().unwrap_or(Value::Null);
            let value = match field.default {
                Some(ref default) if value == Value::Null => default.clone(),
                _ => value,
            };
            (field.name, value)
        })
        .collect()
}

impl<M: Model> Repository<M> {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            marker: PhantomData,
        }
    }

    async fn ensure(&self) -> Result<(), StoreError> {
        self.store.execute(&M::ddl(), &[]).await.map(|_| ())
    }

    pub async fn save(&self, model: &mut M) -> Result<(), StoreError> {
        self.save_many(std::slice::from_mut(model)).await
    }

    pub async fn save_many(&self, models: &mut [M]) -> Result<(), StoreError> {
        if models.is_empty() {
            return Ok(());
        }
        self.ensure().await?;
        let keyed = M::fields()
            .iter()
            .any(|field| matches!(field.kind.flat(), Type::Key));
        if keyed {
            let mut columns = Vec::new();
            let mut params = Vec::new();
            let mut groups = Vec::new();
            for model in models.iter() {
                let found = pairs(model);
                if columns.is_empty() {
                    columns = found.iter().map(|pair| format!("\"{}\"", pair.0)).collect();
                }
                groups.push(format!("({})", vec!["?"; found.len()].join(", ")));
                params.extend(found.into_iter().map(|pair| pair.1));
            }
            let sql = format!(
                "INSERT INTO \"{}\" ({}) VALUES {}",
                M::table(),
                columns.join(", "),
                groups.join(", ")
            );
            self.store.execute(&sql, &params).await?;
            return Ok(());
        }
        for model in models.iter_mut() {
            let found = pairs(model);
            let columns = found
                .iter()
                .map(|pair| pair.0.to_string())
                .collect::<Vec<_>>();
            let params = found.into_iter().map(|pair| pair.1).collect::<Vec<_>>();
            let id = self
                .store
                .insert(M::table().as_str(), &columns, &params)
                .await?;
            model.set_id(Value::int(id));
        }
        Ok(())
    }

    pub async fn get(&self, id: &Value) -> Result<Option<M>, StoreError> {
        self.ensure().await?;
        let sql = format!(
            "SELECT * FROM \"{}\" WHERE \"{}\" = ?",
            M::table(),
            id_column::<M>()
        );
        let rows = self
            .store
            .fetch(&sql, std::slice::from_ref(id), &kinds::<M>())
            .await?;
        rows.into_iter()
            .next()
            .map(|row| M::from_row(&row))
            .transpose()
    }

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

    pub async fn scan_query(&self, query: &Query) -> Result<Vec<M>, StoreError> {
        if !matches!(query.only, Only::All) {
            return Err(StoreError::Unsupported("projected rows need rows()".into()));
        }
        let rows = self.rows(query).await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn rows(&self, query: &Query) -> Result<Vec<Row>, StoreError> {
        self.ensure().await?;
        self.store.scan_query(&M::schema(), query).await
    }

    pub async fn total_query(&self, query: &Query) -> Result<usize, StoreError> {
        self.ensure().await?;
        self.store.total_query(&M::schema(), query).await
    }

    pub async fn update(&self, model: &M) -> Result<(), StoreError> {
        self.ensure().await?;
        let mut sets = Vec::new();
        let mut params = Vec::new();
        for (name, value) in pairs(model) {
            sets.push(format!("\"{name}\" = ?"));
            params.push(value);
        }
        params.push(model.id());
        let sql = format!(
            "UPDATE \"{}\" SET {} WHERE \"{}\" = ?",
            M::table(),
            sets.join(", "),
            id_column::<M>()
        );
        self.store.execute(&sql, &params).await?;
        Ok(())
    }

    pub async fn delete(&self, id: &Value) -> Result<(), StoreError> {
        self.ensure().await?;
        let sql = format!(
            "DELETE FROM \"{}\" WHERE \"{}\" = ?",
            M::table(),
            id_column::<M>()
        );
        self.store
            .execute(&sql, std::slice::from_ref(id))
            .await
            .map(|_| ())
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
            vec![Field::id(), Field::new("title", Type::Str)]
        }

        fn row(&self) -> Vec<Value> {
            vec![Value::str(&self.title)]
        }

        fn from_row(row: &Row) -> Result<Self, StoreError> {
            Ok(Self {
                id: row.int(0)?,
                title: row.str(1)?,
            })
        }

        fn set_id(&mut self, id: Value) {
            if let Value::Int(id) = id {
                self.id = id;
            }
        }

        fn id(&self) -> Value {
            Value::int(self.id)
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
            vec![Field::key("sku"), Field::new("price", Type::Decimal)]
        }

        fn row(&self) -> Vec<Value> {
            vec![Value::str(&self.sku), Value::decimal(self.price)]
        }

        fn from_row(row: &Row) -> Result<Self, StoreError> {
            Ok(Self {
                sku: row.str(0)?,
                price: row.decimal(1)?,
            })
        }

        fn set_id(&mut self, id: Value) {
            if let Value::Str(id) = id {
                self.sku = id;
            }
        }

        fn id(&self) -> Value {
            Value::str(&self.sku)
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
}
