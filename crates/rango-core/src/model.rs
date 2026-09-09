use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    store::{Column, ColumnKind, Row, Store, StoreError, Value},
};

#[derive(Clone, PartialEq)]
pub enum Type {
    Id,
    Str,
    Int,
    Float,
    Bool,
    DateTime,
    Optional(Box<Type>),
}

impl Type {
    pub fn optional(self) -> Type {
        Type::Optional(Box::new(self))
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, &Type::Optional(_))
    }

    pub fn flat(&self) -> &Type {
        match self {
            Type::Optional(inner) => inner.flat(),
            kind => kind,
        }
    }
}

#[derive(Clone)]
pub struct Field {
    pub name: &'static str,
    pub kind: Type,
    pub unique: bool,
    pub default: Option<Value>,
}

impl Field {
    pub fn new(name: &'static str, kind: Type) -> Self {
        Self {
            name,
            kind,
            unique: false,
            default: None,
        }
    }

    pub fn id() -> Self {
        Self::new("id", Type::Id)
    }

    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub fn default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }
}

pub trait Model: Clone + Send + Sync + 'static {
    fn table() -> &'static str;
    fn fields() -> Vec<Field>;
    fn row(&self) -> Vec<Value>;
    fn from_row(row: &Row) -> Result<Self, StoreError>;
    fn set_id(&mut self, id: i64);
    fn id(&self) -> i64;

    fn ddl() -> String {
        Self::schema().ddl()
    }

    fn schema() -> Schema {
        Schema {
            table: Self::table(),
            fields: Self::fields(),
        }
    }
}

pub struct Schema {
    pub table: &'static str,
    pub fields: Vec<Field>,
}

impl Schema {
    pub fn ddl(&self) -> String {
        let columns: Vec<String> = self.fields.iter().map(column).collect();
        format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            self.table,
            columns.join(", ")
        )
    }

    pub fn alter(&self, have: &[Column]) -> Vec<String> {
        let moved = self.moved(have);
        let mut out = Vec::new();
        for field in &self.fields {
            if field.kind == Type::Id {
                continue;
            }
            if !have.iter().any(|col| col.name == field.name)
                && !moved.iter().any(|(_, name)| name == field.name)
            {
                out.push(format!(
                    "ALTER TABLE \"{}\" ADD COLUMN {}",
                    self.table,
                    column(field)
                ));
            }
        }
        out
    }

    pub fn drop(&self, have: &[Column]) -> Vec<String> {
        let moved = self.moved(have);
        let mut out = Vec::new();
        for col in have {
            if col.name == "id" {
                continue;
            }
            if !self.fields.iter().any(|field| field.name == col.name)
                && !moved.iter().any(|(name, _)| name == &col.name)
            {
                out.push(format!(
                    "ALTER TABLE \"{}\" DROP COLUMN \"{}\"",
                    self.table, col.name
                ));
            }
        }
        out
    }

    pub fn rename(&self, have: &[Column]) -> Vec<String> {
        self.moved(have)
            .into_iter()
            .map(|(old, name)| {
                format!(
                    "ALTER TABLE \"{}\" RENAME COLUMN \"{old}\" TO \"{name}\"",
                    self.table
                )
            })
            .collect()
    }

    fn moved(&self, have: &[Column]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for kind in [ColumnKind::Integer, ColumnKind::Real, ColumnKind::Text] {
            let gone: Vec<&String> = have
                .iter()
                .filter(|col| {
                    col.name != "id"
                        && col.kind == kind
                        && !self.fields.iter().any(|field| field.name == col.name)
                })
                .map(|col| &col.name)
                .collect();
            let fresh: Vec<&Field> = self
                .fields
                .iter()
                .filter(|field| {
                    field.kind != Type::Id
                        && affinity(&field.kind) == kind
                        && !have.iter().any(|col| col.name == field.name)
                })
                .collect();
            if let ([old], [new]) = (gone.as_slice(), fresh.as_slice()) {
                out.push(((*old).clone(), new.name.to_string()));
            }
        }
        out
    }
}

fn affinity(kind: &Type) -> ColumnKind {
    match kind {
        Type::Id | Type::Int | Type::DateTime | Type::Bool => ColumnKind::Integer,
        Type::Float => ColumnKind::Real,
        Type::Str => ColumnKind::Text,
        Type::Optional(inner) => affinity(inner),
    }
}

pub(crate) fn kinds<M: Model>() -> Vec<ColumnKind> {
    M::fields()
        .iter()
        .map(|field| affinity(&field.kind))
        .collect()
}

fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Str(value) => format!("'{}'", value.replace('\'', "''")),
        Value::Bool(value) => {
            if *value {
                "1".into()
            } else {
                "0".into()
            }
        }
        Value::DateTime(at) => at.timestamp().to_string(),
    }
}

fn sql(kind: &Type) -> &'static str {
    match kind {
        Type::Id => "INTEGER PRIMARY KEY AUTOINCREMENT",
        Type::Str => "TEXT",
        Type::Int | Type::DateTime => "INTEGER",
        Type::Float => "REAL",
        Type::Bool => "INTEGER",
        Type::Optional(inner) => sql(inner),
    }
}

fn column(field: &Field) -> String {
    let mut base = sql(&field.kind).to_string();
    if field.kind != Type::Id {
        if field.unique {
            base.push_str(" UNIQUE");
        }
        if !field.kind.is_optional() {
            base.push_str(" NOT NULL");
        }
        if let Some(default) = &field.default {
            base.push_str(&format!(" DEFAULT {}", literal(default)));
        }
    }
    format!("\"{}\" {}", field.name, base)
}

pub struct Repository<M = ()> {
    store: Arc<dyn Store>,
    marker: PhantomData<M>,
}

fn pairs<M: Model>(model: &M) -> Vec<(&'static str, Value)> {
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
        self.ensure().await?;
        let pairs = pairs(model);
        let columns: Vec<&str> = pairs.iter().map(|pair| pair.0).collect();
        let params: Vec<Value> = pairs.into_iter().map(|pair| pair.1).collect();
        let holes = vec!["?"; params.len()].join(", ");
        let sql = format!(
            "INSERT INTO \"{}\" ({}) VALUES ({holes})",
            M::table(),
            columns.join(", ")
        );
        self.store.execute(&sql, &params).await?;
        let id = self.store.last_id(M::table()).await?;
        model.set_id(id);
        Ok(())
    }

    pub async fn get(&self, id: i64) -> Result<Option<M>, StoreError> {
        self.ensure().await?;
        let sql = format!("SELECT * FROM \"{}\" WHERE id = ?", M::table());
        let rows = self
            .store
            .fetch(&sql, &[Value::int(id)], &kinds::<M>())
            .await?;
        rows.into_iter()
            .next()
            .map(|row| M::from_row(&row))
            .transpose()
    }

    pub async fn all(&self) -> Result<Vec<M>, StoreError> {
        self.ensure().await?;
        let sql = format!("SELECT * FROM \"{}\" ORDER BY id", M::table());
        let rows = self.store.fetch(&sql, &[], &kinds::<M>()).await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn update(&self, model: &M) -> Result<(), StoreError> {
        self.ensure().await?;
        let mut sets = Vec::new();
        let mut params = Vec::new();
        for (name, value) in pairs(model) {
            sets.push(format!("{name} = ?"));
            params.push(value);
        }
        params.push(Value::int(model.id()));
        let sql = format!(
            "UPDATE \"{}\" SET {} WHERE id = ?",
            M::table(),
            sets.join(", ")
        );
        self.store.execute(&sql, &params).await?;
        Ok(())
    }

    pub async fn delete(&self, id: i64) -> Result<(), StoreError> {
        self.ensure().await?;
        let sql = format!("DELETE FROM \"{}\" WHERE id = ?", M::table());
        self.store
            .execute(&sql, &[Value::int(id)])
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
    use crate::store::Column;

    #[derive(Clone)]
    struct Post {
        id: i64,
        title: String,
    }

    impl Model for Post {
        fn table() -> &'static str {
            "posts"
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

        fn set_id(&mut self, id: i64) {
            self.id = id;
        }

        fn id(&self) -> i64 {
            self.id
        }
    }

    fn col(name: &str, kind: ColumnKind) -> Column {
        Column {
            name: name.to_string(),
            kind,
        }
    }

    #[test]
    fn ddl() {
        assert_eq!(
            Post::schema().ddl(),
            "CREATE TABLE IF NOT EXISTS \"posts\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"title\" TEXT NOT NULL)"
        );
    }

    #[test]
    fn adds() {
        let schema = Post::schema();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
        ];
        assert!(schema.alter(&have).is_empty());
        let missing = vec![col("id", ColumnKind::Integer)];
        assert_eq!(schema.alter(&missing).len(), 1);
    }

    #[test]
    fn drops() {
        let schema = Post::schema();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
            col("junk", ColumnKind::Text),
        ];
        let drop = schema.drop(&have);
        assert_eq!(drop.len(), 1);
        assert!(schema.drop(&have[..2]).is_empty());
    }

    #[test]
    fn renames() {
        let schema = Post::schema();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
        ];
        let rename = schema.rename(&have);
        assert_eq!(rename.len(), 1);
        assert!(schema.alter(&have).is_empty());
        assert!(schema.drop(&have).is_empty());
        let mixed = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
            col("age", ColumnKind::Integer),
        ];
        assert_eq!(schema.rename(&mixed).len(), 1);
        assert!(schema.alter(&mixed).is_empty());
        assert_eq!(schema.drop(&mixed).len(), 1);
    }

    #[test]
    fn affinities() {
        assert_eq!(kinds::<Post>(), vec![ColumnKind::Integer, ColumnKind::Text]);
    }
}
