use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    store::{Column, ColumnKind, Row, Store, StoreError, Value},
};

#[derive(Clone, PartialEq)]
pub enum Type {
    Id,
    Key,
    Str,
    Int,
    Float,
    Bool,
    DateTime,
    Decimal,
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
    pub references: Option<&'static str>,
}

impl Field {
    pub fn new(name: &'static str, kind: Type) -> Self {
        Self {
            name,
            kind,
            unique: false,
            default: None,
            references: None,
        }
    }

    pub fn id() -> Self {
        Self::new("id", Type::Id)
    }

    pub fn key(name: &'static str) -> Self {
        Self::new(name, Type::Key)
    }

    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub fn default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    pub fn references(mut self, table: &'static str) -> Self {
        self.references = Some(table);
        self
    }

    pub fn reference(&self) -> Option<(&'static str, &'static str)> {
        let target = self.references?;
        match target.split_once('.') {
            Some((table, column)) => Some((table, column)),
            None => Some((target, "id")),
        }
    }
}

pub trait Model: Clone + Send + Sync + 'static {
    fn table() -> &'static str;
    fn fields() -> Vec<Field>;
    fn row(&self) -> Vec<Value>;
    fn from_row(row: &Row) -> Result<Self, StoreError>;
    fn set_id(&mut self, id: Value);
    fn id(&self) -> Value;

    fn columns() -> Vec<&'static str> {
        Self::fields()
            .iter()
            .filter(|f| !matches!(f.kind.flat(), Type::Id | Type::Key))
            .map(|f| f.name)
            .collect()
    }

    fn search() -> Vec<&'static str> {
        Self::fields()
            .iter()
            .filter(|f| matches!(f.kind.flat(), Type::Str))
            .map(|f| f.name)
            .collect()
    }

    fn readonly() -> Vec<&'static str> {
        Vec::new()
    }

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
    pub fn key(&self) -> &'static str {
        self.fields
            .iter()
            .find(|field| matches!(field.kind.flat(), Type::Id | Type::Key))
            .map(|field| field.name)
            .unwrap_or("id")
    }

    pub fn kinds(&self) -> Vec<ColumnKind> {
        self.fields
            .iter()
            .map(|field| affinity(&field.kind))
            .collect()
    }

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
        Type::Str | Type::Key | Type::Decimal => ColumnKind::Text,
        Type::Optional(inner) => affinity(inner),
    }
}

pub(crate) fn kinds<M: Model>() -> Vec<ColumnKind> {
    M::schema().kinds()
}

pub fn id_column<M: Model>() -> &'static str {
    M::schema().key()
}

pub fn key<M: Model>(raw: &str) -> Value {
    let keyed = M::fields()
        .iter()
        .any(|field| matches!(field.kind.flat(), Type::Key) && field.name == id_column::<M>());
    if keyed {
        return Value::str(raw);
    }
    match raw.parse::<i64>() {
        Ok(id) => Value::int(id),
        Err(_) => Value::str(raw),
    }
}

fn order<M: Model>(sort: &str) -> String {
    let (name, down) = match sort.strip_prefix('-') {
        Some(name) => (name, true),
        None => (sort, false),
    };
    let known = M::fields().iter().any(|field| field.name == name);
    let name = if known { name } else { id_column::<M>() };
    if down {
        format!("\"{name}\" DESC")
    } else {
        format!("\"{name}\"")
    }
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
        Value::Decimal(value) => format!("'{value}'"),
    }
}

fn sql(kind: &Type) -> &'static str {
    match kind {
        Type::Id => "INTEGER PRIMARY KEY AUTOINCREMENT",
        Type::Key => "TEXT PRIMARY KEY",
        Type::Str => "TEXT",
        Type::Int | Type::DateTime => "INTEGER",
        Type::Float => "REAL",
        Type::Bool => "INTEGER",
        Type::Decimal => "TEXT",
        Type::Optional(inner) => sql(inner),
    }
}

fn column(field: &Field) -> String {
    let mut base = sql(&field.kind).to_string();
    if !matches!(field.kind.flat(), Type::Id | Type::Key) {
        if let Some((table, column)) = field.reference() {
            base.push_str(&format!(" REFERENCES \"{table}\"(\"{column}\")"));
        }
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
            let id = self.store.insert(M::table(), &columns, &params).await?;
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
        self.ensure().await?;
        let sql = format!(
            "SELECT * FROM \"{}\" ORDER BY \"{}\"",
            M::table(),
            id_column::<M>()
        );
        let rows = self.store.fetch(&sql, &[], &kinds::<M>()).await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn filter(&self, field: &str, value: &Value) -> Result<Vec<M>, StoreError> {
        self.ensure().await?;
        let sql = format!(
            "SELECT * FROM \"{}\" WHERE \"{field}\" = ? ORDER BY \"{}\"",
            M::table(),
            id_column::<M>()
        );
        let rows = self
            .store
            .fetch(&sql, std::slice::from_ref(value), &kinds::<M>())
            .await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn ordered(&self, field: &str, down: bool) -> Result<Vec<M>, StoreError> {
        self.ensure().await?;
        let sort = if down {
            format!("-{field}")
        } else {
            field.to_string()
        };
        self.scan("", &[], &sort, None).await
    }

    pub async fn scan(
        &self,
        cond: &str,
        params: &[Value],
        sort: &str,
        limit: Option<(usize, usize)>,
    ) -> Result<Vec<M>, StoreError> {
        self.ensure().await?;
        let mut sql = format!("SELECT * FROM \"{}\"", M::table());
        if !cond.is_empty() {
            sql.push_str(&format!(" WHERE {cond}"));
        }
        sql.push_str(&format!(" ORDER BY {}", order::<M>(sort)));
        if let Some((count, offset)) = limit {
            sql.push_str(&format!(" LIMIT {count} OFFSET {offset}"));
        }
        let rows = self.store.fetch(&sql, params, &kinds::<M>()).await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn total(&self, cond: &str, params: &[Value]) -> Result<usize, StoreError> {
        self.ensure().await?;
        let mut sql = format!("SELECT COUNT(*) FROM \"{}\"", M::table());
        if !cond.is_empty() {
            sql.push_str(&format!(" WHERE {cond}"));
        }
        let rows = self
            .store
            .fetch(&sql, params, &[ColumnKind::Integer])
            .await?;
        Ok(rows.first().and_then(|row| row.int(0).ok()).unwrap_or(0) as usize)
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

        fn set_id(&mut self, id: Value) {
            if let Value::Int(id) = id {
                self.id = id;
            }
        }

        fn id(&self) -> Value {
            Value::int(self.id)
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

    #[derive(Clone)]
    struct Product {
        sku: String,
        price: rust_decimal::Decimal,
    }

    impl Model for Product {
        fn table() -> &'static str {
            "products"
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
    fn key_ddl() {
        assert_eq!(
            Product::schema().ddl(),
            "CREATE TABLE IF NOT EXISTS \"products\" (\"sku\" TEXT PRIMARY KEY, \"price\" TEXT NOT NULL)"
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
