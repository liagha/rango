use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    store::{Column, ColumnKind, Row, Store, StoreError, Value},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Table(pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Name(pub &'static str);

impl Table {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl Name {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Clone, PartialEq)]
pub enum Type {
    Id,
    Key,
    Str,
    Int,
    Float,
    Bool,
    Moment,
    Decimal,
    Ref,
    Many,
    Opt(Box<Type>),
}

impl Type {
    pub fn optional(self) -> Type {
        Type::Opt(Box::new(self))
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, &Type::Opt(_))
    }

    pub fn flat(&self) -> &Type {
        match self {
            Type::Opt(inner) => inner.flat(),
            kind => kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub options: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check(pub &'static str);

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    To(Table, Name),
    Via(Table, Name, Name),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    Same(Vec<Name>),
    Hold(Name),
    Said(Check),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    More,
    Less,
    At,
    In,
    Out,
    Like,
    Bare,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub field: Name,
    pub op: Op,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    Leaf(Filter),
    And(Vec<Tree>),
    Or(Vec<Tree>),
    Cut(Box<Tree>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub field: Name,
    pub order: Order,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub count: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Only {
    All,
    Some(Vec<Name>),
    Lone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mass {
    Count,
    Sum(Name),
    Mean(Name),
    Low(Name),
    High(Name),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub tree: Tree,
    pub sort: Vec<Sort>,
    pub page: Page,
    pub only: Only,
    pub mass: Option<Mass>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Int(i64),
    Text(String),
}

impl Key {
    pub fn parse(raw: &str, schema: &Schema) -> Self {
        let keyed = schema.fields.iter().any(|field| {
            matches!(field.kind.flat(), Type::Key) && field.name == schema.key()
        });
        if keyed {
            return Key::Text(raw.into());
        }
        match raw.parse::<i64>() {
            Ok(id) => Key::Int(id),
            Err(_) => Key::Text(raw.into()),
        }
    }

    pub fn value(&self) -> Value {
        match self {
            Key::Int(id) => Value::int(*id),
            Key::Text(text) => Value::str(text),
        }
    }
}

#[derive(Clone)]
pub struct Field {
    pub name: Name,
    pub kind: Type,
    pub unique: bool,
    pub index: bool,
    pub pick: Option<Pick>,
    pub default: Option<Value>,
    pub link: Option<Link>,
}

impl Field {
    pub fn new(name: &'static str, kind: Type) -> Self {
        Self {
            name: Name(name),
            kind,
            unique: false,
            index: false,
            pick: None,
            default: None,
            link: None,
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

    pub fn indexed(mut self) -> Self {
        self.index = true;
        self
    }

    pub fn default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    pub fn references(mut self, target: &'static str) -> Self {
        let (table, column) = match target.split_once('.') {
            Some((table, column)) => (table, column),
            None => (target, "id"),
        };
        self.link = Some(Link::To(Table(table), Name(column)));
        self
    }

    pub fn link(mut self, link: Link) -> Self {
        self.link = Some(link);
        self
    }

    pub fn reference(&self) -> Option<(Table, Name)> {
        match self.link {
            Some(Link::To(table, name)) => Some((table, name)),
            Some(Link::Via(..)) => todo!("phase 3"),
            None => None,
        }
    }
}

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

pub struct Schema {
    pub table: Table,
    pub fields: Vec<Field>,
    pub rules: Vec<Rule>,
}

impl Schema {
    pub fn key(&self) -> Name {
        self.fields
            .iter()
            .find(|field| matches!(field.kind.flat(), Type::Id | Type::Key))
            .map(|field| field.name)
            .unwrap_or(Name("id"))
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
            if !have.iter().any(|col| col.name == field.name.as_str())
                && !moved.iter().any(|(_, name)| name == field.name.as_str())
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
            if !self
                .fields
                .iter()
                .any(|field| field.name.as_str() == col.name.as_str())
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
                        && !self
                            .fields
                            .iter()
                            .any(|field| field.name.as_str() == col.name.as_str())
                })
                .map(|col| &col.name)
                .collect();
            let fresh: Vec<&Field> = self
                .fields
                .iter()
                .filter(|field| {
                    field.kind != Type::Id
                        && affinity(&field.kind) == kind
                        && !have.iter().any(|col| col.name == field.name.as_str())
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
        Type::Id | Type::Int | Type::Moment | Type::Bool => ColumnKind::Integer,
        Type::Float => ColumnKind::Real,
        Type::Str | Type::Key | Type::Decimal => ColumnKind::Text,
        Type::Ref => todo!("phase 2"),
        Type::Many => todo!("phase 3"),
        Type::Opt(inner) => affinity(inner),
    }
}

pub(crate) fn kinds<M: Model>() -> Vec<ColumnKind> {
    M::schema().kinds()
}

pub fn id_column<M: Model>() -> Name {
    M::schema().key()
}

impl Sort {
    pub fn parse(raw: &str, schema: &Schema) -> Self {
        let (name, order) = match raw.strip_prefix('-') {
            Some(name) => (name, Order::Desc),
            None => (raw, Order::Asc),
        };
        let known = schema
            .fields
            .iter()
            .any(|field| field.name.as_str() == name);
        if known {
            for field in &schema.fields {
                if field.name.as_str() == name {
                    return Sort {
                        field: field.name,
                        order,
                    };
                }
            }
        }
        Sort {
            field: schema.key(),
            order: Order::Asc,
        }
    }
}

pub fn key<M: Model>(raw: &str) -> Value {
    Key::parse(raw, &M::schema()).value()
}

fn order<M: Model>(sort: &str) -> String {
    let (name, down) = match sort.strip_prefix('-') {
        Some(name) => (name, true),
        None => (sort, false),
    };
    let known = M::fields()
        .iter()
        .any(|field| field.name.as_str() == name);
    let name = if known {
        name
    } else {
        id_column::<M>().as_str()
    };
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
        Type::Int | Type::Moment => "INTEGER",
        Type::Float => "REAL",
        Type::Bool => "INTEGER",
        Type::Decimal => "TEXT",
        Type::Ref => todo!("phase 2"),
        Type::Many => todo!("phase 3"),
        Type::Opt(inner) => sql(inner),
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
        self.ensure().await?;
        let sql = format!(
            "SELECT * FROM \"{}\" ORDER BY \"{}\"",
            M::table(),
            id_column::<M>()
        );
        let rows = self.store.fetch(&sql, &[], &kinds::<M>()).await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn filter(&self, field: Name, value: &Value) -> Result<Vec<M>, StoreError> {
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

    pub async fn ordered(&self, sort: Sort) -> Result<Vec<M>, StoreError> {
        self.ensure().await?;
        let text = match sort.order {
            Order::Asc => sort.field.to_string(),
            Order::Desc => format!("-{}", sort.field),
        };
        self.scan("", &[], &text, None).await
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
        assert_eq!(
            Key::parse("7", &Product::spec()).value(),
            Value::str("7")
        );
        assert_eq!(
            Key::parse("x", &Post::spec()).value(),
            Value::str("x")
        );
    }
}
